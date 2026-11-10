//! Offline E5 release and rollback checks using two deterministic fake versions.

use std::{collections::BTreeSet, env, sync::Arc};

use async_trait::async_trait;
use rockserver::persistence::{PostgresEmbeddingStore, PostgresStationRepository};
use rockserver::search::{
    DeterministicQueryParser, Embedding, EmbeddingBackfill, EmbeddingProvenance, EmbeddingProvider,
    EmbeddingProviderError, EmbeddingStore, QueryParserInput, SearchConstraints, SearchService,
};
use sqlx::PgPool;

struct FakeE5(&'static str);

#[async_trait]
impl EmbeddingProvider for FakeE5 {
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        Some(EmbeddingProvenance {
            model: "intfloat/multilingual-e5-small".to_owned(),
            version: self.0.to_owned(),
            dimension: 384,
        })
    }

    async fn embed(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.vector(true)
    }

    async fn embed_document(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        let alpha = text.contains("Alpha");
        self.vector(if self.0 == "onnx-v1" { alpha } else { !alpha })
    }
}

impl FakeE5 {
    fn vector(&self, first: bool) -> Result<Embedding, EmbeddingProviderError> {
        let mut values = vec![0.0; 384];
        values[usize::from(!first)] = 1.0;
        Embedding::new("intfloat/multilingual-e5-small", self.0, 384, values)
            .map_err(|error| EmbeddingProviderError::safe(error.to_string()))
    }
}

/// Partial new vectors leave old-provider ranking intact; complete new vectors can be
/// selected for an offline restart and switching back restores the old ranking.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn partial_build_complete_activation_and_rollback_keep_old_vectors() {
    assert_eq!(
        env::var("ROCKSERVER_SEARCH_EVAL_DISPOSABLE").as_deref(),
        Ok("1")
    );
    let url = env::var("TEST_DATABASE_URL").expect("isolated disposable database URL");
    let store = PostgresEmbeddingStore::connect(&url).await.unwrap();
    let repository = PostgresStationRepository::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query("TRUNCATE stations CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    for (id, name) in [
        ("srch010-alpha", "Alpha Rock"),
        ("srch010-beta", "Beta Rock"),
    ] {
        sqlx::query("INSERT INTO stations (id, source_station_id, name, tags, searchable_text) VALUES ($1, $1, $2, ARRAY['rock'], $2)")
            .bind(id).bind(name).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO station_streams (station_id, source_stream_id, stream_url, health, is_primary) VALUES ($1, $1, $2, 'healthy', true)")
            .bind(id).bind(format!("https://example.com/{id}.mp3"))
            .execute(&pool).await.unwrap();
    }

    EmbeddingBackfill::new(FakeE5("onnx-v1"), store.clone(), 2)
        .run()
        .await
        .unwrap();
    assert!(
        store
            .e5_release_readiness(&FakeE5("onnx-v1").provenance().unwrap())
            .await
            .unwrap()
            .is_ready()
    );
    let old_rank = ranked_ids(&repository, "onnx-v1").await;
    assert_eq!(old_rank.first().map(String::as_str), Some("srch010-alpha"));

    let staged = FakeE5("onnx-v2");
    let document = store
        .station_documents(&staged.provenance().unwrap(), None, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let vector = staged.embed_document(&document.text).await.unwrap();
    assert!(
        store
            .upsert_document_embedding(&document, &vector)
            .await
            .unwrap()
    );
    let partial = store
        .e5_release_readiness(&staged.provenance().unwrap())
        .await
        .unwrap();
    assert_eq!((partial.active_stations, partial.current_vectors), (2, 1));
    assert!(!partial.is_ready());
    assert_eq!(ranked_ids(&repository, "onnx-v1").await, old_rank);

    EmbeddingBackfill::new(FakeE5("onnx-v2"), store.clone(), 2)
        .run()
        .await
        .unwrap();
    assert!(
        store
            .e5_release_readiness(&staged.provenance().unwrap())
            .await
            .unwrap()
            .is_ready()
    );
    let new_rank = ranked_ids(&repository, "onnx-v2").await;
    assert_eq!(new_rank.first().map(String::as_str), Some("srch010-beta"));
    assert_eq!(ranked_ids(&repository, "onnx-v1").await, old_rank);

    // With a two-station fixture the planner prefers a sequential scan; forcing
    // index consideration proves the v2 SQL expression can use the generic HNSW.
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *transaction)
        .await
        .unwrap();
    let query_vector = format!("[1,{}]", vec!["0"; 383].join(","));
    let plan = sqlx::query_scalar::<_, String>(
        "EXPLAIN SELECT station_id FROM station_embeddings \
         WHERE model = 'intfloat/multilingual-e5-small' AND version = $1 AND dimension = 384 \
         ORDER BY embedding::vector(384) <=> ($2::text)::vector(384) LIMIT 200",
    )
    .bind("onnx-v2")
    .bind(query_vector)
    .fetch_all(&mut *transaction)
    .await
    .unwrap()
    .join("\n");
    assert!(
        plan.contains("station_embeddings_e5_384_hnsw_cosine_idx"),
        "{plan}"
    );
    transaction.rollback().await.unwrap();

    pool.close().await;
    repository.close().await;
    store.close().await;
}

async fn ranked_ids(repository: &PostgresStationRepository, version: &'static str) -> Vec<String> {
    let service = SearchService::with_providers(
        Arc::new(repository.clone()),
        Arc::new(DeterministicQueryParser),
        Some(Arc::new(FakeE5(version))),
    );
    service
        .interpret_and_search(
            QueryParserInput {
                query: "rock".to_owned(),
                locale: "en-US".to_owned(),
            },
            &SearchConstraints {
                limit: 10,
                offset: 0,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .unwrap()
        .stations
        .into_iter()
        .map(|item| item.station.id)
        .collect()
}

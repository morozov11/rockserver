//! PostgreSQL integration checks for versioned E5 ANN candidate retrieval.

use std::{collections::BTreeSet, env, sync::Arc};

use async_trait::async_trait;
use rockserver::persistence::{
    EMBEDDINGS_DIMENSION, EMBEDDINGS_MODEL, EMBEDDINGS_VERSION, EmbeddingVersionStore,
    WrittenStationEmbedding,
};
use rockserver::persistence::{PostgresStationRepository, station_fields_hash};
use rockserver::search::{
    DeterministicQueryParser, Embedding, EmbeddingProvenance, EmbeddingProvider,
    EmbeddingProviderError, SearchAction, SearchConstraints, SearchQuery, SearchService,
    StationRepository,
};
use sqlx::PgPool;

/// An E5 nearest neighbour with no lexical overlap must enter final scoring.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn vector_only_station_is_retrieved_and_other_provenances_fall_back() {
    let (repository, mut store, pool) = setup().await;
    station(&pool, "ann-target", "Lunar Signal", "en", "US", "healthy").await;
    embed(&mut store, "ann-target", &[1.0, 0.0]).await;
    let query = query();
    let constraints = constraints(5, 0);

    let semantic = repository
        .search(&query, &constraints, Some(&e5(1.0, 0.0)))
        .await
        .unwrap();
    assert_eq!(semantic[0].station.id, "ann-target");
    assert_eq!(semantic[0].total_matches, Some(1));
    let service = SearchService::with_providers(
        Arc::new(repository.clone()),
        Arc::new(DeterministicQueryParser),
        Some(Arc::new(FixedE5)),
    );
    // The unchanged 0.35 service gate hides pure semantic hits whose hybrid
    // score cannot exceed the 0.30 semantic weight; SRCH-005 owns this.
    assert!(
        service
            .search(&query, &constraints)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .search(&query, &constraints, None)
            .await
            .unwrap()
            .is_empty()
    );
    let other_model = Embedding::new("integration-model", "1", 3, vec![1.0, 0.0, 0.0]).unwrap();
    assert!(
        repository
            .search(&query, &constraints, Some(&other_model))
            .await
            .unwrap()
            .is_empty()
    );
    let wrong_e5_dimension = Embedding::new(
        "intfloat/multilingual-e5-small",
        "onnx-v1",
        3,
        vec![1.0, 0.0, 0.0],
    )
    .unwrap();
    assert!(
        repository
            .search(&query, &constraints, Some(&wrong_e5_dimension))
            .await
            .unwrap()
            .is_empty()
    );

    station(
        &pool,
        "ann-lexical-only",
        "Quartz Music",
        "en",
        "US",
        "healthy",
    )
    .await;
    let without_station_vector = repository
        .search(&query, &constraints, Some(&e5(1.0, 0.0)))
        .await
        .unwrap();
    assert!(
        without_station_vector
            .iter()
            .any(|row| row.station.id == "ann-lexical-only")
    );
    let exact_query = SearchQuery {
        prefer_station_name: true,
        station_name_hint_queries: vec!["quartz music".to_owned()],
        ..query
    };
    let metadata_only = repository
        .search(&exact_query, &constraints, None)
        .await
        .unwrap();
    assert_eq!(metadata_only[0].station.id, "ann-lexical-only");
    assert!(
        metadata_only[0].score > 1.0,
        "metadata fallback retains its scale"
    );
    embed(&mut store, "ann-lexical-only", &[1.0, 0.0]).await;
    let hybrid = repository
        .search(&exact_query, &constraints, Some(&e5(1.0, 0.0)))
        .await
        .unwrap();
    assert_eq!(hybrid[0].station.id, "ann-lexical-only");
    assert_eq!(hybrid[0].score, 1.0, "hybrid metadata must be bounded");

    pool.close().await;
    repository.close().await;
}

/// ANN respects hard filters, stream health, exclusions, and stable pages; a
/// stale `fields_hash` must drop a station back to the lexical path.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn vector_candidates_keep_filters_pagination_and_hash_freshness() {
    let (repository, mut store, pool) = setup().await;
    for (id, language, country, health, x, y) in [
        ("ann-a", "en", "US", "healthy", 1.0, 0.0),
        ("ann-b", "en", "US", "healthy", 1.0, 0.0),
        ("ann-degraded", "en", "US", "degraded", 1.0, 0.0),
        ("ann-french", "fr", "FR", "healthy", 1.0, 0.0),
    ] {
        station(&pool, id, "No Lexical Overlap", language, country, health).await;
        embed(&mut store, id, &[x, y]).await;
    }
    let mut query = query();
    query.language = Some("en".to_owned());
    query.country_code = Some("US".to_owned());
    let vector = e5(1.0, 0.0);
    let first = repository
        .search(&query, &constraints(1, 0), Some(&vector))
        .await
        .unwrap();
    let second = repository
        .search(&query, &constraints(1, 1), Some(&vector))
        .await
        .unwrap();
    let page = repository
        .search(&query, &constraints(2, 0), Some(&vector))
        .await
        .unwrap();
    // ann-a and ann-b share one vector: the station-id tie-break keeps pages
    // identical across repeated runs instead of breathing with scan order.
    assert_eq!(first[0].station.id, "ann-a");
    assert_eq!(second[0].station.id, "ann-b");
    assert_eq!(
        page.iter()
            .map(|row| row.station.id.as_str())
            .collect::<Vec<_>>(),
        vec!["ann-a", "ann-b"]
    );
    let repeat = repository
        .search(&query, &constraints(2, 0), Some(&vector))
        .await
        .unwrap();
    assert_eq!(
        repeat
            .iter()
            .map(|row| row.station.id.as_str())
            .collect::<Vec<_>>(),
        vec!["ann-a", "ann-b"],
        "candidate selection must be deterministic on equal distances"
    );
    assert_eq!(first[0].total_matches, Some(2));

    let excluded = SearchConstraints {
        excluded_station_ids: BTreeSet::from(["ann-a".to_owned()]),
        ..constraints(1, 0)
    };
    let result = repository
        .search(&query, &excluded, Some(&vector))
        .await
        .unwrap();
    assert_eq!(result[0].station.id, "ann-b");
    assert_eq!(result[0].total_matches, Some(1));

    // Simulate an import that changed fields: the stale vector must stop
    // matching even though the row still exists.
    let stale_name = "Renamed Overlap";
    let stale_hash = station_fields_hash(
        stale_name,
        &[], // tags column default of the helper is an empty array
        Some("en"),
        Some("US"),
    );
    sqlx::query("UPDATE stations SET name = $2, fields_hash = $3 WHERE id = $1")
        .bind("ann-a")
        .bind(stale_name)
        .bind(&stale_hash)
        .execute(&pool)
        .await
        .unwrap();
    let after_change = repository
        .search(&query, &constraints(2, 0), Some(&vector))
        .await
        .unwrap();
    assert_eq!(
        after_change
            .iter()
            .map(|row| row.station.id.as_str())
            .collect::<Vec<_>>(),
        vec!["ann-b"],
        "a stale fields_hash vector must not enter ANN candidates"
    );

    pool.close().await;
    repository.close().await;
}

/// The transaction-local iterative scan must fill 200 ANN slots after hard filters.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn filtered_hnsw_scan_fills_its_candidate_budget() {
    let (repository, mut store, pool) = setup().await;
    // Deterministic synthetic vectors exercise index depth only, not semantic quality.
    sqlx::query(
        "INSERT INTO stations (id, source_station_id, name, tags, language, country_code, searchable_text) \
         SELECT 'ann-depth-' || n, 'ann-depth-' || n, 'Depth Station ' || n, '{}', \
                CASE WHEN n % 3 = 0 THEN 'fr' ELSE 'en' END, 'US', 'Depth Station ' || n \
         FROM generate_series(1, 3000) AS n",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO station_streams (station_id, source_stream_id, stream_url, health, is_primary) \
         SELECT id, id, 'https://example.com/' || id, \
                CASE WHEN split_part(id, '-', 3)::int % 2 = 0 THEN 'degraded' ELSE 'healthy' END, true \
         FROM stations WHERE id LIKE 'ann-depth-%'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let pending = store
        .pending_stations(EMBEDDINGS_VERSION, 4_000)
        .await
        .unwrap();
    assert_eq!(pending.len(), 3_000);
    let mut batch = Vec::with_capacity(pending.len());
    for station in &pending {
        let index: f32 = station.station_id["ann-depth-".len()..]
            .parse()
            .expect("depth station index");
        let mut values = vec![0.0; EMBEDDINGS_DIMENSION];
        values[0] = 1.0;
        values[1] = index / 3000.0;
        batch.push(WrittenStationEmbedding {
            station_id: station.station_id.clone(),
            fields_hash: station.fields_hash.clone(),
            values,
        });
    }
    assert_eq!(
        store
            .write_embeddings(EMBEDDINGS_VERSION, &batch)
            .await
            .unwrap(),
        3_000
    );
    for table in ["stations", "station_streams", "station_embeddings_v1"] {
        sqlx::query(&format!("ANALYZE {table}"))
            .execute(&pool)
            .await
            .unwrap();
    }

    let query = SearchQuery {
        language: Some("en".to_owned()),
        country_code: Some("US".to_owned()),
        ..query()
    };
    let results = repository
        .search(&query, &constraints(10, 0), Some(&e5(1.0, 0.0)))
        .await
        .unwrap();
    assert_eq!(results.len(), 10);
    assert_eq!(results[0].total_matches, Some(200));
    assert!(results.iter().all(|row| row.station.health
        == rockserver::search::StationHealth::Healthy
        && row.station.language.as_deref() == Some("en")));

    pool.close().await;
    repository.close().await;
}

async fn setup() -> (PostgresStationRepository, EmbeddingVersionStore, PgPool) {
    assert_eq!(
        env::var("ROCKSERVER_SEARCH_EVAL_DISPOSABLE").as_deref(),
        Ok("1")
    );
    let url = env::var("TEST_DATABASE_URL").expect("set isolated TEST_DATABASE_URL");
    let repository = PostgresStationRepository::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query("TRUNCATE TABLE stations CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    let mut store = EmbeddingVersionStore::connect(&url).await.unwrap();
    store
        .ensure_table(EMBEDDINGS_VERSION, EMBEDDINGS_DIMENSION)
        .await
        .unwrap();
    (repository, store, pool)
}

async fn station(pool: &PgPool, id: &str, name: &str, language: &str, country: &str, health: &str) {
    sqlx::query("INSERT INTO stations (id, source_station_id, name, tags, language, country_code, searchable_text) VALUES ($1, $1, $2, '{}', $3, $4, $2)")
        .bind(id).bind(name).bind(language).bind(country).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO station_streams (station_id, source_stream_id, stream_url, health, is_primary) VALUES ($1, $1, 'https://example.com/' || $1, $2, true)")
        .bind(id).bind(health).execute(pool).await.unwrap();
}

/// Embeds one already-inserted station through the guarded write path.
async fn embed(store: &mut EmbeddingVersionStore, station_id: &str, prefix: &[f32]) {
    let pending = store
        .pending_stations(EMBEDDINGS_VERSION, 100)
        .await
        .unwrap();
    let target = pending
        .iter()
        .find(|station| station.station_id == station_id)
        .unwrap_or_else(|| panic!("station {station_id} must be pending"));
    let mut values = vec![0.0; EMBEDDINGS_DIMENSION];
    for (index, value) in prefix.iter().enumerate() {
        values[index] = *value;
    }
    let written = store
        .write_embeddings(
            EMBEDDINGS_VERSION,
            &[WrittenStationEmbedding {
                station_id: target.station_id.clone(),
                fields_hash: target.fields_hash.clone(),
                values,
            }],
        )
        .await
        .unwrap();
    assert_eq!(written, 1);
}

fn e5(x: f32, y: f32) -> Embedding {
    let mut values = vec![0.0; EMBEDDINGS_DIMENSION];
    values[0] = x;
    values[1] = y;
    Embedding::new(EMBEDDINGS_MODEL, "onnx-v1", EMBEDDINGS_DIMENSION, values).unwrap()
}

fn query() -> SearchQuery {
    SearchQuery {
        action: SearchAction::Play,
        original: "quartz".to_owned(),
        locale: "en-US".to_owned(),
        terms: vec!["quartz".to_owned()],
        tags: Vec::new(),
        language: None,
        country_code: None,
        core_term_count: 1,
        raw_query: "quartz".to_owned(),
        prefer_station_name: false,
        station_name_hint_queries: Vec::new(),
    }
}

fn constraints(limit: usize, offset: usize) -> SearchConstraints {
    SearchConstraints {
        limit,
        offset,
        excluded_station_ids: BTreeSet::new(),
    }
}

struct FixedE5;

#[async_trait]
impl EmbeddingProvider for FixedE5 {
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        Some(EmbeddingProvenance {
            model: EMBEDDINGS_MODEL.to_owned(),
            version: "onnx-v1".to_owned(),
            dimension: EMBEDDINGS_DIMENSION,
        })
    }

    async fn embed(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
        Ok(e5(1.0, 0.0))
    }
}

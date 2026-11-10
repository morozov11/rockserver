//! PostgreSQL integration checks for independent E5 ANN candidate retrieval.

use std::{collections::BTreeSet, env, sync::Arc};

use async_trait::async_trait;
use rockserver::persistence::{PostgresEmbeddingStore, PostgresStationRepository};
use rockserver::search::{
    DeterministicQueryParser, Embedding, EmbeddingProvider, EmbeddingProviderError, EmbeddingStore,
    SearchAction, SearchConstraints, SearchQuery, SearchService, StationRepository,
};
use sqlx::PgPool;

/// An E5 nearest neighbour with no lexical overlap must enter final scoring.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn vector_only_station_is_retrieved_and_other_models_fall_back() {
    let (repository, store, pool) = setup().await;
    station(&pool, "ann-target", "Lunar Signal", "en", "US", "healthy").await;
    store
        .upsert_embedding("ann-target", &e5(1.0, 0.0))
        .await
        .unwrap();
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
    store
        .upsert_embedding("ann-lexical-only", &e5(1.0, 0.0))
        .await
        .unwrap();
    let hybrid = repository
        .search(&exact_query, &constraints, Some(&e5(1.0, 0.0)))
        .await
        .unwrap();
    assert_eq!(hybrid[0].station.id, "ann-lexical-only");
    assert_eq!(hybrid[0].score, 1.0, "hybrid metadata must be bounded");

    pool.close().await;
    store.close().await;
    repository.close().await;
}

/// ANN respects hard filters, selected stream health, exclusions, and stable pages.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn vector_candidates_keep_filters_and_pagination() {
    let (repository, store, pool) = setup().await;
    for (id, language, country, health, x, y) in [
        ("ann-a", "en", "US", "healthy", 1.0, 0.0),
        ("ann-b", "en", "US", "healthy", 0.9, 0.1),
        ("ann-degraded", "en", "US", "degraded", 1.0, 0.0),
        ("ann-french", "fr", "FR", "healthy", 1.0, 0.0),
    ] {
        station(&pool, id, "No Lexical Overlap", language, country, health).await;
        store.upsert_embedding(id, &e5(x, y)).await.unwrap();
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
    assert_eq!(first[0].station.id, "ann-a");
    assert_eq!(second[0].station.id, "ann-b");
    assert_eq!(
        page.iter()
            .map(|row| row.station.id.as_str())
            .collect::<Vec<_>>(),
        vec!["ann-a", "ann-b"]
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

    pool.close().await;
    store.close().await;
    repository.close().await;
}

/// The transaction-local iterative scan must fill 200 ANN slots after hard filters.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn filtered_hnsw_scan_fills_its_candidate_budget() {
    let (repository, store, pool) = setup().await;
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
    sqlx::query(
        "INSERT INTO station_embeddings (station_id, model, version, dimension, embedding) \
         SELECT id, 'intfloat/multilingual-e5-small', 'onnx-v1', 384, \
                ('[1,' || (split_part(id, '-', 3)::float8 / 3000)::text || ',' \
                 || array_to_string(array_fill(0, ARRAY[382]), ',') || ']')::vector \
         FROM stations WHERE id LIKE 'ann-depth-%'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE stations")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ANALYZE station_streams")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ANALYZE station_embeddings")
        .execute(&pool)
        .await
        .unwrap();

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
    store.close().await;
    repository.close().await;
}

async fn setup() -> (PostgresStationRepository, PostgresEmbeddingStore, PgPool) {
    assert_eq!(
        env::var("ROCKSERVER_SEARCH_EVAL_DISPOSABLE").as_deref(),
        Ok("1")
    );
    let url = env::var("TEST_DATABASE_URL").expect("set isolated TEST_DATABASE_URL");
    let repository = PostgresStationRepository::connect(&url).await.unwrap();
    let store = PostgresEmbeddingStore::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query("TRUNCATE TABLE stations CASCADE")
        .execute(&pool)
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

fn e5(x: f32, y: f32) -> Embedding {
    let mut values = vec![0.0; 384];
    values[0] = x;
    values[1] = y;
    Embedding::new("intfloat/multilingual-e5-small", "onnx-v1", 384, values).unwrap()
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
    async fn embed(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
        Ok(e5(1.0, 0.0))
    }
}

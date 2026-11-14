//! Disposable PostgreSQL checks for the startup lifecycle of the versioned
//! embedding table (roadmap E2, audits 7/8): background fill, vector-branch
//! enablement, idempotent reruns, and the advisory lock.

use std::sync::Arc;

use rockserver::persistence::{
    EMBEDDINGS_DIMENSION, EMBEDDINGS_MODEL, EMBEDDINGS_VERSION, PostgresStationRepository,
};
use rockserver::providers::embedder::{EmbedderGateway, EmbedderLifecycle, EmbedderService};
use rockserver::search::{
    EmbeddingProvenance, EmbeddingProvider, SearchAction, SearchConstraints, SearchQuery,
    StationRepository,
};
use sqlx::{Connection as _, PgPool};

use crate::common::{FakeEmbedderService, disposable_url, embedding_store_setup, manual_station};

/// The lifecycle fills pending stations (including legacy `NULL` hashes),
/// stamps `stations.fields_hash`, and turns the ANN branch on.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn lifecycle_fills_pending_stations_and_enables_vector_search() {
    let url = disposable_url();
    // Connect before the reset: an empty catalog would otherwise trigger the
    // repository's pinned-catalog seeding and pollute the counts below.
    let repository = PostgresStationRepository::connect(&url).await.unwrap();
    let (store, pool) = embedding_store_setup(&url).await;
    drop(store);
    for id in ["lifecycle-alpha", "lifecycle-beta"] {
        manual_station(&pool, id, &format!("{id} station")).await;
    }
    let gateway = Arc::new(EmbedderGateway::disabled());
    let service = Arc::new(FakeEmbedderService::new());
    EmbedderLifecycle::new(Arc::clone(&service), Arc::clone(&gateway), url.clone())
        .run()
        .await;

    assert!(gateway.is_enabled());
    assert_eq!(
        gateway.provenance().unwrap(),
        EmbeddingProvenance {
            model: EMBEDDINGS_MODEL.to_owned(),
            version: "v1".to_owned(),
            dimension: EMBEDDINGS_DIMENSION,
        }
    );
    assert_eq!(vector_count(&pool).await, 2);
    let stamped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM stations WHERE fields_hash IS NOT NULL AND retired_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stamped, 2);
    // A second full lifecycle pass embeds nothing new.
    let calls_before = service.batch_calls();
    EmbedderLifecycle::new(Arc::clone(&service), Arc::clone(&gateway), url.clone())
        .run()
        .await;
    assert_eq!(
        service.batch_calls(),
        calls_before,
        "a covered catalog must not be re-embedded"
    );

    // The vector-only station is reachable through the real search SQL.
    let query = SearchQuery {
        action: SearchAction::Play,
        original: "zzz no lexical overlap".to_owned(),
        locale: "en-US".to_owned(),
        terms: vec!["zzznooverlap".to_owned()],
        tags: Vec::new(),
        language: None,
        country_code: None,
        core_term_count: 1,
        raw_query: "zzz no lexical overlap".to_owned(),
        prefer_station_name: false,
        station_name_hint_queries: Vec::new(),
    };
    let embedding = service.embed_query("anything").await.unwrap();
    let results = repository
        .search(
            &query,
            &SearchConstraints {
                limit: 10,
                offset: 0,
                excluded_station_ids: Default::default(),
            },
            Some(&embedding),
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].station.id, "lifecycle-alpha");

    pool.close().await;
    repository.close().await;
}

/// A second lifecycle waits on the advisory lock and proceeds once the holder
/// releases it (session-level lock on a dedicated connection).
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn advisory_lock_makes_the_second_lifecycle_wait() {
    let url = disposable_url();
    let (_, pool) = embedding_store_setup(&url).await;
    manual_station(&pool, "lock-alpha", "Lock Alpha station").await;
    let table = rockserver::persistence::embeddings_table_name(EMBEDDINGS_VERSION);
    let mut holder = sqlx::postgres::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind(&table)
        .execute(&mut holder)
        .await
        .unwrap();

    let gateway = Arc::new(EmbedderGateway::disabled());
    let lifecycle = EmbedderLifecycle::new(
        Arc::new(FakeEmbedderService::new()),
        Arc::clone(&gateway),
        url.clone(),
    );
    let task = tokio::spawn(lifecycle.run());
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert_eq!(
        vector_count(&pool).await,
        0,
        "the lifecycle must wait for the lock"
    );
    assert!(!gateway.is_enabled());

    sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(&table)
        .execute(&mut holder)
        .await
        .unwrap();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), task).await;

    assert_eq!(vector_count(&pool).await, 1);
    assert!(gateway.is_enabled());

    pool.close().await;
}

async fn vector_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM station_embeddings_v1")
        .fetch_one(pool)
        .await
        .unwrap()
}

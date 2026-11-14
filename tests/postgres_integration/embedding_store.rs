//! Disposable PostgreSQL checks for the versioned embedding store itself
//! (roadmap E2, audits 7/8): `ensure_table` verification, the import write
//! race, re-embedding after field changes, and side-by-side version staging.

use rockserver::catalog::{CatalogImporter, ImportLimits};
use rockserver::persistence::PostgresStationRepository;
use rockserver::persistence::{
    EMBEDDINGS_DIMENSION, EMBEDDINGS_VERSION, PostgresImportStore, embeddings_table_name,
    station_fields_hash,
};
use rockserver::providers::embedder::{EmbedderService, fill_version};
use rockserver::search::{SearchAction, SearchConstraints, SearchQuery, StationRepository};

use crate::common::{
    FakeEmbedderService, OnePageProvider, disposable_url, embedding_store_setup, imported_station,
    manual_station,
};

/// `ensure_table` creates the table once, accepts the identical schema again,
/// restores a dropped HNSW index, and rejects an incompatible column type.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn ensure_table_is_idempotent_and_verifies_schema() {
    let url = disposable_url();
    let (mut store, pool) = embedding_store_setup(&url).await;
    store
        .ensure_table(EMBEDDINGS_VERSION, EMBEDDINGS_DIMENSION)
        .await
        .unwrap();
    store
        .ensure_table(EMBEDDINGS_VERSION, EMBEDDINGS_DIMENSION)
        .await
        .unwrap();
    let table = embeddings_table_name(EMBEDDINGS_VERSION);
    let column_type: String = sqlx::query_scalar(&format!(
        "SELECT format_type(a.atttypid, a.atttypmod) FROM pg_attribute a \
         WHERE a.attrelid = '{table}'::regclass AND a.attname = 'embedding'"
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(column_type, "vector(384)");
    let index_am: String = sqlx::query_scalar(&format!(
        "SELECT am.amname FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid \
         JOIN pg_am am ON am.oid = c.relam WHERE i.indrelid = '{table}'::regclass \
         AND c.relname LIKE '%hnsw%'"
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(index_am, "hnsw");

    // A dropped HNSW index is restored by `ensure_table` (CREATE IF NOT EXISTS).
    sqlx::query(&format!("DROP INDEX {table}_hnsw_cosine_idx"))
        .execute(&pool)
        .await
        .unwrap();
    store
        .ensure_table(EMBEDDINGS_VERSION, EMBEDDINGS_DIMENSION)
        .await
        .unwrap();
    let index_restored: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM pg_class WHERE relname = '{table}_hnsw_cosine_idx')"
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(index_restored);

    // An incompatible column type cannot be fixed in place: verification must
    // refuse and the caller keeps the no-vectors mode.
    sqlx::query(&format!(
        "ALTER TABLE {table} ALTER COLUMN embedding TYPE vector(768) USING embedding::vector(768)"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        store
            .ensure_table(EMBEDDINGS_VERSION, EMBEDDINGS_DIMENSION)
            .await
            .is_err()
    );

    // Leave the database usable: drop the broken table and let `ensure_table`
    // rebuild the correct schema for the rest of the suite.
    sqlx::query(&format!("DROP TABLE {table}"))
        .execute(&pool)
        .await
        .unwrap();
    store
        .ensure_table(EMBEDDINGS_VERSION, EMBEDDINGS_DIMENSION)
        .await
        .unwrap();

    pool.close().await;
}

/// A vector computed from outdated fields is not written; the station stays
/// pending and the next batch embeds the changed fields (import race). A
/// re-import of identical fields keeps the vector row.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn import_race_rejects_stale_writes_and_reembeds_changed_fields() {
    let url = disposable_url();
    let (mut store, pool) = embedding_store_setup(&url).await;
    let import_store = PostgresImportStore::connect(&url).await.unwrap();
    let source_id = "50c0ffee-0000-4000-8000-000000000e2e";
    let initial = imported_station(
        source_id,
        "Initial Alpha Radio",
        "https://streams.example.com/race.mp3",
        &["rock"],
    );
    CatalogImporter::new(
        OnePageProvider::station(initial),
        import_store.clone(),
        ImportLimits {
            page_size: 10,
            max_pages: 1,
        },
    )
    .run()
    .await
    .unwrap();
    let station_id = format!("rb-{source_id}");
    let changed = imported_station(
        source_id,
        "Changed Beta Radio",
        "https://streams.example.com/race.mp3",
        &["rock"],
    );

    let service =
        FakeEmbedderService::new().importing_on_first_batch(import_store.clone(), changed);
    let stats = fill_version(&service, &mut store, EMBEDDINGS_VERSION, 64)
        .await
        .unwrap();
    // First batch lost the race (0 written), the retry embedded the new name.
    assert_eq!(stats.processed, 2);
    assert_eq!(stats.written, 1);

    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT fields_hash, name FROM stations WHERE id = $1",
    )
    .bind(&station_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.1, "Changed Beta Radio");
    let expected_hash = station_fields_hash(
        "Changed Beta Radio",
        &["rock".to_owned()],
        Some("en"),
        Some("US"),
    );
    assert_eq!(row.0, expected_hash);
    let vector_hash: String =
        sqlx::query_scalar("SELECT fields_hash FROM station_embeddings_v1 WHERE station_id = $1")
            .bind(&station_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(vector_hash, expected_hash);
    // Re-importing identical fields keeps the vector row (no invalidation).
    let unchanged = imported_station(
        source_id,
        "Changed Beta Radio",
        "https://streams.example.com/race.mp3",
        &["rock"],
    );
    CatalogImporter::new(
        OnePageProvider::station(unchanged),
        import_store.clone(),
        ImportLimits {
            page_size: 10,
            max_pages: 1,
        },
    )
    .run()
    .await
    .unwrap();
    let preserved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM station_embeddings_v1 WHERE station_id = $1 AND fields_hash = $2",
    )
    .bind(&station_id)
    .bind(&expected_hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        preserved, 1,
        "imports must not invalidate versioned vectors"
    );

    import_store.close().await;
    pool.close().await;
}

/// Staging the next table version fills v2 without touching the released v1
/// rows that the current build's search SQL still reads (audit 8).
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn staging_next_version_keeps_released_rows_intact() {
    let url = disposable_url();
    // Connect before the reset (see the lifecycle test module).
    let repository = PostgresStationRepository::connect(&url).await.unwrap();
    let (mut store, pool) = embedding_store_setup(&url).await;
    manual_station(&pool, "stage-alpha", "Stage Alpha station").await;
    manual_station(&pool, "stage-beta", "Stage Beta station").await;

    let service = FakeEmbedderService::new();
    fill_version(&service, &mut store, EMBEDDINGS_VERSION, 64)
        .await
        .unwrap();
    let v1_before = sqlx::query_as::<_, (String, String)>(
        "SELECT station_id, fields_hash FROM station_embeddings_v1 ORDER BY station_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    // Stage the next version beside the released one.
    store
        .ensure_table(EMBEDDINGS_VERSION + 1, EMBEDDINGS_DIMENSION)
        .await
        .unwrap();
    let staged = fill_version(&service, &mut store, EMBEDDINGS_VERSION + 1, 64)
        .await
        .unwrap();
    assert_eq!(staged.written, 2);
    let v1_after = sqlx::query_as::<_, (String, String)>(
        "SELECT station_id, fields_hash FROM station_embeddings_v1 ORDER BY station_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(v1_before, v1_after, "staging v2 must not disturb v1");
    let v2_count: i64 = sqlx::query_scalar("SELECT count(*) FROM station_embeddings_v2")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(v2_count, 2);

    // The search path of this build still answers from v1.
    let query = SearchQuery {
        action: SearchAction::Play,
        original: "zzz stage probe".to_owned(),
        locale: "en-US".to_owned(),
        terms: vec!["zzzstageprobe".to_owned()],
        tags: Vec::new(),
        language: None,
        country_code: None,
        core_term_count: 1,
        raw_query: "zzz stage probe".to_owned(),
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
    assert_eq!(results[0].station.id, "stage-alpha");

    pool.close().await;
    repository.close().await;
}

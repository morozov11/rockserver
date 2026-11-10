//! Disposable PostgreSQL checks for incremental, restartable embedding backfill.

use std::{
    env,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use rockserver::catalog::{CatalogImporter, ImportLimits};
use rockserver::persistence::{PostgresEmbeddingStore, PostgresImportStore};
use rockserver::search::{
    Embedding, EmbeddingBackfill, EmbeddingProvenance, EmbeddingProvider, EmbeddingProviderError,
    EmbeddingStore,
};
use sqlx::PgPool;

use crate::common::{OnePageProvider, imported_station};

struct CountingProvider {
    calls: Arc<AtomicUsize>,
    fail_at: Option<usize>,
}

#[async_trait]
impl EmbeddingProvider for CountingProvider {
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        Some(EmbeddingProvenance {
            model: "srch009-fake".to_owned(),
            version: "1".to_owned(),
            dimension: 2,
        })
    }

    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.embed_document(text).await
    }

    async fn embed_document(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_at == Some(call) {
            return Err(EmbeddingProviderError::safe("scripted interruption"));
        }
        let first = if text.contains("changed") { 2.0 } else { 1.0 };
        Ok(Embedding::new("srch009-fake", "1", 2, vec![first, 1.0]).unwrap())
    }
}

async fn run(store: &PostgresEmbeddingStore, fail_at: Option<usize>) -> (usize, Option<usize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let workflow = EmbeddingBackfill::new(
        CountingProvider {
            calls: calls.clone(),
            fail_at,
        },
        store.clone(),
        2,
    );
    let result = workflow.run().await;
    (
        calls.load(Ordering::SeqCst),
        result.ok().map(|value| value.updated),
    )
}

/// A failed middle document resumes by stable ID without recomputing prior rows.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn only_missing_or_changed_documents_are_embedded_and_restart_resumes() {
    let (store, pool) = setup().await;
    for id in ["srch009-a", "srch009-b", "srch009-c"] {
        insert_station(&pool, id, id).await;
    }

    assert_eq!(run(&store, Some(2)).await, (2, None));
    assert_eq!(vector_count(&pool).await, 1);
    assert_eq!(run(&store, None).await, (2, Some(2)));
    assert_eq!(vector_count(&pool).await, 3);
    assert_eq!(run(&store, None).await, (0, Some(0)));

    sqlx::query("UPDATE stations SET searchable_text = 'changed station' WHERE id = 'srch009-b'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(run(&store, None).await, (1, Some(1)));
    assert_eq!(run(&store, None).await, (0, Some(0)));

    pool.close().await;
    store.close().await;
}

/// A source change between page read and vector write cannot attest a stale vector.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn changed_source_rejects_stale_vector_write() {
    let (store, pool) = setup().await;
    insert_station(&pool, "srch009-stale", "original station").await;
    let provider = CountingProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        fail_at: None,
    };
    let document = store
        .station_documents(&provider.provenance().unwrap(), None, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    sqlx::query(
        "UPDATE stations SET searchable_text = 'changed station' WHERE id = 'srch009-stale'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let embedding = provider.embed_document(&document.text).await.unwrap();
    assert!(
        !store
            .upsert_document_embedding(&document, &embedding)
            .await
            .unwrap()
    );
    assert_eq!(vector_count(&pool).await, 0);
    assert_eq!(run(&store, None).await, (1, Some(1)));

    pool.close().await;
    store.close().await;
}

/// Importing non-searchable metadata preserves a vector; changing text invalidates one.
#[tokio::test]
#[ignore = "requires an isolated disposable PostgreSQL TEST_DATABASE_URL"]
async fn import_invalidates_only_when_searchable_text_changes() {
    let (store, pool) = setup().await;
    let url = env::var("TEST_DATABASE_URL").unwrap();
    let import_store = PostgresImportStore::connect(&url).await.unwrap();
    let source_id = "00900000-0000-4000-8000-000000000001";
    let make_station = |name: &str, homepage: &str| {
        let mut station = imported_station(
            source_id,
            name,
            "https://streams.example.com/srch009.mp3",
            &["rock"],
        );
        station.homepage_url = Some(homepage.to_owned());
        station
    };
    for (station, before, inferences) in [
        (
            make_station("Initial Radio", "https://example.com/initial"),
            0,
            1,
        ),
        (
            make_station("Initial Radio", "https://example.com/new-homepage"),
            1,
            0,
        ),
        (
            make_station("Changed Radio", "https://example.com/new-homepage"),
            0,
            1,
        ),
    ] {
        CatalogImporter::new(
            OnePageProvider::station(station),
            import_store.clone(),
            ImportLimits {
                page_size: 10,
                max_pages: 1,
            },
        )
        .run()
        .await
        .unwrap();
        assert_eq!(vector_count(&pool).await, before);
        assert_eq!(run(&store, None).await, (inferences, Some(inferences)));
    }
    // The final name change re-embedded the single imported station.
    assert_eq!(vector_count(&pool).await, 1);
    import_store.close().await;
    pool.close().await;
    store.close().await;
}

async fn setup() -> (PostgresEmbeddingStore, PgPool) {
    assert_eq!(
        env::var("ROCKSERVER_SEARCH_EVAL_DISPOSABLE").as_deref(),
        Ok("1")
    );
    let url = env::var("TEST_DATABASE_URL").expect("isolated disposable database URL");
    let store = PostgresEmbeddingStore::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query("TRUNCATE stations CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    (store, pool)
}

async fn insert_station(pool: &PgPool, id: &str, text: &str) {
    sqlx::query("INSERT INTO stations (id, source_station_id, name, tags, searchable_text) VALUES ($1, $1, $1, '{}', $2)")
        .bind(id).bind(text).execute(pool).await.unwrap();
}

async fn vector_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM station_embeddings WHERE model = 'srch009-fake'")
        .fetch_one(pool)
        .await
        .unwrap()
}

//! Shared helper functions and test providers for PostgreSQL integration tests.

use async_trait::async_trait;
use rockserver::catalog::{
    CatalogImportError, CatalogImportProvider, CatalogImporter, ImportLimits, ImportPage,
    ImportedStation, ImportedStream,
};
use rockserver::persistence::{
    EMBEDDINGS_DIMENSION, EMBEDDINGS_MODEL, EMBEDDINGS_VERSION, EmbeddingVersionStore,
    PendingStationEmbedding, PostgresImportStore,
};
use rockserver::providers::embedder::EmbedderService;
use rockserver::providers::radio_browser::SOURCE;
use rockserver::search::{Embedding, EmbeddingProvenance, EmbeddingProviderError, RankedStation};
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;

/// Connects an inspection pool to the target test database.
pub(crate) async fn repository_pool(database_url: &str) -> PgPool {
    PgPool::connect(database_url)
        .await
        .expect("inspection connection must succeed")
}

/// Extracts station IDs from ranked search results.
pub(crate) fn station_ids(results: &[RankedStation]) -> Vec<&str> {
    results
        .iter()
        .map(|ranked| ranked.station.id.as_str())
        .collect()
}

/// A deterministic one-page import provider for testing catalog import pipelines.
pub(crate) struct OnePageProvider {
    pub(crate) station: Option<ImportedStation>,
    pub(crate) fail: bool,
}

impl OnePageProvider {
    pub(crate) fn station(station: ImportedStation) -> Self {
        Self {
            station: Some(station),
            fail: false,
        }
    }

    pub(crate) fn failure() -> Self {
        Self {
            station: None,
            fail: true,
        }
    }
}

#[async_trait]
impl CatalogImportProvider for OnePageProvider {
    fn source(&self) -> &'static str {
        SOURCE
    }

    async fn fetch_page(
        &self,
        _offset: usize,
        _limit: usize,
    ) -> Result<ImportPage, CatalogImportError> {
        if self.fail {
            return Err(CatalogImportError::safe("mock provider unavailable"));
        }
        Ok(ImportPage {
            fetched: usize::from(self.station.is_some()),
            stations: self.station.clone().into_iter().collect(),
            skipped: 0,
        })
    }
}

/// Constructs a synthetic `ImportedStation` for provider tests.
pub(crate) fn imported_station(
    source_station_id: &str,
    name: &str,
    stream_url: &str,
    tags: &[&str],
) -> ImportedStation {
    ImportedStation {
        source: SOURCE,
        source_station_id: source_station_id.to_owned(),
        id: format!("rb-{source_station_id}"),
        name: name.to_owned(),
        homepage_url: Some("https://example.com/imported-radio".to_owned()),
        favicon_source_url: Some("https://icons.example.com/imported-radio.png".to_owned()),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        language: Some("en".to_owned()),
        country_code: Some("US".to_owned()),
        streams: vec![ImportedStream {
            source_stream_id: source_station_id.to_owned(),
            stream_url: stream_url.to_owned(),
            codec: Some("MP3".to_owned()),
            bitrate_kbps: Some(192),
            is_primary: true,
        }],
    }
}

/// Asserts the completion status, counts, and error summary of an import run.
pub(crate) async fn assert_run(
    pool: &PgPool,
    run_id: &str,
    expected_status: &str,
    expected_counts: (i64, i64, i64, i64),
    expects_error: bool,
) {
    let run_id = Uuid::parse_str(run_id).unwrap();
    let row = sqlx::query_as::<_, (String, i64, i64, i64, i64, Option<String>, bool)>(
        r#"
SELECT status, fetched_count, imported_count, skipped_count, failed_count, error_summary,
       completed_at IS NOT NULL
FROM import_runs
WHERE id = $1
"#,
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(row.0, expected_status);
    assert_eq!((row.1, row.2, row.3, row.4), expected_counts);
    assert_eq!(row.5.is_some(), expects_error);
    assert!(row.6, "terminal run must have completed_at");
}

/// Deterministic fake of the embedding service (`embed.v1`) for lifecycle and
/// versioned-store tests. Vectors distinguish stations by name so tests can
/// observe re-embedding after field changes; the first batch can run a
/// competing catalog import to exercise the write race.
pub(crate) struct FakeEmbedderService {
    import_store: Option<PostgresImportStore>,
    changed_station: Option<ImportedStation>,
    import_ran: std::sync::Mutex<bool>,
    batch_calls: Arc<AtomicUsize>,
}

impl FakeEmbedderService {
    pub(crate) fn new() -> Self {
        Self {
            import_store: None,
            changed_station: None,
            import_ran: std::sync::Mutex::new(false),
            batch_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Runs a competing import inside the first `embed_stations` call.
    pub(crate) fn importing_on_first_batch(
        mut self,
        store: PostgresImportStore,
        station: ImportedStation,
    ) -> Self {
        self.import_store = Some(store);
        self.changed_station = Some(station);
        self
    }

    /// Number of `embed_stations` batches served so far.
    pub(crate) fn batch_calls(&self) -> usize {
        self.batch_calls.load(Ordering::SeqCst)
    }

    fn vector_for(name: &str) -> Vec<f32> {
        let mut values = vec![0.0; EMBEDDINGS_DIMENSION];
        if name.to_lowercase().contains("alpha") {
            values[0] = 1.0;
        } else {
            values[1] = 1.0;
        }
        values
    }
}

#[async_trait]
impl EmbedderService for FakeEmbedderService {
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        Some(EmbeddingProvenance {
            model: EMBEDDINGS_MODEL.to_owned(),
            version: "v1".to_owned(),
            dimension: EMBEDDINGS_DIMENSION,
        })
    }

    async fn info(&self) -> Result<EmbeddingProvenance, EmbeddingProviderError> {
        Ok(self.provenance().unwrap())
    }

    async fn embed_query(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
        Ok(Embedding::new(
            EMBEDDINGS_MODEL,
            "v1",
            EMBEDDINGS_DIMENSION,
            Self::vector_for("Alpha Query"),
        )
        .unwrap())
    }

    async fn embed_stations(
        &self,
        batch: &[PendingStationEmbedding],
    ) -> Result<Vec<Embedding>, EmbeddingProviderError> {
        self.batch_calls.fetch_add(1, Ordering::SeqCst);
        // The scripted race: a concurrent import changes station fields
        // between the pending read and the guarded vector write. The flag
        // flips before awaiting so no sync guard crosses an await point.
        let run_import_now = {
            let mut import_ran = self.import_ran.lock().unwrap();
            std::mem::replace(&mut *import_ran, true)
        };
        if !run_import_now
            && let (Some(store), Some(station)) = (&self.import_store, &self.changed_station)
        {
            CatalogImporter::new(
                OnePageProvider::station(station.clone()),
                store.clone(),
                ImportLimits {
                    page_size: 10,
                    max_pages: 1,
                },
            )
            .run()
            .await
            .expect("racing import must succeed");
        }
        Ok(batch
            .iter()
            .map(|station| {
                Embedding::new(
                    EMBEDDINGS_MODEL,
                    "v1",
                    EMBEDDINGS_DIMENSION,
                    Self::vector_for(&station.name),
                )
                .unwrap()
            })
            .collect())
    }
}

/// Connects a verified versioned-embedding store and empties the catalog
/// (CASCADE also clears `station_embeddings_v1` rows via the foreign key).
pub(crate) async fn embedding_store_setup(url: &str) -> (EmbeddingVersionStore, PgPool) {
    let mut store = EmbeddingVersionStore::connect(url).await.unwrap();
    store
        .ensure_table(EMBEDDINGS_VERSION, EMBEDDINGS_DIMENSION)
        .await
        .unwrap();
    let pool = PgPool::connect(url).await.unwrap();
    sqlx::query("TRUNCATE stations CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    (store, pool)
}

/// Inserts a station with a primary healthy stream and no `fields_hash`.
pub(crate) async fn manual_station(pool: &PgPool, id: &str, name: &str) {
    sqlx::query("INSERT INTO stations (id, source_station_id, name, tags, language, country_code, searchable_text) VALUES ($1, $1, $2, '{}', 'en', 'US', $2)")
        .bind(id).bind(name).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO station_streams (station_id, source_stream_id, stream_url, health, is_primary) VALUES ($1, $1, 'https://example.com/' || $1, 'healthy', true)")
        .bind(id).execute(pool).await.unwrap();
}

/// The disposable-database guard and URL used by the embedding tests.
pub(crate) fn disposable_url() -> String {
    assert_eq!(
        std::env::var("ROCKSERVER_SEARCH_EVAL_DISPOSABLE").as_deref(),
        Ok("1")
    );
    std::env::var("TEST_DATABASE_URL").expect("isolated disposable database URL")
}

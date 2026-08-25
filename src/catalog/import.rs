//! Provider-neutral orchestration for bounded catalog imports.

use std::{error::Error, fmt};

use async_trait::async_trait;

/// A normalized station ready to be owned and upserted by an external provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedStation {
    /// Provider name used for ownership and conflict isolation.
    pub source: &'static str,
    /// Stable station identifier supplied by the provider.
    pub source_station_id: String,
    /// Stable RockServer identifier derived from the source identity.
    pub id: String,
    /// Normalized display name.
    pub name: String,
    /// Valid HTTP(S) station homepage, when supplied.
    pub homepage_url: Option<String>,
    /// Sorted, deduplicated searchable tags.
    pub tags: Vec<String>,
    /// Normalized ISO 639-style language code, when supplied.
    pub language: Option<String>,
    /// Normalized ISO 3166-1 alpha-2 country code, when supplied.
    pub country_code: Option<String>,
    /// Provider-owned streams. Every imported station has at least one stream.
    pub streams: Vec<ImportedStream>,
}

/// A provider-owned playable stream belonging to an [`ImportedStation`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedStream {
    /// Stable stream identifier inside the provider namespace.
    pub source_stream_id: String,
    /// Valid direct or resolved HTTP(S) stream URL.
    pub stream_url: String,
    /// Normalized codec label, when supplied.
    pub codec: Option<String>,
    /// Plausible positive bitrate in kilobits per second, when supplied.
    pub bitrate_kbps: Option<u32>,
    /// Whether this is the single stream selected by the public search contract.
    pub is_primary: bool,
}

/// Lifecycle instruction retained for a canonical station that is no longer active.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedTombstone {
    /// The permanently reserved canonical station identifier that was retired.
    pub id: String,
    /// The contract-defined retirement meaning.
    pub reason: TombstoneReason,
    /// Active canonical IDs that may replace the retired identity.
    pub replacement_ids: Vec<String>,
}

/// Contract-defined meaning of a canonical station retirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TombstoneReason {
    /// The station has no successor.
    Removed,
    /// The station has exactly one continuity successor and may be redirected.
    Merged,
    /// The station has several successors and must remain ambiguous to callers.
    Split,
}

impl TombstoneReason {
    /// Returns the stable database value for this retirement meaning.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Merged => "merged",
            Self::Split => "split",
        }
    }
}

/// One bounded page returned by an external catalog provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportPage {
    /// Number of upstream DTOs received before validation.
    pub fetched: usize,
    /// Valid normalized stations from this page.
    pub stations: Vec<ImportedStation>,
    /// DTOs deliberately rejected by validation and skip rules.
    pub skipped: usize,
}

/// Aggregate counters persisted with an import run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImportCounts {
    /// Upstream DTOs fetched.
    pub fetched: usize,
    /// Stations successfully upserted, including deterministic updates.
    pub imported: usize,
    /// Upstream DTOs rejected by documented validation rules.
    pub skipped: usize,
    /// Normalized stations that could not be persisted.
    pub failed: usize,
}

/// Identifier and final counts returned after a completed import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportRunResult {
    /// Database-owned run identifier.
    pub run_id: String,
    /// Final persisted counters.
    pub counts: ImportCounts,
}

/// Safe operational failure shared across import boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogImportError {
    summary: String,
}

impl CatalogImportError {
    /// Creates a failure summary that must not contain credentials, DSNs, or stream URLs.
    pub fn safe(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }

    /// Returns the sanitized summary suitable for `import_runs.error_summary`.
    pub fn safe_summary(&self) -> &str {
        &self.summary
    }
}

impl fmt::Display for CatalogImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.summary)
    }
}

impl Error for CatalogImportError {}

/// External source boundary used by the import orchestrator.
#[async_trait]
pub trait CatalogImportProvider: Send + Sync {
    /// Returns the stable provider name used for catalog ownership.
    fn source(&self) -> &'static str;

    /// Fetches and normalizes one page at the requested absolute offset.
    async fn fetch_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<ImportPage, CatalogImportError>;
}

/// Persistence boundary dedicated to import ownership and run bookkeeping.
#[async_trait]
pub trait CatalogImportStore: Send + Sync {
    /// Creates a durable run in the `started` state and returns its identifier.
    async fn start_run(&self, source: &str) -> Result<String, CatalogImportError>;

    /// Idempotently upserts one normalized batch under a source tied to the started run.
    ///
    /// Implementations must use the explicit `source` as the ownership namespace rather than
    /// trusting a record field, and return the successful row count.
    async fn upsert_batch(
        &self,
        run_id: &str,
        source: &str,
        stations: &[ImportedStation],
    ) -> Result<usize, CatalogImportError>;

    /// Finalizes a run as completed with its aggregate counts.
    async fn complete_run(
        &self,
        run_id: &str,
        counts: ImportCounts,
    ) -> Result<(), CatalogImportError>;

    /// Finalizes a run as failed with aggregate counts and a sanitized summary.
    async fn fail_run(
        &self,
        run_id: &str,
        counts: ImportCounts,
        error_summary: &str,
    ) -> Result<(), CatalogImportError>;
}

/// Safe pagination limits applied independently of upstream defaults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportLimits {
    /// Maximum DTOs requested in one page.
    pub page_size: usize,
    /// Maximum pages requested in one run.
    pub max_pages: usize,
}

/// Runs a provider import outside the HTTP request and startup paths.
pub struct CatalogImporter<P, S> {
    provider: P,
    store: S,
    limits: ImportLimits,
}

impl<P, S> CatalogImporter<P, S>
where
    P: CatalogImportProvider,
    S: CatalogImportStore,
{
    /// Creates an importer with already-validated pagination limits.
    pub fn new(provider: P, store: S, limits: ImportLimits) -> Self {
        Self {
            provider,
            store,
            limits,
        }
    }

    /// Executes one bounded run and durably records completion or failure.
    pub async fn run(&self) -> Result<ImportRunResult, CatalogImportError> {
        let source = self.provider.source();
        let run_id = self.store.start_run(source).await?;
        let mut counts = ImportCounts::default();
        tracing::info!(%run_id, source, "catalog import started");

        for page_index in 0..self.limits.max_pages {
            let offset = page_index * self.limits.page_size;
            let page = match self
                .provider
                .fetch_page(offset, self.limits.page_size)
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    self.record_failure(&run_id, counts, &error).await?;
                    return Err(error);
                }
            };

            counts.fetched += page.fetched;
            counts.skipped += page.skipped;
            let normalized_count = page.stations.len();
            // Reject the whole page before persistence when a provider crosses its run namespace.
            if page.stations.iter().any(|station| station.source != source) {
                counts.failed += normalized_count;
                let error = CatalogImportError::safe(
                    "Catalog provider returned mismatched source ownership",
                );
                self.record_failure(&run_id, counts, &error).await?;
                return Err(error);
            }
            if !page.stations.is_empty() {
                match self
                    .store
                    .upsert_batch(&run_id, source, &page.stations)
                    .await
                {
                    Ok(imported) => counts.imported += imported,
                    Err(error) => {
                        counts.failed += normalized_count;
                        self.record_failure(&run_id, counts, &error).await?;
                        return Err(error);
                    }
                }
            }

            tracing::info!(
                %run_id,
                page = page_index + 1,
                offset,
                fetched = page.fetched,
                imported = normalized_count,
                skipped = page.skipped,
                "catalog import page completed"
            );

            if page.fetched < self.limits.page_size {
                break;
            }
        }

        self.store.complete_run(&run_id, counts).await?;
        tracing::info!(
            %run_id,
            fetched = counts.fetched,
            imported = counts.imported,
            skipped = counts.skipped,
            failed = counts.failed,
            "catalog import completed"
        );
        Ok(ImportRunResult { run_id, counts })
    }

    // Failure recording is kept in one place so every post-start exit attempts terminal status.
    async fn record_failure(
        &self,
        run_id: &str,
        counts: ImportCounts,
        error: &CatalogImportError,
    ) -> Result<(), CatalogImportError> {
        tracing::error!(%run_id, error = %error, "catalog import failed");
        self.store
            .fail_run(run_id, counts, error.safe_summary())
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{
        CatalogImportError, CatalogImportProvider, CatalogImportStore, CatalogImporter,
        ImportCounts, ImportLimits, ImportPage, ImportedStation, ImportedStream,
    };

    struct ScriptedProvider {
        offsets: Mutex<Vec<usize>>,
        pages: Mutex<Vec<ImportPage>>,
        failure_after_pages: Option<usize>,
    }

    #[async_trait]
    impl CatalogImportProvider for ScriptedProvider {
        fn source(&self) -> &'static str {
            "test"
        }

        async fn fetch_page(
            &self,
            offset: usize,
            _limit: usize,
        ) -> Result<ImportPage, CatalogImportError> {
            let mut offsets = self.offsets.lock().unwrap();
            if self.failure_after_pages == Some(offsets.len()) {
                return Err(CatalogImportError::safe("sanitized provider failure"));
            }
            offsets.push(offset);
            Ok(self.pages.lock().unwrap().remove(0))
        }
    }

    #[derive(Default)]
    struct RecordingStore {
        batches: Mutex<Vec<Vec<ImportedStation>>>,
        completed: Mutex<Option<ImportCounts>>,
        failed: Mutex<Option<(ImportCounts, String)>>,
    }

    #[async_trait]
    impl CatalogImportStore for RecordingStore {
        async fn start_run(&self, _source: &str) -> Result<String, CatalogImportError> {
            Ok("run-test".to_owned())
        }

        async fn upsert_batch(
            &self,
            _run_id: &str,
            _source: &str,
            stations: &[ImportedStation],
        ) -> Result<usize, CatalogImportError> {
            self.batches.lock().unwrap().push(stations.to_vec());
            Ok(stations.len())
        }

        async fn complete_run(
            &self,
            _run_id: &str,
            counts: ImportCounts,
        ) -> Result<(), CatalogImportError> {
            *self.completed.lock().unwrap() = Some(counts);
            Ok(())
        }

        async fn fail_run(
            &self,
            _run_id: &str,
            counts: ImportCounts,
            error_summary: &str,
        ) -> Result<(), CatalogImportError> {
            *self.failed.lock().unwrap() = Some((counts, error_summary.to_owned()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn importer_stops_after_a_short_page_and_aggregates_counts() {
        let provider = ScriptedProvider {
            offsets: Mutex::new(Vec::new()),
            pages: Mutex::new(vec![
                ImportPage {
                    fetched: 2,
                    stations: vec![station("one")],
                    skipped: 1,
                },
                ImportPage {
                    fetched: 1,
                    stations: vec![station("two")],
                    skipped: 0,
                },
            ]),
            failure_after_pages: None,
        };
        let store = RecordingStore::default();
        let importer = CatalogImporter::new(
            provider,
            store,
            ImportLimits {
                page_size: 2,
                max_pages: 5,
            },
        );

        let result = importer.run().await.unwrap();

        assert_eq!(
            result.counts,
            ImportCounts {
                fetched: 3,
                imported: 2,
                skipped: 1,
                failed: 0,
            }
        );
        assert_eq!(*importer.provider.offsets.lock().unwrap(), [0, 2]);
        assert_eq!(
            *importer.store.completed.lock().unwrap(),
            Some(result.counts)
        );
    }

    #[tokio::test]
    async fn importer_never_fetches_more_than_max_pages() {
        let provider = ScriptedProvider {
            offsets: Mutex::new(Vec::new()),
            pages: Mutex::new(vec![
                ImportPage {
                    fetched: 2,
                    stations: vec![station("one"), station("two")],
                    skipped: 0,
                },
                ImportPage {
                    fetched: 2,
                    stations: vec![station("three"), station("four")],
                    skipped: 0,
                },
            ]),
            failure_after_pages: None,
        };
        let importer = CatalogImporter::new(
            provider,
            RecordingStore::default(),
            ImportLimits {
                page_size: 2,
                max_pages: 2,
            },
        );

        let result = importer.run().await.unwrap();

        assert_eq!(result.counts.fetched, 4);
        assert_eq!(*importer.provider.offsets.lock().unwrap(), [0, 2]);
    }

    #[tokio::test]
    async fn provider_failure_is_recorded_with_partial_counts() {
        let provider = ScriptedProvider {
            offsets: Mutex::new(Vec::new()),
            pages: Mutex::new(vec![ImportPage {
                fetched: 2,
                stations: vec![station("one")],
                skipped: 1,
            }]),
            failure_after_pages: Some(1),
        };
        let importer = CatalogImporter::new(
            provider,
            RecordingStore::default(),
            ImportLimits {
                page_size: 2,
                max_pages: 3,
            },
        );

        let error = importer.run().await.unwrap_err();

        assert_eq!(error.safe_summary(), "sanitized provider failure");
        assert_eq!(
            *importer.store.failed.lock().unwrap(),
            Some((
                ImportCounts {
                    fetched: 2,
                    imported: 1,
                    skipped: 1,
                    failed: 0,
                },
                "sanitized provider failure".to_owned(),
            ))
        );
    }

    #[tokio::test]
    async fn source_mismatch_fails_before_upsert_and_records_terminal_counts() {
        let mut mismatched = station("foreign");
        mismatched.source = "foreign_source";
        let provider = ScriptedProvider {
            offsets: Mutex::new(Vec::new()),
            pages: Mutex::new(vec![ImportPage {
                fetched: 1,
                stations: vec![mismatched],
                skipped: 0,
            }]),
            failure_after_pages: None,
        };
        let importer = CatalogImporter::new(
            provider,
            RecordingStore::default(),
            ImportLimits {
                page_size: 2,
                max_pages: 1,
            },
        );

        let error = importer.run().await.unwrap_err();

        assert_eq!(
            error.safe_summary(),
            "Catalog provider returned mismatched source ownership"
        );
        assert!(importer.store.batches.lock().unwrap().is_empty());
        assert_eq!(*importer.store.completed.lock().unwrap(), None);
        assert_eq!(
            *importer.store.failed.lock().unwrap(),
            Some((
                ImportCounts {
                    fetched: 1,
                    imported: 0,
                    skipped: 0,
                    failed: 1,
                },
                "Catalog provider returned mismatched source ownership".to_owned(),
            ))
        );
    }

    fn station(id: &str) -> ImportedStation {
        ImportedStation {
            source: "test",
            source_station_id: id.to_owned(),
            id: format!("test-{id}"),
            name: format!("Station {id}"),
            homepage_url: None,
            tags: vec!["rock".to_owned()],
            language: Some("en".to_owned()),
            country_code: Some("US".to_owned()),
            streams: vec![ImportedStream {
                source_stream_id: id.to_owned(),
                stream_url: format!("https://streams.example.com/{id}"),
                codec: Some("MP3".to_owned()),
                bitrate_kbps: Some(128),
                is_primary: true,
            }],
        }
    }
}

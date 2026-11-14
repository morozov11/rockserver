//! Batched fill of one versioned embedding table through `EmbedStations`.

use crate::persistence::{EmbeddingVersionStore, MAX_BATCH_STATIONS, WrittenStationEmbedding};
use crate::search::{EmbeddingProviderError, EmbeddingStoreError};

use super::traits::EmbedderService;

/// Counts from one table fill run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VersionBackfillStats {
    /// Station batches read as pending.
    pub processed: usize,
    /// Vectors actually persisted (hash race losses excluded).
    pub written: usize,
}

/// Failures of a fill run; both variants are safe for logs.
#[derive(Debug)]
pub enum VersionBackfillError {
    /// The embedding service failed for a batch.
    Provider(EmbeddingProviderError),
    /// Persistence failed for a batch.
    Store(EmbeddingStoreError),
    /// The fill made no progress for several batches (data race loop).
    Stalled,
}

impl std::fmt::Display for VersionBackfillError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(error) => write!(formatter, "embedding service failed: {error}"),
            Self::Store(error) => write!(formatter, "embedding storage failed: {error}"),
            Self::Stalled => {
                formatter.write_str("embedding backfill made no progress; rerun it later")
            }
        }
    }
}

impl std::error::Error for VersionBackfillError {}

impl From<EmbeddingProviderError> for VersionBackfillError {
    fn from(error: EmbeddingProviderError) -> Self {
        Self::Provider(error)
    }
}

impl From<EmbeddingStoreError> for VersionBackfillError {
    fn from(error: EmbeddingStoreError) -> Self {
        Self::Store(error)
    }
}

/// Consecutive fully-lost batches tolerated before giving up this run.
const MAX_STALLED_BATCHES: usize = 3;

/// Fills a versioned table with vectors for all pending stations.
///
/// Batches are capped at the contract limit; each batch does one RPC outside
/// any transaction and then one short guarded write transaction (no RPC
/// inside a transaction). Stations whose fields changed concurrently lose
/// their write and stay pending for the next run; a run gives up after
/// [`MAX_STALLED_BATCHES`] fully-lost batches instead of spinning.
pub async fn fill_version<S: EmbedderService>(
    service: &S,
    store: &mut EmbeddingVersionStore,
    version: u32,
    batch_size: usize,
) -> Result<VersionBackfillStats, VersionBackfillError> {
    let batch_size = batch_size.clamp(1, MAX_BATCH_STATIONS);
    let mut stats = VersionBackfillStats::default();
    let mut stalled = 0usize;
    loop {
        let pending = store.pending_stations(version, batch_size).await?;
        if pending.is_empty() {
            return Ok(stats);
        }
        let embeddings = service.embed_stations(&pending).await?;
        let batch = pending
            .iter()
            .zip(&embeddings)
            .map(|(station, embedding)| WrittenStationEmbedding {
                station_id: station.station_id.clone(),
                fields_hash: station.fields_hash.clone(),
                values: embedding.values().to_vec(),
            })
            .collect::<Vec<_>>();
        let written = store.write_embeddings(version, &batch).await?;
        stats.processed += pending.len();
        stats.written += written;
        tracing::debug!(
            version,
            pending = pending.len(),
            written,
            "embedding backfill batch completed"
        );
        if written == 0 {
            stalled += 1;
            if stalled >= MAX_STALLED_BATCHES {
                tracing::warn!(
                    version,
                    stalled_batches = stalled,
                    "embedding backfill batches keep losing the import race; stopping this run"
                );
                return Err(VersionBackfillError::Stalled);
            }
        } else {
            stalled = 0;
        }
    }
}

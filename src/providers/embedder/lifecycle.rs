//! Startup lifecycle of the versioned embedding table.

use std::{sync::Arc, time::Duration};

use sqlx::{Connection, PgConnection};

use crate::persistence::{
    EMBEDDINGS_DIMENSION, EMBEDDINGS_MODEL, EMBEDDINGS_VERSION, EmbeddingVersionStore,
    MAX_BATCH_STATIONS,
};
use crate::search::{EmbeddingProvenance, EmbeddingProviderError};

use super::backfill::{VersionBackfillStats, fill_version};
use super::provider::{EmbedderGateway, ServiceEmbeddingProvider};
use super::traits::EmbedderService;

/// Initial retry delay after a failed lifecycle attempt.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_secs(1);
/// Retry delay ceiling; the embedder may come up long after this process.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(30);

/// Safe lifecycle failure summary.
#[derive(Debug)]
struct LifecycleError(String);

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<EmbeddingProviderError> for LifecycleError {
    fn from(error: EmbeddingProviderError) -> Self {
        Self(error.to_string())
    }
}

impl From<crate::search::EmbeddingStoreError> for LifecycleError {
    fn from(error: crate::search::EmbeddingStoreError) -> Self {
        Self(error.to_string())
    }
}

/// Brings the versioned embedding table up after the HTTP server starts.
///
/// Readiness never depends on this task. The sequence per attempt: `Info`
/// (verified against this build's expected model and dimension) → advisory
/// lock on a dedicated connection (released when the connection breaks) →
/// `ensure_table` with schema verification → enable the vector branch →
/// backfill pending stations in batches. Any failure keeps the no-vectors
/// mode, is logged, and retried with capped exponential backoff; a completed
/// run ends the task (fills also happen manually via `backfill_embeddings`).
pub struct EmbedderLifecycle<S> {
    service: Arc<S>,
    gateway: Arc<EmbedderGateway>,
    database_url: String,
}

impl<S> EmbedderLifecycle<S>
where
    S: EmbedderService + 'static,
{
    /// Creates the lifecycle for one embedder service and gateway pair.
    pub fn new(
        service: Arc<S>,
        gateway: Arc<EmbedderGateway>,
        database_url: impl Into<String>,
    ) -> Self {
        Self {
            service,
            gateway,
            database_url: database_url.into(),
        }
    }

    /// Runs the lifecycle forever until one attempt fully succeeds.
    pub async fn run(self) {
        let mut backoff = INITIAL_RETRY_BACKOFF;
        loop {
            match self.run_once().await {
                Ok(stats) => {
                    tracing::info!(
                        version = EMBEDDINGS_VERSION,
                        processed = stats.processed,
                        written = stats.written,
                        "embedding lifecycle completed"
                    );
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        backoff_ms = backoff.as_millis() as u64,
                        "embedding lifecycle attempt failed; vector branch stays degraded"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_RETRY_BACKOFF);
                }
            }
        }
    }

    /// One full lifecycle attempt; the vector branch turns on only after the
    /// table schema was verified, before waiting for full coverage.
    async fn run_once(&self) -> Result<VersionBackfillStats, LifecycleError> {
        let provenance = self.verify_info().await?;
        let connection = PgConnection::connect(&self.database_url)
            .await
            .map_err(|_| LifecycleError("embedding lifecycle database connection failed".into()))?;
        let mut store = EmbeddingVersionStore::from_connection(connection);
        store
            .acquire_advisory_lock(EMBEDDINGS_VERSION)
            .await
            .map_err(|error| LifecycleError(error.to_string()))?;
        store
            .ensure_table(EMBEDDINGS_VERSION, provenance.dimension)
            .await
            .map_err(|error| LifecycleError(error.to_string()))?;

        // Enable immediately (without full coverage): stations without a
        // fresh vector simply stay on the lexical path until the fill lands.
        self.gateway
            .enable(Arc::new(ServiceEmbeddingProvider::new(Arc::clone(
                &self.service,
            ))));
        tracing::info!(
            model = %provenance.model,
            vector_version = %provenance.version,
            dimension = provenance.dimension,
            "embedding vector branch enabled"
        );

        let stats = fill_version(
            self.service.as_ref(),
            &mut store,
            EMBEDDINGS_VERSION,
            MAX_BATCH_STATIONS,
        )
        .await
        .map_err(|error| LifecycleError(error.to_string()))?;
        let (active, covered) = store
            .coverage(EMBEDDINGS_VERSION)
            .await
            .map_err(|error| LifecycleError(error.to_string()))?;
        tracing::info!(
            version = EMBEDDINGS_VERSION,
            active,
            covered,
            "embedding table coverage after lifecycle fill"
        );
        Ok(stats)
    }

    /// Calls `Info` and checks it matches this build's versioned table.
    async fn verify_info(&self) -> Result<EmbeddingProvenance, LifecycleError> {
        let provenance = self.service.info().await?;
        if provenance.model != EMBEDDINGS_MODEL || provenance.dimension != EMBEDDINGS_DIMENSION {
            return Err(LifecycleError(
                "embedding service provenance does not match this build's table version".into(),
            ));
        }
        Ok(provenance)
    }
}

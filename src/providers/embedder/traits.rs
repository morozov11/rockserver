//! Boundary between embedder consumers and the embedding service client.

use async_trait::async_trait;

use crate::persistence::PendingStationEmbedding;
use crate::search::{Embedding, EmbeddingProvenance, EmbeddingProviderError};

/// Calls the embedding service (`embed.v1`) for search, backfill, and the
/// startup lifecycle. Implementations own transport concerns (deadlines,
/// authentication, breaker); the concrete gRPC client is
/// [`super::EmbedderClient`], tests use deterministic fakes.
#[async_trait]
pub trait EmbedderService: Send + Sync {
    /// Provenance verified by the last successful `Info` call, if any.
    fn provenance(&self) -> Option<EmbeddingProvenance>;

    /// Asks `Info` and verifies the API version before use.
    async fn info(&self) -> Result<EmbeddingProvenance, EmbeddingProviderError>;

    /// Embeds one raw search query; the service applies the model prefix.
    async fn embed_query(&self, text: &str) -> Result<Embedding, EmbeddingProviderError>;

    /// Embeds station field sets in one batch (1..=64 items); the service
    /// builds the station text and returns one vector per input, in order.
    async fn embed_stations(
        &self,
        batch: &[PendingStationEmbedding],
    ) -> Result<Vec<Embedding>, EmbeddingProviderError>;
}

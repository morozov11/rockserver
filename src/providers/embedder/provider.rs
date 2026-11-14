//! Search-path adapter: a switchable `EmbeddingProvider` over the service.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::search::{Embedding, EmbeddingProvenance, EmbeddingProvider, EmbeddingProviderError};

use super::traits::EmbedderService;

/// `EmbeddingProvider` view of the embedding service that starts disabled.
///
/// The process starts in the no-vectors mode; the startup lifecycle enables
/// the vector branch only after `Info` answered and the versioned table
/// schema was verified. Every search call before that degrades to the
/// metadata path through the provider's error, matching the existing
/// provider-failure fallback.
#[derive(Default)]
pub struct EmbedderGateway {
    inner: RwLock<Option<Arc<dyn EmbeddingProvider>>>,
}

impl EmbedderGateway {
    /// Creates a gateway with the vector branch off.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Turns the vector branch on for all subsequent searches.
    pub fn enable(&self, provider: Arc<dyn EmbeddingProvider>) {
        *self.inner.write().unwrap() = Some(provider);
    }

    /// Reports whether searches currently get query vectors.
    pub fn is_enabled(&self) -> bool {
        self.inner.read().unwrap().is_some()
    }

    fn current(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.inner.read().unwrap().clone()
    }
}

#[async_trait]
impl EmbeddingProvider for EmbedderGateway {
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        self.current().and_then(|provider| provider.provenance())
    }

    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        match self.current() {
            Some(provider) => provider.embed(text).await,
            None => Err(EmbeddingProviderError::safe(
                "embedding service is not ready; using metadata fallback",
            )),
        }
    }

    fn supports_semantic_intent_filters(&self) -> bool {
        self.current()
            .is_some_and(|provider| provider.supports_semantic_intent_filters())
    }
}

/// `EmbeddingProvider` adapter over any [`EmbedderService`] implementation.
///
/// Used by the lifecycle to enable the gateway with the very service it
/// verified; queries embed via `EmbedQuery` with the service-side prefix.
pub struct ServiceEmbeddingProvider<S> {
    service: Arc<S>,
}

impl<S> ServiceEmbeddingProvider<S> {
    /// Wraps a shared embedding service.
    pub fn new(service: Arc<S>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl<S> EmbeddingProvider for ServiceEmbeddingProvider<S>
where
    S: EmbedderService + 'static,
{
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        self.service.provenance()
    }

    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.service.embed_query(text).await
    }

    fn supports_semantic_intent_filters(&self) -> bool {
        // A deployed neural model produces meaningful similarity for
        // diagnostics; classifiers load lazily after the lifecycle enables it.
        true
    }
}

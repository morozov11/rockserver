//! Adapter connecting the LLM service's `Languages` RPC to language intersection.

use std::sync::Arc;

use async_trait::async_trait;

use crate::languages::{ServiceLanguageSource, ServiceLanguageStatus};

use super::traits::LlmService;

/// `ServiceLanguageSource` backed by the neural LLM service client.
#[derive(Clone)]
pub struct LlmLanguageSource {
    service: Option<Arc<dyn LlmService>>,
}

impl LlmLanguageSource {
    /// Creates a source backed by an active LLM service.
    pub fn new(service: Option<Arc<dyn LlmService>>) -> Self {
        Self { service }
    }

    /// Creates an unconnected source for when the LLM service is disabled.
    pub fn unconnected() -> Self {
        Self { service: None }
    }
}

#[async_trait]
impl ServiceLanguageSource for LlmLanguageSource {
    async fn poll_languages(&self) -> ServiceLanguageStatus {
        let Some(service) = &self.service else {
            return ServiceLanguageStatus::NotConnected;
        };
        match service.languages().await {
            Ok(langs) => ServiceLanguageStatus::Available(langs.into_iter().collect()),
            Err(err) => {
                tracing::warn!(%err, "LLM service languages poll failed; status set to Unknown");
                ServiceLanguageStatus::Unknown
            }
        }
    }
}

//! External service language discovery and intersection rules.

use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::registry::LanguageRegistry;

/// Status of an external neural service's supported communication languages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceLanguageStatus {
    /// Service is not configured or not yet connected (stages before E6/E7).
    NotConnected,
    /// Service responded with its loaded valid language codes.
    Available(BTreeSet<String>),
    /// Service failed to respond or returned an error; languages are unknown.
    Unknown,
}

/// Abstract provider for querying an external service's loaded languages.
#[async_trait]
pub trait ServiceLanguageSource: Send + Sync {
    /// Polls the external service for its supported language codes.
    async fn poll_languages(&self) -> ServiceLanguageStatus;
}

/// Default stub source for services that are not yet connected before E6/E7.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnconnectedServiceLanguageSource;

#[async_trait]
impl ServiceLanguageSource for UnconnectedServiceLanguageSource {
    async fn poll_languages(&self) -> ServiceLanguageStatus {
        ServiceLanguageStatus::NotConnected
    }
}

/// Deterministic fake language source for unit and integration testing.
#[derive(Clone, Debug)]
pub struct FakeServiceLanguageSource {
    status: Arc<RwLock<ServiceLanguageStatus>>,
}

impl FakeServiceLanguageSource {
    /// Creates a fake source with an initial status.
    pub fn new(status: ServiceLanguageStatus) -> Self {
        Self {
            status: Arc::new(RwLock::new(status)),
        }
    }

    /// Sets the status returned by future polls.
    pub async fn set_status(&self, new_status: ServiceLanguageStatus) {
        *self.status.write().await = new_status;
    }
}

#[async_trait]
impl ServiceLanguageSource for FakeServiceLanguageSource {
    async fn poll_languages(&self) -> ServiceLanguageStatus {
        self.status.read().await.clone()
    }
}

/// Summary of an available communication language returned by `GET /api/v1/languages`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanguageSummary {
    /// 2-3 letter lowercase language code.
    pub code: String,
    /// Localized display name of the language.
    pub name: String,
    /// Native display name of the language (synonym for name).
    pub display_name: String,
}

/// Service that coordinates language availability across rockserver and external neural services.
pub struct LanguageIntersectionService {
    registry: Arc<LanguageRegistry>,
    llm_source: Arc<dyn ServiceLanguageSource>,
    stt_source: Arc<dyn ServiceLanguageSource>,
    cached_statuses: RwLock<(ServiceLanguageStatus, ServiceLanguageStatus)>,
    retry_pause: Duration,
}

impl LanguageIntersectionService {
    /// Creates an intersection service with default unconnected stub sources.
    pub fn with_unconnected_sources(registry: Arc<LanguageRegistry>) -> Self {
        Self::new(
            registry,
            Arc::new(UnconnectedServiceLanguageSource),
            Arc::new(UnconnectedServiceLanguageSource),
        )
    }

    /// Creates a new intersection service with the given sources.
    pub fn new(
        registry: Arc<LanguageRegistry>,
        llm_source: Arc<dyn ServiceLanguageSource>,
        stt_source: Arc<dyn ServiceLanguageSource>,
    ) -> Self {
        Self::with_retry_pause(registry, llm_source, stt_source, Duration::from_secs(5))
    }

    /// Creates an intersection service with a specific retry pause.
    pub fn with_retry_pause(
        registry: Arc<LanguageRegistry>,
        llm_source: Arc<dyn ServiceLanguageSource>,
        stt_source: Arc<dyn ServiceLanguageSource>,
        retry_pause: Duration,
    ) -> Self {
        Self {
            registry,
            llm_source,
            stt_source,
            cached_statuses: RwLock::new((
                ServiceLanguageStatus::NotConnected,
                ServiceLanguageStatus::NotConnected,
            )),
            retry_pause,
        }
    }

    /// Polls both external services and updates cached statuses.
    pub async fn refresh(&self) {
        let (llm_status, stt_status) = tokio::join!(
            self.llm_source.poll_languages(),
            self.stt_source.poll_languages(),
        );

        if llm_status == ServiceLanguageStatus::Unknown {
            tracing::warn!("LLM service language status is Unknown; retrying with pause");
        }
        if stt_status == ServiceLanguageStatus::Unknown {
            tracing::warn!("STT service language status is Unknown; retrying with pause");
        }

        let mut lock = self.cached_statuses.write().await;
        *lock = (llm_status, stt_status);
    }

    /// Returns the retry pause duration when an unknown service is encountered.
    pub fn retry_pause(&self) -> Duration {
        self.retry_pause
    }

    /// Computes the published communication languages according to the contract intersection rules.
    pub async fn published_languages(&self) -> Vec<LanguageSummary> {
        let (llm_status, stt_status) = self.cached_statuses.read().await.clone();
        self.compute_intersection(&llm_status, &stt_status)
    }

    /// Synchronous intersection computation given current service statuses.
    pub fn compute_intersection(
        &self,
        llm_status: &ServiceLanguageStatus,
        stt_status: &ServiceLanguageStatus,
    ) -> Vec<LanguageSummary> {
        let rockserver_codes: BTreeSet<String> = self.registry.valid_codes().into_iter().collect();

        // Start with rockserver's valid modules
        let mut candidates = rockserver_codes.clone();

        // 1. LLM intersection
        match llm_status {
            ServiceLanguageStatus::NotConnected => {
                // Not connected yet: does not constrain candidates
            }
            ServiceLanguageStatus::Available(llm_langs) => {
                candidates = candidates.intersection(llm_langs).cloned().collect();
            }
            ServiceLanguageStatus::Unknown => {
                // Unknown: its languages (except en) are not published
                candidates.retain(|code| code == "en");
            }
        }

        // 2. STT intersection
        match stt_status {
            ServiceLanguageStatus::NotConnected => {
                // Not connected yet: does not constrain candidates
            }
            ServiceLanguageStatus::Available(stt_langs) => {
                candidates = candidates.intersection(stt_langs).cloned().collect();
            }
            ServiceLanguageStatus::Unknown => {
                // Unknown: its languages (except en) are not published
                candidates.retain(|code| code == "en");
            }
        }

        // 3. Contract rule: 'en', verified in rockserver, is always published
        if rockserver_codes.contains("en") {
            candidates.insert("en".to_owned());
        }

        // Map candidate codes to LanguageSummary sorted alphabetically
        let mut result = Vec::new();
        for code in candidates {
            if let Some(module) = self.registry.get(&code) {
                result.push(LanguageSummary {
                    code: module.code.clone(),
                    name: module.display_name.clone(),
                    display_name: module.display_name.clone(),
                });
            }
        }

        result.sort_by(|a, b| a.code.cmp(&b.code));
        result
    }

    /// Returns a set of currently published language codes.
    pub async fn published_language_codes(&self) -> HashSet<String> {
        self.published_languages()
            .await
            .into_iter()
            .map(|lang| lang.code)
            .collect()
    }

    /// Returns true if the language code is currently published.
    pub async fn is_published(&self, code: &str) -> bool {
        self.published_language_codes().await.contains(code)
    }
}

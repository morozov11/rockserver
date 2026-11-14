//! Environment configuration of the embedding-service gRPC client.

use std::time::Duration;

use crate::search::EmbeddingProviderError;

/// Environment variable holding the embedding service `host:port` address.
/// When it is absent, the process runs without query vectors (not an error).
pub const EMBEDDER_ADDR_ENV: &str = "ROCKSERVER_EMBEDDER_ADDR";
/// Environment variable holding the shared bearer token for the service.
pub const EMBEDDER_TOKEN_ENV: &str = "ROCKSERVER_EMBEDDER_TOKEN";
/// Optional per-call budget overrides in milliseconds.
pub const EMBEDDER_QUERY_TIMEOUT_MS_ENV: &str = "ROCKSERVER_EMBEDDER_QUERY_TIMEOUT_MS";
pub const EMBEDDER_BATCH_TIMEOUT_MS_ENV: &str = "ROCKSERVER_EMBEDDER_BATCH_TIMEOUT_MS";
pub const EMBEDDER_INFO_TIMEOUT_MS_ENV: &str = "ROCKSERVER_EMBEDDER_INFO_TIMEOUT_MS";

/// Contract deadline for one query vector (service contract, budgets).
const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_millis(300);
/// One `EmbedStations` batch is a single model pass; 64 stations may take a while.
const DEFAULT_BATCH_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_INFO_TIMEOUT: Duration = Duration::from_secs(5);

/// Validated connection settings for the embedding service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbedderConfig {
    address: String,
    token: String,
    query_timeout: Duration,
    batch_timeout: Duration,
    info_timeout: Duration,
}

impl EmbedderConfig {
    /// Assembles the configuration from explicit values (tests, tools).
    pub fn new(
        address: impl Into<String>,
        token: impl Into<String>,
        query_timeout: Duration,
        batch_timeout: Duration,
        info_timeout: Duration,
    ) -> Self {
        Self {
            address: address.into(),
            token: token.into(),
            query_timeout,
            batch_timeout,
            info_timeout,
        }
    }

    /// Reads the configuration; `Ok(None)` means the no-vectors mode.
    ///
    /// A present address with a missing token is a startup error: the service
    /// rejects unauthenticated calls, so running would only produce noise.
    pub fn from_env() -> Result<Option<Self>, EmbeddingProviderError> {
        let Some(address) = non_empty_env(EMBEDDER_ADDR_ENV)? else {
            return Ok(None);
        };
        let Some(token) = non_empty_env(EMBEDDER_TOKEN_ENV)? else {
            return Err(EmbeddingProviderError::safe(format!(
                "{EMBEDDER_TOKEN_ENV} is required when {EMBEDDER_ADDR_ENV} is set"
            )));
        };
        Ok(Some(Self {
            address,
            token,
            query_timeout: duration_env(EMBEDDER_QUERY_TIMEOUT_MS_ENV, DEFAULT_QUERY_TIMEOUT)?,
            batch_timeout: duration_env(EMBEDDER_BATCH_TIMEOUT_MS_ENV, DEFAULT_BATCH_TIMEOUT)?,
            info_timeout: duration_env(EMBEDDER_INFO_TIMEOUT_MS_ENV, DEFAULT_INFO_TIMEOUT)?,
        }))
    }

    /// Target `host:port` of the embedding service.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Shared bearer token sent in `authorization` metadata.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Deadline for one `EmbedQuery` call.
    pub fn query_timeout(&self) -> Duration {
        self.query_timeout
    }

    /// Deadline for one `EmbedStations` batch call.
    pub fn batch_timeout(&self) -> Duration {
        self.batch_timeout
    }

    /// Deadline for one `Info` call.
    pub fn info_timeout(&self) -> Duration {
        self.info_timeout
    }
}

fn non_empty_env(name: &str) -> Result<Option<String>, EmbeddingProviderError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value.trim().to_owned())),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(EmbeddingProviderError::safe(format!(
            "{name} must contain valid Unicode"
        ))),
    }
}

fn duration_env(name: &str, default: Duration) -> Result<Duration, EmbeddingProviderError> {
    let Some(raw) = non_empty_env(name)? else {
        return Ok(default);
    };
    let millis = raw
        .parse::<u64>()
        .map_err(|_| EmbeddingProviderError::safe(format!("{name} must be a millisecond count")))?;
    if millis == 0 {
        return Err(EmbeddingProviderError::safe(format!(
            "{name} must be a positive millisecond count"
        )));
    }
    Ok(Duration::from_millis(millis))
}

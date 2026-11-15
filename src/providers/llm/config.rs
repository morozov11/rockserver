//! Environment configuration of the LLM-service gRPC client.

use std::time::Duration;

use super::error::LlmClientError;

/// Environment variable holding the LLM service `host:port` address.
/// When it is absent, the process runs with deterministic intent parsing (not an error).
pub const LLM_ADDR_ENV: &str = "ROCKSERVER_LLM_ADDR";
/// Environment variable holding the shared bearer token for the service.
pub const LLM_TOKEN_ENV: &str = "ROCKSERVER_LLM_TOKEN";
/// Optional per-call budget overrides in milliseconds.
pub const LLM_INTENT_TIMEOUT_MS_ENV: &str = "ROCKSERVER_LLM_INTENT_TIMEOUT_MS";
pub const LLM_COMMAND_TIMEOUT_MS_ENV: &str = "ROCKSERVER_LLM_COMMAND_TIMEOUT_MS";
pub const LLM_INFO_TIMEOUT_MS_ENV: &str = "ROCKSERVER_LLM_INFO_TIMEOUT_MS";

/// Contract default deadline for one `ParseIntent` query call (5 seconds).
const DEFAULT_INTENT_TIMEOUT: Duration = Duration::from_millis(5000);
/// Contract default deadline for one `ParseCommand` call (4.5 seconds).
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_millis(4500);
/// Contract default deadline for one `Languages` or health call (5 seconds).
const DEFAULT_INFO_TIMEOUT: Duration = Duration::from_secs(5);

/// Validated connection settings for the neural LLM service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlmConfig {
    address: String,
    token: String,
    intent_timeout: Duration,
    command_timeout: Duration,
    info_timeout: Duration,
}

impl LlmConfig {
    /// Assembles the configuration from explicit values (tests, tools).
    pub fn new(
        address: impl Into<String>,
        token: impl Into<String>,
        intent_timeout: Duration,
        command_timeout: Duration,
        info_timeout: Duration,
    ) -> Self {
        Self {
            address: address.into(),
            token: token.into(),
            intent_timeout,
            command_timeout,
            info_timeout,
        }
    }

    /// Reads the configuration; `Ok(None)` means the deterministic parser fallback mode.
    ///
    /// A present address with a missing token is a startup error: the service
    /// rejects unauthenticated calls, so running would only produce noise.
    pub fn from_env() -> Result<Option<Self>, LlmClientError> {
        let Some(address) = non_empty_env(LLM_ADDR_ENV)? else {
            return Ok(None);
        };
        let Some(token) = non_empty_env(LLM_TOKEN_ENV)? else {
            return Err(LlmClientError::safe(format!(
                "{LLM_TOKEN_ENV} is required when {LLM_ADDR_ENV} is set"
            )));
        };
        Ok(Some(Self {
            address,
            token,
            intent_timeout: duration_env(LLM_INTENT_TIMEOUT_MS_ENV, DEFAULT_INTENT_TIMEOUT)?,
            command_timeout: duration_env(LLM_COMMAND_TIMEOUT_MS_ENV, DEFAULT_COMMAND_TIMEOUT)?,
            info_timeout: duration_env(LLM_INFO_TIMEOUT_MS_ENV, DEFAULT_INFO_TIMEOUT)?,
        }))
    }

    /// Target `host:port` of the LLM service.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Shared bearer token sent in `authorization` metadata.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Deadline for one `ParseIntent` call.
    pub fn intent_timeout(&self) -> Duration {
        self.intent_timeout
    }

    /// Deadline for one `ParseCommand` call.
    pub fn command_timeout(&self) -> Duration {
        self.command_timeout
    }

    /// Deadline for one `Languages` or info call.
    pub fn info_timeout(&self) -> Duration {
        self.info_timeout
    }
}

fn non_empty_env(name: &str) -> Result<Option<String>, LlmClientError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value.trim().to_owned())),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(LlmClientError::safe(format!(
            "{name} must contain valid Unicode"
        ))),
    }
}

fn duration_env(name: &str, default: Duration) -> Result<Duration, LlmClientError> {
    let Some(raw) = non_empty_env(name)? else {
        return Ok(default);
    };
    let millis = raw
        .parse::<u64>()
        .map_err(|_| LlmClientError::safe(format!("{name} must be a millisecond count")))?;
    if millis == 0 {
        return Err(LlmClientError::safe(format!(
            "{name} must be a positive millisecond count"
        )));
    }
    Ok(Duration::from_millis(millis))
}

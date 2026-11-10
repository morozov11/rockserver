//! Yandex AI Studio implementation of the catalog-free `LlmProvider` boundary.

use std::{env, error::Error, fmt, time::Duration};

use async_trait::async_trait;
use reqwest::{Url, header};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::search::{LlmProvider, LlmProviderError, LlmRequest, MAX_LLM_INTENT_JSON_BYTES};

/// Secret API-key environment variable required to enable Yandex AI Studio parsing.
pub const API_KEY_ENV: &str = "YANDEX_AI_API_KEY";
/// Folder-ID environment variable required to enable Yandex AI Studio parsing.
pub const FOLDER_ID_ENV: &str = "YANDEX_FOLDER_ID";
/// Optional Yandex model identifier, without folder or version.
pub const MODEL_ENV: &str = "YANDEX_LLM_MODEL";
/// Optional fixed Yandex model version; defaults to `latest`.
pub const MODEL_VERSION_ENV: &str = "YANDEX_LLM_MODEL_VERSION";
/// Optional whole-request timeout in milliseconds.
pub const TIMEOUT_MS_ENV: &str = "YANDEX_LLM_TIMEOUT_MS";

/// Official Yandex AI Studio synchronous text-generation endpoint.
pub const DEFAULT_ENDPOINT: &str =
    "https://llm.api.cloud.yandex.net/foundationModels/v1/completion";
/// Default model identifier used with the configured folder ID.
pub const DEFAULT_MODEL: &str = "yandexgpt-lite";
/// Default whole-request timeout for one bounded intent parse.
pub const DEFAULT_TIMEOUT_MS: u64 = 4_500;

const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 10_000;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_COMPLETION_TOKENS: u16 = 256;

/// Validated, secret-safe Yandex AI Studio settings.
#[derive(Clone)]
pub struct YandexLlmConfig {
    api_key: String,
    folder_id: String,
    model: String,
    model_version: String,
    timeout: Duration,
    endpoint: Url,
}

impl fmt::Debug for YandexLlmConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("YandexLlmConfig")
            .field("api_key", &"[REDACTED]")
            .field("folder_id", &"[REDACTED]")
            .field("model", &self.model)
            .field("model_version", &self.model_version)
            .field("timeout", &self.timeout)
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

impl YandexLlmConfig {
    /// Selects a Yandex configuration only when both required environment values are present.
    ///
    /// When neither required value is set, startup keeps the deterministic parser. A partial
    /// configuration is rejected without including the value that was supplied.
    pub fn optional_from_env() -> Result<Option<Self>, YandexLlmConfigError> {
        let api_key = read_env(API_KEY_ENV, YandexLlmConfigError::InvalidApiKey)?;
        let folder_id = read_env(FOLDER_ID_ENV, YandexLlmConfigError::InvalidFolderId)?;
        let model = read_env(MODEL_ENV, YandexLlmConfigError::InvalidModel)?;
        let model_version = read_env(MODEL_VERSION_ENV, YandexLlmConfigError::InvalidModelVersion)?;
        let timeout = read_env(TIMEOUT_MS_ENV, YandexLlmConfigError::InvalidTimeout)?;
        Self::from_lookup(|name| match name {
            API_KEY_ENV => api_key.clone(),
            FOLDER_ID_ENV => folder_id.clone(),
            MODEL_ENV => model.clone(),
            MODEL_VERSION_ENV => model_version.clone(),
            TIMEOUT_MS_ENV => timeout.clone(),
            _ => None,
        })
    }

    // A lookup boundary keeps configuration tests deterministic and avoids process-global env.
    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Option<Self>, YandexLlmConfigError> {
        let api_key = lookup(API_KEY_ENV).filter(|value| !value.trim().is_empty());
        let folder_id = lookup(FOLDER_ID_ENV).filter(|value| !value.trim().is_empty());
        let (api_key, folder_id) = match (api_key, folder_id) {
            (None, None) => return Ok(None),
            (Some(api_key), Some(folder_id)) => (api_key, folder_id),
            _ => return Err(YandexLlmConfigError::PartialConfiguration),
        };
        if header::HeaderValue::from_str(&format!("Api-Key {api_key}")).is_err() {
            return Err(YandexLlmConfigError::InvalidApiKey);
        }
        if !is_identifier(&folder_id) {
            return Err(YandexLlmConfigError::InvalidFolderId);
        }
        let model = lookup(MODEL_ENV)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        if !is_identifier(&model) {
            return Err(YandexLlmConfigError::InvalidModel);
        }
        let model_version = lookup(MODEL_VERSION_ENV)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "latest".to_owned());
        if !is_identifier(&model_version) {
            return Err(YandexLlmConfigError::InvalidModelVersion);
        }
        let timeout_ms = parse_timeout(lookup(TIMEOUT_MS_ENV))?;
        let endpoint = Url::parse(DEFAULT_ENDPOINT).expect("official Yandex endpoint is valid");
        Ok(Some(Self {
            api_key,
            folder_id,
            model,
            model_version,
            timeout: Duration::from_millis(timeout_ms),
            endpoint,
        }))
    }

    fn model_uri(&self) -> String {
        format!(
            "gpt://{}/{}/{}",
            self.folder_id, self.model, self.model_version
        )
    }
}

fn read_env(
    name: &str,
    invalid_error: YandexLlmConfigError,
) -> Result<Option<String>, YandexLlmConfigError> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(invalid_error),
    }
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn parse_timeout(value: Option<String>) -> Result<u64, YandexLlmConfigError> {
    let timeout = match value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| YandexLlmConfigError::InvalidTimeout)?,
        None => DEFAULT_TIMEOUT_MS,
    };
    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&timeout) {
        return Err(YandexLlmConfigError::InvalidTimeout);
    }
    Ok(timeout)
}

/// Safe startup error for Yandex parser settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum YandexLlmConfigError {
    /// Only one required Yandex setting was present.
    PartialConfiguration,
    /// The API key cannot be represented safely in the authorization header.
    InvalidApiKey,
    /// The folder ID does not have the supported identifier form.
    InvalidFolderId,
    /// The optional model identifier does not have the supported form.
    InvalidModel,
    /// The optional version identifier does not have the supported form.
    InvalidModelVersion,
    /// The optional timeout is outside the allowed range or not an integer.
    InvalidTimeout,
}

impl fmt::Display for YandexLlmConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartialConfiguration => write!(
                formatter,
                "{API_KEY_ENV} and {FOLDER_ID_ENV} must both be set to enable Yandex LLM parsing"
            ),
            Self::InvalidApiKey => write!(
                formatter,
                "{API_KEY_ENV} is not a valid API-key header value"
            ),
            Self::InvalidFolderId => {
                write!(formatter, "{FOLDER_ID_ENV} must be a valid identifier")
            }
            Self::InvalidModel => write!(formatter, "{MODEL_ENV} must be a valid model identifier"),
            Self::InvalidModelVersion => write!(
                formatter,
                "{MODEL_VERSION_ENV} must be a valid model version"
            ),
            Self::InvalidTimeout => write!(
                formatter,
                "{TIMEOUT_MS_ENV} must be an integer from {MIN_TIMEOUT_MS} to {MAX_TIMEOUT_MS}"
            ),
        }
    }
}

impl Error for YandexLlmConfigError {}

/// Synchronous Yandex AI Studio client for a single structured intent completion.
#[derive(Clone, Debug)]
pub struct YandexLlmProvider {
    config: YandexLlmConfig,
    client: reqwest::Client,
}

impl YandexLlmProvider {
    /// Loads explicit Yandex settings and constructs the bounded HTTPS client.
    pub fn optional_from_env() -> Result<Option<Self>, YandexLlmConfigError> {
        YandexLlmConfig::optional_from_env()?
            .map(Self::new)
            .transpose()
    }

    /// Creates a provider from already validated settings.
    pub fn new(config: YandexLlmConfig) -> Result<Self, YandexLlmConfigError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|_| YandexLlmConfigError::InvalidTimeout)?;
        Ok(Self { config, client })
    }

    /// Returns a diagnostic request body with identity and user text redacted.
    pub fn safe_request_body(&self, request: &LlmRequest) -> Value {
        let mut body = self.request_body(request, true);
        body["messages"][1]["text"] = Value::String("[REDACTED]".to_owned());
        body
    }

    /// Returns the non-secret official endpoint used by this provider.
    pub fn endpoint(&self) -> &Url {
        &self.config.endpoint
    }

    fn request_body(&self, request: &LlmRequest, redact_folder: bool) -> Value {
        let model_uri = if redact_folder {
            format!(
                "gpt://[REDACTED]/{}/{}",
                self.config.model, self.config.model_version
            )
        } else {
            self.config.model_uri()
        };
        json!({
            "modelUri": model_uri,
            "completionOptions": {
                "stream": false,
                "temperature": 0.0,
                "maxTokens": MAX_COMPLETION_TOKENS.to_string(),
                "reasoningOptions": {"mode": "DISABLED"}
            },
            "messages": [
                {"role": "system", "text": request.system_instruction()},
                {"role": "user", "text": json!({"command": request.command(), "locale": request.locale()}).to_string()}
            ],
            "jsonSchema": {"schema": request.response_schema()}
        })
    }
}

#[async_trait]
impl LlmProvider for YandexLlmProvider {
    async fn generate_json(&self, request: &LlmRequest) -> Result<String, LlmProviderError> {
        let body = self.request_body(request, false);
        tracing::debug!(method = "POST", "structured intent request started");
        let response = self
            .client
            .post(self.config.endpoint.clone())
            .header(
                header::AUTHORIZATION,
                format!("Api-Key {}", self.config.api_key),
            )
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    LlmProviderError::transient("structured intent request timed out")
                } else {
                    LlmProviderError::transient("structured intent request failed")
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            tracing::debug!(status = status.as_u16(), "structured intent request failed");
            let error = format!(
                "structured intent service returned HTTP {}",
                status.as_u16()
            );
            return Err(if status.as_u16() == 429 || status.is_server_error() {
                LlmProviderError::transient(error)
            } else {
                LlmProviderError::safe(error)
            });
        }
        let bytes = read_bounded(response).await?;
        tracing::debug!(
            status = status.as_u16(),
            "structured intent request completed"
        );
        let envelope = serde_json::from_slice::<CompletionEnvelope>(&bytes).map_err(|_| {
            LlmProviderError::safe("structured intent service returned a malformed response")
        })?;
        let text = envelope
            .result
            .alternatives
            .into_iter()
            .next()
            .map(|alternative| alternative.message.text)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| {
                LlmProviderError::safe("structured intent service returned no completion text")
            })?;
        if text.len() > MAX_LLM_INTENT_JSON_BYTES {
            return Err(LlmProviderError::safe(
                "structured intent service returned an oversized intent response",
            ));
        }
        Ok(text)
    }
}

async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, LlmProviderError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PROVIDER_RESPONSE_BYTES as u64)
    {
        return Err(LlmProviderError::safe(
            "structured intent service returned an oversized response",
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        LlmProviderError::transient("structured intent service response read failed")
    })? {
        if body.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
            return Err(LlmProviderError::safe(
                "structured intent service returned an oversized response",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Deserialize)]
struct CompletionEnvelope {
    result: CompletionResult,
}

#[derive(Deserialize)]
struct CompletionResult {
    alternatives: Vec<CompletionAlternative>,
}

#[derive(Deserialize)]
struct CompletionAlternative {
    message: CompletionMessage,
}

#[derive(Deserialize)]
struct CompletionMessage {
    text: String,
}

#[cfg(test)]
#[path = "yandex_llm/tests.rs"]
mod tests;

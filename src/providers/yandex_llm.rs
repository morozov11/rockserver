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
/// Optional whole-request timeout in milliseconds.
pub const TIMEOUT_MS_ENV: &str = "YANDEX_LLM_TIMEOUT_MS";

/// Official Yandex AI Studio synchronous text-generation endpoint.
pub const DEFAULT_ENDPOINT: &str =
    "https://llm.api.cloud.yandex.net/foundationModels/v1/completion";
/// Default model identifier used with the configured folder ID.
pub const DEFAULT_MODEL: &str = "yandexgpt";
/// Default whole-request timeout for one bounded intent parse.
pub const DEFAULT_TIMEOUT_MS: u64 = 3_000;

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
        let timeout = read_env(TIMEOUT_MS_ENV, YandexLlmConfigError::InvalidTimeout)?;
        Self::from_lookup(|name| match name {
            API_KEY_ENV => api_key.clone(),
            FOLDER_ID_ENV => folder_id.clone(),
            MODEL_ENV => model.clone(),
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
        let model = lookup(MODEL_ENV).unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        if !is_identifier(&model) {
            return Err(YandexLlmConfigError::InvalidModel);
        }
        let timeout_ms = parse_timeout(lookup(TIMEOUT_MS_ENV))?;
        let endpoint = Url::parse(DEFAULT_ENDPOINT).expect("official Yandex endpoint is valid");
        Ok(Some(Self {
            api_key,
            folder_id,
            model,
            timeout: Duration::from_millis(timeout_ms),
            endpoint,
        }))
    }

    fn model_uri(&self) -> String {
        format!("gpt://{}/{}/latest", self.folder_id, self.model)
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
    let timeout = match value {
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

    /// Returns a diagnostic request body with the folder identifier redacted.
    pub fn safe_request_body(&self, request: &LlmRequest) -> Value {
        self.request_body(request, true)
    }

    /// Returns the non-secret official endpoint used by this provider.
    pub fn endpoint(&self) -> &Url {
        &self.config.endpoint
    }

    fn request_body(&self, request: &LlmRequest, redact_folder: bool) -> Value {
        let model_uri = if redact_folder {
            format!("gpt://[REDACTED]/{}/latest", self.config.model)
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
        tracing::debug!(
            method = "POST",
            endpoint = %self.config.endpoint,
            authorization = "Api-Key [REDACTED]",
            request_body = %self.safe_request_body(request),
            "Yandex LLM request"
        );
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
                    LlmProviderError::safe("Yandex LLM request timed out")
                } else {
                    LlmProviderError::safe("Yandex LLM request failed")
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            let bytes = read_bounded(response).await?;
            let safe_response = String::from_utf8_lossy(&bytes)
                .replace(&self.config.api_key, "[REDACTED]")
                .replace(&self.config.folder_id, "[REDACTED]");
            tracing::debug!(
                status = status.as_u16(),
                response_body = %safe_response,
                "Yandex LLM error response"
            );
            return Err(LlmProviderError::safe(format!(
                "Yandex LLM returned HTTP {}",
                status.as_u16()
            )));
        }
        let bytes = read_bounded(response).await?;
        let safe_response = String::from_utf8_lossy(&bytes)
            .replace(&self.config.api_key, "[REDACTED]")
            .replace(&self.config.folder_id, "[REDACTED]");
        tracing::debug!(
            status = status.as_u16(),
            response_body = %safe_response,
            "Yandex LLM response"
        );
        let envelope = serde_json::from_slice::<CompletionEnvelope>(&bytes)
            .map_err(|_| LlmProviderError::safe("Yandex LLM returned a malformed response"))?;
        let text = envelope
            .result
            .alternatives
            .into_iter()
            .next()
            .map(|alternative| alternative.message.text)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| LlmProviderError::safe("Yandex LLM returned no completion text"))?;
        if text.len() > MAX_LLM_INTENT_JSON_BYTES {
            return Err(LlmProviderError::safe(
                "Yandex LLM returned an oversized intent response",
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
            "Yandex LLM returned an oversized response",
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| LlmProviderError::safe("Yandex LLM response read failed"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
            return Err(LlmProviderError::safe(
                "Yandex LLM returned an oversized response",
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
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{
        Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
    };
    use serde_json::{Value, json};

    use super::{
        API_KEY_ENV, DEFAULT_MODEL, FOLDER_ID_ENV, LlmProvider, LlmRequest, MODEL_ENV,
        TIMEOUT_MS_ENV, YandexLlmConfig, YandexLlmConfigError, YandexLlmProvider,
    };
    use crate::search::{MAX_LLM_INTENT_JSON_BYTES, QueryParserInput};

    #[test]
    fn configuration_is_optional_strict_and_secret_safe() {
        assert!(YandexLlmConfig::from_lookup(|_| None).unwrap().is_none());
        assert_eq!(
            YandexLlmConfig::from_lookup(
                |name| (name == API_KEY_ENV).then(|| "secret-key".to_owned())
            )
            .unwrap_err(),
            YandexLlmConfigError::PartialConfiguration
        );
        let config = YandexLlmConfig::from_lookup(|name| match name {
            API_KEY_ENV => Some("secret-key".to_owned()),
            FOLDER_ID_ENV => Some("folder_123".to_owned()),
            _ => None,
        })
        .unwrap()
        .unwrap();
        assert!(format!("{config:?}").contains("[REDACTED]"));
        assert!(!format!("{config:?}").contains("secret-key"));
        assert_eq!(config.model, DEFAULT_MODEL);
        let error = YandexLlmConfig::from_lookup(|name| match name {
            API_KEY_ENV => Some("secret-key".to_owned()),
            FOLDER_ID_ENV => Some("folder_123".to_owned()),
            TIMEOUT_MS_ENV => Some("1".to_owned()),
            _ => None,
        })
        .unwrap_err();
        assert!(!error.to_string().contains("secret-key"));
    }

    #[tokio::test]
    async fn provider_sends_documented_auth_model_and_catalog_free_request() {
        let seen = Arc::new(Mutex::new(None));
        let (endpoint, server) = start_mock(
            StatusCode::OK,
            completion("{\"terms\":[\"jazz\"],\"tags\":[]}"),
            seen.clone(),
            None,
        )
        .await;
        let provider = provider(endpoint, 1_000);
        let output = provider
            .generate_json(&LlmRequest::radio_intent(&input()))
            .await
            .unwrap();
        server.abort();
        assert!(output.contains("jazz"));
        let request = seen.lock().unwrap().clone().unwrap();
        assert_eq!(
            request.authorization.as_deref(),
            Some("Api-Key test-api-key")
        );
        assert_eq!(
            request.body["modelUri"],
            "gpt://folder_123/yandexgpt/latest"
        );
        assert_eq!(request.body["completionOptions"]["stream"], false);
        assert_eq!(
            request.body["jsonSchema"]["schema"]["additionalProperties"],
            false
        );
        assert_eq!(request.body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(request.body["messages"][1]["role"], "user");
        assert_eq!(
            request.body["messages"][1]["text"],
            json!({"command":"calm jazz","locale":"en-US"}).to_string()
        );
        assert!(!request.body.to_string().contains("station-"));
    }

    #[tokio::test]
    async fn provider_sanitizes_http_timeout_malformed_and_oversized_failures() {
        let seen = Arc::new(Mutex::new(None));
        let (endpoint, server) = start_mock(
            StatusCode::BAD_GATEWAY,
            "upstream secret".to_owned(),
            seen.clone(),
            None,
        )
        .await;
        let error = provider(endpoint, 1_000)
            .generate_json(&LlmRequest::radio_intent(&input()))
            .await
            .unwrap_err();
        server.abort();
        assert_eq!(error.to_string(), "Yandex LLM returned HTTP 502");
        assert!(!error.to_string().contains("secret"));

        let (endpoint, server) =
            start_mock(StatusCode::OK, "not JSON".to_owned(), seen.clone(), None).await;
        let error = provider(endpoint, 1_000)
            .generate_json(&LlmRequest::radio_intent(&input()))
            .await
            .unwrap_err();
        server.abort();
        assert_eq!(
            error.to_string(),
            "Yandex LLM returned a malformed response"
        );

        let oversized = completion(&format!(
            "{{\"terms\":[\"{}\"],\"tags\":[]}}",
            "x".repeat(MAX_LLM_INTENT_JSON_BYTES + 1)
        ));
        let (endpoint, server) = start_mock(StatusCode::OK, oversized, seen.clone(), None).await;
        let error = provider(endpoint, 1_000)
            .generate_json(&LlmRequest::radio_intent(&input()))
            .await
            .unwrap_err();
        server.abort();
        assert!(error.to_string().contains("oversized"));

        let (endpoint, server) = start_mock(
            StatusCode::OK,
            completion("{\"terms\":[],\"tags\":[]}"),
            seen,
            Some(Duration::from_millis(100)),
        )
        .await;
        let error = provider(endpoint, 100)
            .generate_json(&LlmRequest::radio_intent(&input()))
            .await
            .unwrap_err();
        server.abort();
        assert_eq!(error.to_string(), "Yandex LLM request timed out");
    }

    fn input() -> QueryParserInput {
        QueryParserInput {
            query: "calm jazz".to_owned(),
            locale: "en-US".to_owned(),
        }
    }

    fn provider(endpoint: String, timeout_ms: u64) -> YandexLlmProvider {
        let mut config = YandexLlmConfig::from_lookup(|name| match name {
            API_KEY_ENV => Some("test-api-key".to_owned()),
            FOLDER_ID_ENV => Some("folder_123".to_owned()),
            MODEL_ENV => Some(DEFAULT_MODEL.to_owned()),
            TIMEOUT_MS_ENV => Some(timeout_ms.to_string()),
            _ => None,
        })
        .unwrap()
        .unwrap();
        config.endpoint = endpoint.parse().unwrap();
        YandexLlmProvider::new(config).unwrap()
    }

    fn completion(text: &str) -> String {
        json!({"result":{"alternatives":[{"message":{"text":text}}]}}).to_string()
    }

    #[derive(Clone)]
    struct MockState {
        status: StatusCode,
        body: String,
        seen: Arc<Mutex<Option<RecordedRequest>>>,
        delay: Option<Duration>,
    }

    #[derive(Clone, Debug)]
    struct RecordedRequest {
        authorization: Option<String>,
        body: Value,
    }

    async fn start_mock(
        status: StatusCode,
        body: String,
        seen: Arc<Mutex<Option<RecordedRequest>>>,
        delay: Option<Duration>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/", post(mock_handler))
            .with_state(MockState {
                status,
                body,
                seen,
                delay,
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_handler(
        State(state): State<MockState>,
        headers: HeaderMap,
        body: String,
    ) -> impl IntoResponse {
        *state.seen.lock().unwrap() = Some(RecordedRequest {
            authorization: headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            body: serde_json::from_str(&body).unwrap(),
        });
        if let Some(delay) = state.delay {
            tokio::time::sleep(delay).await;
        }
        (state.status, state.body)
    }
}

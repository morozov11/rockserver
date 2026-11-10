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
    API_KEY_ENV, DEFAULT_MODEL, DEFAULT_TIMEOUT_MS, FOLDER_ID_ENV, LlmProvider, LlmRequest,
    MODEL_ENV, MODEL_VERSION_ENV, TIMEOUT_MS_ENV, YandexLlmConfig, YandexLlmConfigError,
    YandexLlmProvider,
};
use crate::search::{MAX_LLM_INTENT_JSON_BYTES, QueryParserInput};

#[test]
fn configuration_is_optional_strict_and_secret_safe() {
    assert!(YandexLlmConfig::from_lookup(|_| None).unwrap().is_none());
    assert_eq!(
        YandexLlmConfig::from_lookup(|name| (name == API_KEY_ENV).then(|| "secret-key".to_owned()))
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
    assert_eq!(config.timeout, Duration::from_millis(DEFAULT_TIMEOUT_MS));

    let empty_override = YandexLlmConfig::from_lookup(|name| match name {
        API_KEY_ENV => Some("secret-key".to_owned()),
        FOLDER_ID_ENV => Some("folder_123".to_owned()),
        MODEL_ENV => Some("   ".to_owned()),
        TIMEOUT_MS_ENV => Some("".to_owned()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    assert_eq!(empty_override.model, DEFAULT_MODEL);
    assert_eq!(
        empty_override.timeout,
        Duration::from_millis(DEFAULT_TIMEOUT_MS)
    );

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
        "gpt://folder_123/yandexgpt-lite/latest"
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

#[test]
fn fixed_model_version_is_validated_without_exposing_configuration() {
    let config = YandexLlmConfig::from_lookup(|name| match name {
        API_KEY_ENV => Some("secret-key".into()),
        FOLDER_ID_ENV => Some("folder_123".into()),
        MODEL_VERSION_ENV => Some("rc-2026.11".into()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        config.model_uri(),
        "gpt://folder_123/yandexgpt-lite/rc-2026.11"
    );
    let error = YandexLlmConfig::from_lookup(|name| match name {
        API_KEY_ENV => Some("secret-key".into()),
        FOLDER_ID_ENV => Some("folder_123".into()),
        MODEL_VERSION_ENV => Some("bad/version".into()),
        _ => None,
    })
    .unwrap_err();
    assert_eq!(error, YandexLlmConfigError::InvalidModelVersion);
    assert!(!error.to_string().contains("bad/version"));
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
    assert_eq!(
        error.to_string(),
        "structured intent service returned HTTP 502"
    );
    assert!(error.is_transient());
    assert!(!error.to_string().contains("secret"));

    for (status, transient) in [
        (StatusCode::TOO_MANY_REQUESTS, true),
        (StatusCode::BAD_REQUEST, false),
    ] {
        let (endpoint, server) =
            start_mock(status, "upstream secret".into(), seen.clone(), None).await;
        let error = provider(endpoint, 1_000)
            .generate_json(&LlmRequest::radio_intent(&input()))
            .await
            .unwrap_err();
        server.abort();
        assert_eq!(error.is_transient(), transient);
    }

    let (endpoint, server) =
        start_mock(StatusCode::OK, "not JSON".to_owned(), seen.clone(), None).await;
    let error = provider(endpoint, 1_000)
        .generate_json(&LlmRequest::radio_intent(&input()))
        .await
        .unwrap_err();
    server.abort();
    assert_eq!(
        error.to_string(),
        "structured intent service returned a malformed response"
    );
    assert!(!error.is_transient());

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
    assert_eq!(error.to_string(), "structured intent request timed out");
    assert!(error.is_transient());
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

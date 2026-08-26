//! Contract coverage for the stable Windows voice-command JSON routes.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use rockserver::{
    http::router_with_search_service_and_voice_timeout,
    search::{
        RankedStation, RepositoryError, SearchConstraints, SearchQuery, SearchService,
        StationRepository,
    },
};
use serde_json::{Value, json};
use tokio::time::sleep;
use tower::ServiceExt;

const CANONICAL_PATH: &str = "/api/v1/voice/command";
const COMPATIBILITY_PATH: &str = "/v1/voice/command";
const API_TOKEN: &str = rockserver::http::TEST_API_BEARER_TOKEN;

#[tokio::test]
async fn canonical_voice_command_echoes_request_id_and_selects_first_station() {
    let response = rockserver::http::router()
        .oneshot(
            Request::post(CANONICAL_PATH)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {API_TOKEN}"))
                .header("x-request-id", "windows-voice-42")
                .body(Body::from(r#"{"transcript":" rock "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let request_id_header = response.headers().get("x-request-id").unwrap().clone();
    let status = response.status();
    let body = response_body(response).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(request_id_header, "windows-voice-42");
    assert_eq!(body["request_id"], "windows-voice-42");
    assert_eq!(body["transcript"], "rock");
    assert!(
        body["selected_station"]["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(body["stations"][0], body["selected_station"]);
}

#[tokio::test]
async fn compatibility_voice_command_alias_has_the_same_contract() {
    let response = rockserver::http::router()
        .oneshot(
            Request::post(COMPATIBILITY_PATH)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {API_TOKEN}"))
                .body(Body::from(r#"{"transcript":"baroque opera"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_body(response).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["request_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(body["selected_station"], Value::Null);
    assert_eq!(body["stations"], json!([]));
}

#[tokio::test]
async fn voice_command_validation_uses_transcript_field_and_standard_error_shape() {
    let response = rockserver::http::router()
        .oneshot(
            Request::post(CANONICAL_PATH)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {API_TOKEN}"))
                .body(Body::from(r#"{"transcript":"   "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_body(response).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_contract_error(&body, "validation_failed", "Request validation failed.");
    assert!(body["details"].get("transcript").is_some());
}

#[tokio::test]
async fn voice_command_rejects_oversized_json_with_a_structured_413_error() {
    let payload = format!(r#"{{"transcript":"{}"}}"#, "a".repeat(64 * 1024));
    let response = rockserver::http::router()
        .oneshot(
            Request::post(CANONICAL_PATH)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {API_TOKEN}"))
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_body(response).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_contract_error(
        &body,
        "request_too_large",
        "Request body exceeds the allowed size.",
    );
    assert_eq!(body["details"]["max_bytes"], 16_384);
}

struct SlowRepository;

#[async_trait]
impl StationRepository for SlowRepository {
    async fn search(
        &self,
        _query: &SearchQuery,
        _constraints: &SearchConstraints,
        _embedding: Option<&rockserver::search::Embedding>,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        sleep(Duration::from_millis(50)).await;
        Ok(Vec::new())
    }

    async fn check_readiness(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

#[tokio::test]
async fn voice_command_search_timeout_is_a_structured_504_error() {
    let app = router_with_search_service_and_voice_timeout(
        SearchService::new(Arc::new(SlowRepository)),
        Duration::from_millis(1),
    );
    let response = app
        .oneshot(
            Request::post(CANONICAL_PATH)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {API_TOKEN}"))
                .body(Body::from(r#"{"transcript":"jazz"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_body(response).await;

    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_contract_error(&body, "search_timeout", "Voice command search timed out.");
    assert_eq!(body["details"]["timeout_ms"], 1);
}

async fn response_body(response: axum::response::Response) -> Value {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

fn assert_contract_error(body: &Value, code: &str, message: &str) {
    assert_eq!(body["code"], code);
    assert_eq!(body["message"], message);
    assert!(body["request_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(body["details"].is_object());
}

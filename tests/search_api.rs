//! In-memory integration coverage for the public station-search route.

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const API_TOKEN: &str = rockserver::http::TEST_API_BEARER_TOKEN;

#[tokio::test]
async fn successful_search_returns_normalized_query_and_station_results() {
    let (status, body) = search(json!({"query": "calm instrumental jazz"})).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["request_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(body["normalized_query"]["locale"], "en-US");
    assert_eq!(
        body["normalized_query"]["tags"],
        json!(["calm", "instrumental", "jazz"])
    );
    assert_eq!(body["stations"][0]["id"], "station-jazz-001");
    assert_eq!(body["stations"][0]["health"], "healthy");
}

#[tokio::test]
async fn unmatched_query_returns_an_empty_station_list() {
    let (status, body) = search(json!({"query": "baroque opera"})).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stations"], json!([]));
}

#[tokio::test]
async fn limit_is_applied_after_ranking() {
    let (status, body) = search(json!({"query": "rock", "limit": 1})).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stations"].as_array().unwrap().len(), 1);
    assert_eq!(body["stations"][0]["id"], "station-rock-ru-001");
}

#[tokio::test]
async fn equal_score_results_use_station_id_as_a_stable_tie_break() {
    let (_, first) = search(json!({"query": "rock"})).await;
    let (_, second) = search(json!({"query": "rock"})).await;
    let first_ids = station_ids(&first);
    let second_ids = station_ids(&second);

    assert_eq!(
        first_ids,
        vec![
            "station-rock-ru-001",
            "station-rock-001",
            "station-rock-002",
        ]
    );
    assert_eq!(first_ids, second_ids);
}

#[tokio::test]
async fn excluded_station_ids_do_not_appear_in_results() {
    let (status, body) = search(json!({
        "query": "jazz",
        "exclude_station_ids": ["station-jazz-001"]
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(station_ids(&body), vec!["station-jazz-002"]);
}

#[tokio::test]
async fn malformed_json_returns_a_contract_compliant_400_error() {
    let response = rockserver::http::router()
        .oneshot(
            Request::post("/v1/search")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {API_TOKEN}"))
                .body(Body::from(r#"{"query":"jazz""#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_body(response).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_contract_error(
        &body,
        "malformed_request",
        "Request body must contain valid JSON.",
    );
}

#[tokio::test]
async fn semantic_validation_returns_a_contract_compliant_422_error() {
    let (status, body) = search(json!({"query": "   ", "limit": 0})).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_contract_error(&body, "validation_failed", "Request validation failed.");
    assert!(body["details"].get("query").is_some());
    assert!(body["details"].get("limit").is_some());
}

async fn search(payload: Value) -> (StatusCode, Value) {
    let response = rockserver::http::router()
        .oneshot(
            Request::post("/v1/search")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {API_TOKEN}"))
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, response_body(response).await)
}

#[tokio::test]
async fn application_endpoints_require_a_bearer_token_but_health_remains_public() {
    let app = rockserver::http::router();
    let unauthorized = app
        .clone()
        .oneshot(
            Request::post("/v1/search")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"query":"jazz"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unauthorized.headers()[header::WWW_AUTHENTICATE], "Bearer");
    assert_contract_error(
        &response_body(unauthorized).await,
        "authentication_required",
        "A valid Bearer token is required.",
    );

    let health = app
        .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

async fn response_body(response: axum::response::Response) -> Value {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

fn station_ids(body: &Value) -> Vec<&str> {
    body["stations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|station| station["id"].as_str().unwrap())
        .collect()
}

fn assert_contract_error(body: &Value, code: &str, message: &str) {
    assert_eq!(body["code"], code);
    assert_eq!(body["message"], message);
    assert!(body["request_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(body["details"].is_object());
}

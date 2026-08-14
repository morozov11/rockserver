//! Structural checks for the contract-first OpenAPI document.

use serde_yaml::Value;

const OPENAPI: &str = include_str!("../api/openapi.yaml");

/// Reads a slash-delimited mapping path from a YAML value.
fn value_at<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('/').try_fold(root, |value, key| value.get(key))
}

#[test]
fn openapi_contract_is_parseable_and_has_required_surface() {
    let document: Value = serde_yaml::from_str(OPENAPI).expect("OpenAPI YAML must parse");

    let version = value_at(&document, "openapi")
        .and_then(Value::as_str)
        .expect("OpenAPI version must be a string");
    assert!(version.starts_with("3."), "expected OpenAPI 3.x");

    for path in [
        "/health/live",
        "/health/ready",
        "/v1/search",
        "/api/v1/voice/command",
        "/v1/voice/command",
        "/api/v1/voice/stream",
        "/v1/voice/stream",
    ] {
        assert!(
            value_at(&document, "paths")
                .and_then(|paths| paths.get(path))
                .is_some(),
            "missing required path {path}"
        );
    }

    assert!(
        value_at(&document, "paths")
            .and_then(|paths| paths.get("/v1/search"))
            .and_then(|search| search.get("post"))
            .is_some(),
        "search path must define POST"
    );
    let voice = value_at(&document, "paths")
        .and_then(|paths| paths.get("/api/v1/voice/command"))
        .and_then(|command| command.get("post"))
        .expect("canonical voice-command path must define POST");
    for status in ["200", "400", "413", "422", "500", "504"] {
        assert!(
            voice
                .get("responses")
                .and_then(|responses| responses.get(status))
                .is_some(),
            "voice command must document {status}"
        );
    }
    assert_eq!(
        value_at(&document, "paths")
            .and_then(|paths| paths.get("/v1/voice/command"))
            .and_then(|command| command.get("post"))
            .and_then(|operation| operation.get("deprecated"))
            .and_then(Value::as_bool),
        Some(true),
        "the /v1 voice route must remain an explicitly deprecated compatibility alias"
    );
    let voice_stream = value_at(&document, "paths")
        .and_then(|paths| paths.get("/api/v1/voice/stream"))
        .and_then(|stream| stream.get("get"))
        .expect("canonical voice stream path must define GET upgrade");
    assert!(
        voice_stream.get("x-websocket-client-messages").is_some()
            && voice_stream.get("x-websocket-server-messages").is_some(),
        "voice stream must document both WebSocket message directions"
    );
    assert_eq!(
        value_at(&document, "paths")
            .and_then(|paths| paths.get("/v1/voice/stream"))
            .and_then(|stream| stream.get("get"))
            .and_then(|operation| operation.get("deprecated"))
            .and_then(Value::as_bool),
        Some(true),
        "the /v1 stream route must remain an explicitly deprecated compatibility alias"
    );
    assert!(
        value_at(&document, "paths")
            .and_then(|paths| paths.get("/health/ready"))
            .and_then(|ready| ready.get("get"))
            .and_then(|get| get.get("responses"))
            .and_then(|responses| responses.get("503"))
            .is_some(),
        "readiness must document PostgreSQL unavailability"
    );

    let schemas = value_at(&document, "components/schemas")
        .and_then(Value::as_mapping)
        .expect("components.schemas must be a mapping");
    for schema in [
        "SearchRequest",
        "SearchResponse",
        "VoiceCommandRequest",
        "VoiceCommandResponse",
        "VoiceStreamStart",
        "VoiceStreamCommit",
        "VoiceStreamReady",
        "VoiceStreamTranscript",
        "VoiceStreamResult",
        "VoiceStreamError",
        "NormalizedQuery",
        "StationResult",
        "ErrorResponse",
    ] {
        assert!(
            schemas.contains_key(Value::String(schema.to_owned())),
            "missing required schema {schema}"
        );
    }
}

#[tokio::test]
async fn search_endpoint_is_registered() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    let response = rockserver::http::router()
        .oneshot(
            Request::post("/v1/search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":"jazz"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

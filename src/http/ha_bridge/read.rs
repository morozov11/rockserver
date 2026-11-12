//! Browser-cookie reads of the owner's HA bridge projections.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::json;
use uuid::Uuid;

use super::super::{
    state::AppState,
    transport::{
        cookie_value, error_response, no_store_json_response, request_id, token_hash,
        trusted_proxy_header_matches,
    },
};

// Both read endpoints use the same first-party owner boundary as Yandex Home.
async fn owner(state: &AppState, headers: &HeaderMap, id: &str) -> Result<Uuid, Response> {
    if !trusted_proxy_header_matches(headers, state.trusted_proxy_token.as_deref()) {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "Trusted proxy is required.",
            id,
            json!({}),
        ));
    }
    let Some(cookie) = cookie_value(headers, "rockserver_browser") else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A browser session is required.",
            id,
            json!({}),
        ));
    };
    let Some(accounts) = state.account_store.as_ref() else {
        return Err(unavailable(id));
    };
    match accounts.browser_session_user(&token_hash(cookie)).await {
        Ok(Some(owner)) => Ok(owner),
        Ok(None) => Err(error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A browser session is required.",
            id,
            json!({}),
        )),
        Err(_) => Err(unavailable(id)),
    }
}

fn unavailable(id: &str) -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "integration_unavailable",
        "Integration storage is unavailable.",
        id,
        json!({}),
    )
}

/// Returns the signed-in owner's integration, sensor and weather read model.
pub(crate) async fn overview(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let id = request_id(&headers);
    let owner = match owner(&state, &headers, &id).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some(store) = state.ha_bridge.as_ref() else {
        return unavailable(&id);
    };
    match store.read_overview(owner).await {
        Ok(value) => no_store_json_response(value, &id),
        Err(_) => unavailable(&id),
    }
}

/// Returns only the signed-in owner's last weather value with explicit freshness.
pub(crate) async fn weather(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let id = request_id(&headers);
    let owner = match owner(&state, &headers, &id).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some(store) = state.ha_bridge.as_ref() else {
        return unavailable(&id);
    };
    match store.read_weather(owner).await {
        Ok(Some(value)) => no_store_json_response(value, &id),
        Ok(None) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "weather_unavailable",
            "Home Assistant weather is unavailable.",
            &id,
            json!({}),
        ),
        Err(_) => unavailable(&id),
    }
}

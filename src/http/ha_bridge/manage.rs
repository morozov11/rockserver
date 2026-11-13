//! Browser self-service for owner-scoped HA credentials and publication policy.

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ha_bridge::{credential_hash, valid_selection};

use super::super::{
    account::browser_mutation_owner,
    state::AppState,
    transport::{error_response, no_store_json_response, request_id, with_request_id},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisionRequest {
    sensors: Vec<String>,
    weather_entity: String,
}

#[derive(Serialize)]
struct CredentialResponse {
    credential: String,
}

/// Issues a replacement credential once to the signed-in owner after CSRF and policy checks.
pub(crate) async fn provision(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<ProvisionRequest>, JsonRejection>,
) -> Response {
    let id = request_id(&headers);
    let owner = match browser_mutation_owner(&state, &headers, &id).await {
        Ok(owner) => owner,
        Err(response) => return *response,
    };
    let Json(payload) = match payload {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Invalid Home Assistant selection.",
                &id,
                json!({}),
            );
        }
    };
    if !valid_selection(&payload.sensors, &payload.weather_entity) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_selection",
            "Select at most 128 unique sensor IDs and one weather ID.",
            &id,
            json!({}),
        );
    }
    let Some(store) = state.ha_bridge.as_ref() else {
        return unavailable(&id);
    };
    let mut bytes = [0_u8; 32];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return unavailable(&id);
    }
    let credential = format!(
        "ha1.{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    match store
        .provision(
            owner,
            &credential_hash(&credential),
            &payload.sensors,
            &payload.weather_entity,
        )
        .await
    {
        Ok(true) => no_store_json_response(CredentialResponse { credential }, &id),
        Ok(false) => error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "An active account is required.",
            &id,
            json!({}),
        ),
        Err(_) => unavailable(&id),
    }
}

/// Revokes the signed-in owner's credential and hides its HA observations.
pub(crate) async fn revoke(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let id = request_id(&headers);
    let owner = match browser_mutation_owner(&state, &headers, &id).await {
        Ok(owner) => owner,
        Err(response) => return *response,
    };
    let Some(store) = state.ha_bridge.as_ref() else {
        return unavailable(&id);
    };
    match store.revoke(owner).await {
        Ok(()) => with_request_id(StatusCode::NO_CONTENT.into_response(), &id),
        Err(_) => unavailable(&id),
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

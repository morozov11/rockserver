//! Liveness and readiness handlers.

use super::state::AppState;
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

/// Stable JSON response returned by the health endpoints.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthResponse {
    /// Current process health status.
    pub status: HealthStatus,
}
/// Operational health states returned by the health endpoints.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// The process is running or dependencies are available.
    Ok,
    /// A dependency is unavailable.
    NotReady,
}

fn health_response() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HealthStatus::Ok,
    })
}
/// Returns the process liveness response.
pub(super) async fn live() -> Json<HealthResponse> {
    health_response()
}
/// Returns readiness based on the search-service dependency.
pub(super) async fn ready(State(state): State<AppState>) -> Response {
    match state.search_service.check_readiness().await {
        Ok(()) => health_response().into_response(),
        Err(error) => {
            tracing::warn!(error = %error, "readiness dependency check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: HealthStatus::NotReady,
                }),
            )
                .into_response()
        }
    }
}

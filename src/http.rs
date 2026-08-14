//! Axum routes and transport DTOs for the RockServer HTTP API.

use std::{
    collections::BTreeSet,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tower_http::trace::TraceLayer;

use crate::search::{
    InMemoryStationRepository, QueryParserInput, RankedStation, SearchConstraints, SearchService,
    StationHealth, StationRepository,
};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Stable JSON response returned by the health endpoints.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthResponse {
    /// Current process health status.
    pub status: HealthStatus,
}

/// Operational health states returned by liveness and readiness endpoints.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// The process is running or its configured dependencies are available.
    Ok,
    /// A required dependency is currently unavailable.
    NotReady,
}

/// Creates the application router with the default in-memory catalog backend.
pub fn router() -> Router {
    router_with_repository(Arc::new(InMemoryStationRepository::with_builtin_catalog()))
}

/// Creates the application router with a supplied station repository backend.
pub fn router_with_repository(repository: Arc<dyn StationRepository + Send + Sync>) -> Router {
    router_with_search_service(SearchService::new(repository))
}

/// Creates the application router with fully configured search orchestration.
pub fn router_with_search_service(search_service: SearchService) -> Router {
    let state = AppState { search_service };

    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/v1/search", post(search))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn live() -> Json<HealthResponse> {
    health_response()
}

async fn ready(State(state): State<AppState>) -> Response {
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

async fn search(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let request_id = next_request_id();
    if !is_json_content_type(&headers) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "malformed_request",
            "Request body must contain valid JSON.",
            request_id,
            json!({"content_type": "application/json is required"}),
        );
    }

    let value = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "malformed_request",
                "Request body must contain valid JSON.",
                request_id,
                json!({"body": error.to_string()}),
            );
        }
    };
    let request = match serde_json::from_value::<SearchRequestDto>(value) {
        Ok(request) => request,
        Err(error) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "Request validation failed.",
                request_id,
                json!({"request": error.to_string()}),
            );
        }
    };
    let validated = match ValidatedSearchRequest::try_from(request) {
        Ok(request) => request,
        Err(details) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "Request validation failed.",
                request_id,
                Value::Object(details),
            );
        }
    };

    let constraints = SearchConstraints {
        limit: validated.limit,
        excluded_station_ids: validated.exclude_station_ids,
    };
    let outcome = match state
        .search_service
        .interpret_and_search(
            QueryParserInput {
                query: validated.query,
                locale: validated.locale,
            },
            &constraints,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::error!(%error, %request_id, "station search failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An unexpected server error occurred.",
                request_id,
                json!({}),
            );
        }
    };

    Json(SearchResponseDto {
        request_id,
        normalized_query: NormalizedQueryDto::from(outcome.query),
        stations: outcome
            .stations
            .iter()
            .map(StationResultDto::from)
            .collect(),
    })
    .into_response()
}

fn health_response() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HealthStatus::Ok,
    })
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
}

fn next_request_id() -> String {
    format!(
        "req_{:016x}",
        REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn error_response(
    status: StatusCode,
    code: &str,
    message: &str,
    request_id: String,
    details: Value,
) -> Response {
    (
        status,
        Json(ErrorResponseDto {
            code: code.to_owned(),
            message: message.to_owned(),
            request_id,
            details,
        }),
    )
        .into_response()
}

#[derive(Clone)]
struct AppState {
    search_service: SearchService,
}

/// Transport representation of the `SearchRequest` OpenAPI schema.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchRequestDto {
    query: String,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    limit: Option<u8>,
    #[serde(default)]
    exclude_station_ids: Vec<String>,
}

struct ValidatedSearchRequest {
    query: String,
    locale: String,
    limit: usize,
    exclude_station_ids: BTreeSet<String>,
}

impl TryFrom<SearchRequestDto> for ValidatedSearchRequest {
    type Error = Map<String, Value>;

    fn try_from(value: SearchRequestDto) -> Result<Self, Self::Error> {
        let mut details = Map::new();
        let query = value.query.trim().to_owned();
        if query.is_empty() {
            details.insert(
                "query".to_owned(),
                json!("must not be empty or whitespace only"),
            );
        } else if query.chars().count() > 500 {
            details.insert(
                "query".to_owned(),
                json!("must contain at most 500 characters"),
            );
        }

        let locale = value.locale.unwrap_or_else(|| "en-US".to_owned());
        if !is_valid_locale(&locale) {
            details.insert("locale".to_owned(), json!("must be a BCP 47-style locale"));
        }

        let limit = value.limit.unwrap_or(10);
        if !(1..=50).contains(&limit) {
            details.insert("limit".to_owned(), json!("must be between 1 and 50"));
        }

        if value.exclude_station_ids.len() > 100 {
            details.insert(
                "exclude_station_ids".to_owned(),
                json!("must contain at most 100 station IDs"),
            );
        }
        let excluded_id_count = value.exclude_station_ids.len();
        let ids = value
            .exclude_station_ids
            .into_iter()
            .collect::<BTreeSet<_>>();
        if ids.len() != excluded_id_count {
            details.insert(
                "exclude_station_ids".to_owned(),
                json!("must not contain duplicate station IDs"),
            );
        }
        if ids
            .iter()
            .any(|station_id| station_id.is_empty() || station_id.chars().count() > 128)
        {
            details.insert(
                "exclude_station_ids".to_owned(),
                json!("each station ID must contain between 1 and 128 characters"),
            );
        }

        if details.is_empty() {
            Ok(Self {
                query,
                locale,
                limit: usize::from(limit),
                exclude_station_ids: ids,
            })
        } else {
            Err(details)
        }
    }
}

/// Successful transport response for `POST /v1/search`.
#[derive(Serialize)]
struct SearchResponseDto {
    request_id: String,
    normalized_query: NormalizedQueryDto,
    stations: Vec<StationResultDto>,
}

/// Transport representation of normalized query constraints.
#[derive(Serialize)]
struct NormalizedQueryDto {
    original: String,
    locale: String,
    terms: Vec<String>,
    tags: Vec<String>,
    language: Option<String>,
    country_code: Option<String>,
}

impl From<crate::search::SearchQuery> for NormalizedQueryDto {
    fn from(query: crate::search::SearchQuery) -> Self {
        Self {
            original: query.original,
            locale: query.locale,
            terms: query.terms,
            tags: query.tags,
            language: query.language,
            country_code: query.country_code,
        }
    }
}

/// Transport representation of a ranked station result.
#[derive(Serialize)]
struct StationResultDto {
    id: String,
    name: String,
    stream_url: String,
    homepage_url: Option<String>,
    tags: Vec<String>,
    language: Option<String>,
    country_code: Option<String>,
    codec: Option<String>,
    bitrate_kbps: Option<u32>,
    score: f64,
    reason: String,
    health: &'static str,
}

impl From<&RankedStation> for StationResultDto {
    fn from(ranked: &RankedStation) -> Self {
        let station = &ranked.station;
        Self {
            id: station.id.clone(),
            name: station.name.clone(),
            stream_url: station.stream_url.clone(),
            homepage_url: station.homepage_url.clone(),
            tags: station.tags.clone(),
            language: station.language.clone(),
            country_code: station.country_code.clone(),
            codec: station.codec.clone(),
            bitrate_kbps: station.bitrate_kbps,
            score: ranked.score,
            reason: ranked.reason.clone(),
            health: match station.health {
                StationHealth::Healthy => "healthy",
                StationHealth::Degraded => "degraded",
                StationHealth::Unknown => "unknown",
            },
        }
    }
}

/// Contract-compliant transport error body.
#[derive(Serialize)]
struct ErrorResponseDto {
    code: String,
    message: String,
    request_id: String,
    details: Value,
}

fn is_valid_locale(locale: &str) -> bool {
    let mut subtags = locale.split('-');
    let Some(language) = subtags.next() else {
        return false;
    };
    (2..=3).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_alphabetic())
        && subtags.all(|subtag| {
            (2..=8).contains(&subtag.len())
                && subtag.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::{HealthResponse, HealthStatus, router};

    #[tokio::test]
    async fn liveness_returns_stable_json_response() {
        assert_health_endpoint("/health/live").await;
    }

    #[tokio::test]
    async fn readiness_returns_stable_json_response() {
        assert_health_endpoint("/health/ready").await;
    }

    async fn assert_health_endpoint(uri: &str) {
        let response = router()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let payload: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            payload,
            HealthResponse {
                status: HealthStatus::Ok
            }
        );
        assert_eq!(body.as_ref(), br#"{"status":"ok"}"#);
    }
}

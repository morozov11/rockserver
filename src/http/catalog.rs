//! Public catalog HTTP handlers.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::{
    state::{AppState, PublicLimit},
    transport::{CatalogPageDto, PublicStationDto, error_response, request_id, with_request_id},
};

const CATALOG_LIST_LIMIT: PublicLimit = PublicLimit {
    requests: 60,
    burst: 20,
};
const CATALOG_GET_LIMIT: PublicLimit = PublicLimit {
    requests: 120,
    burst: 40,
};

/// Query parameters for the bounded public catalog listing.
#[derive(Deserialize)]
pub(super) struct CatalogQuery {
    #[serde(default)]
    limit: Option<u8>,
    #[serde(default)]
    cursor: Option<String>,
}

/// Lists the bounded active public catalog using an opaque stable-ID cursor.
pub(super) async fn public_catalog_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CatalogQuery>,
) -> Response {
    let request_id = request_id(&headers);
    if let Err(response) =
        state.public_request_allowed("catalog_list", CATALOG_LIST_LIMIT, &request_id)
    {
        return *response;
    }
    let limit = query.limit.unwrap_or(50);
    if !(1..=50).contains(&limit)
        || query.cursor.as_ref().is_some_and(|cursor| {
            cursor.is_empty()
                || cursor.len() > 512
                || !cursor
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "malformed_request",
            "Catalog request is invalid.",
            &request_id,
            json!({"field":"limit_or_cursor"}),
        );
    }
    let stations = match state
        .search_service
        .public_catalog(query.cursor.as_deref(), usize::from(limit))
        .await
    {
        Ok(stations) => stations,
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "Catalog is temporarily unavailable.",
                &request_id,
                json!({}),
            );
        }
    };
    let next_cursor = (stations.len() == usize::from(limit))
        .then(|| stations.last().map(|station| station.id.clone()))
        .flatten();
    with_request_id(
        Json(CatalogPageDto {
            request_id: request_id.clone(),
            stations: stations.iter().map(PublicStationDto::from).collect(),
            next_cursor,
        })
        .into_response(),
        &request_id,
    )
}

/// Returns one active public station or a safe not-found response.
pub(super) async fn public_catalog_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(station_id): Path<String>,
) -> Response {
    let request_id = request_id(&headers);
    if let Err(response) =
        state.public_request_allowed("catalog_get", CATALOG_GET_LIMIT, &request_id)
    {
        return *response;
    }
    if station_id.is_empty() || station_id.len() > 128 {
        return error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "Station was not found.",
            &request_id,
            json!({}),
        );
    }
    match state.search_service.public_station(&station_id).await {
        Ok(Some(station)) => with_request_id(
            Json(PublicStationDto::from(&station)).into_response(),
            &request_id,
        ),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "Station was not found.",
            &request_id,
            json!({}),
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "Catalog is temporarily unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

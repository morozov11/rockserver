//! Account-owned device-control directory HTTP projection.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    device_control::{
        DeviceControlScope, DeviceManifest, DeviceStateSnapshot, Entity, EntityDomain, EntityState,
        Freshness,
    },
    device_control_auth::DeviceControlAuthenticationError,
};

use super::{
    control_auth::authenticate_control_ingress,
    state::AppState,
    transport::{error_response, request_id, retry_after, unauthorized_response, with_request_id},
};

const MAX_DIRECTORY_DEVICES: usize = 50;

/// Typed, currently-modelled directory filters. Home and area are deliberately not v1 filters.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectoryFilters {
    /// Include devices declaring at least one entity in this domain.
    pub(super) domain: Option<EntityDomain>,
    /// Include devices declaring at least one entity with this class.
    pub(super) device_class: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct DirectoryDto {
    pub(crate) protocol_version: u8,
    pub(crate) generated_at: String,
    pub(crate) directory_revision: u64,
    pub(crate) granted_scopes: Vec<DeviceControlScope>,
    pub(crate) devices: Vec<DirectoryDeviceDto>,
}

#[derive(Clone, Serialize)]
pub(crate) struct DirectoryDeviceDto {
    pub(crate) device_id: crate::device_control::DeviceId,
    device_display_name: String,
    device_type: String,
    roles: Vec<crate::device_control::DeviceRole>,
    capabilities: crate::device_control::DeviceCapabilities,
    presence: PresenceDto,
    state_freshness: FreshnessDto,
    entities: Vec<DirectoryEntityDto>,
    surfaces: Vec<crate::device_control::Surface>,
}

#[derive(Clone, Serialize)]
struct PresenceDto {
    status: &'static str,
    last_seen_at: Option<String>,
}

#[derive(Clone, Serialize)]
struct FreshnessDto {
    status: Freshness,
    observed_at: Option<String>,
    received_at: Option<String>,
    stale_after: Option<String>,
}

#[derive(Clone, Serialize)]
struct DirectoryEntityDto {
    #[serde(flatten)]
    entity: Entity,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<EntityState>,
}

/// Builds one account-owned snapshot after the caller's scope has already been verified.
pub(crate) async fn snapshot(
    state: &AppState,
    user_id: uuid::Uuid,
    scopes: &[DeviceControlScope],
    filters: &DirectoryFilters,
    cursor: u64,
) -> Result<DirectoryDto, ()> {
    let (Some(account_store), Some(control_store)) =
        (state.account_store.as_ref(), state.control_store.as_ref())
    else {
        return Err(());
    };
    let devices = account_store
        .list_owned_devices(user_id)
        .await
        .map_err(|_| ())?;
    if devices.len() > MAX_DIRECTORY_DEVICES {
        return Err(());
    }
    let mut entries = Vec::new();
    for device in devices {
        let device_id = crate::device_control::DeviceId(device.id);
        let Some(manifest) = control_store
            .load_manifest(user_id, device_id)
            .await
            .map_err(|_| ())?
        else {
            continue;
        };
        if !matches_filters(&manifest, filters) {
            continue;
        }
        let state_snapshot = control_store
            .load_device_state(user_id, device_id)
            .await
            .map_err(|_| ())?;
        let include_states = scopes.contains(&DeviceControlScope::EntityStateRead);
        let mut entities = Vec::with_capacity(manifest.entities.len());
        for entity in manifest.entities.clone() {
            let entity_state = if include_states {
                control_store
                    .load_entity_state(user_id, device_id, &entity.entity_id)
                    .await
                    .map_err(|_| ())?
            } else {
                None
            };
            entities.push(DirectoryEntityDto {
                entity,
                state: entity_state,
            });
        }
        let online = state
            .control_registry
            .active_for(user_id, device.id)
            .is_some();
        entries.push(DirectoryDeviceDto {
            device_id,
            device_display_name: device.device_display_name,
            device_type: device.device_type,
            roles: manifest.roles,
            capabilities: manifest.capabilities,
            presence: PresenceDto {
                status: if online { "online" } else { "offline" },
                last_seen_at: if online {
                    Some(now())
                } else {
                    device.last_seen_at
                },
            },
            state_freshness: freshness(state_snapshot.as_ref()),
            entities,
            surfaces: manifest.surfaces,
        });
    }
    Ok(DirectoryDto {
        protocol_version: 1,
        generated_at: now(),
        directory_revision: cursor.max(1),
        granted_scopes: scopes.to_vec(),
        devices: entries,
    })
}

/// Reads the bounded controller directory after native control authentication and scope checks.
pub(super) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    filters: Result<Query<DirectoryFilters>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let request_id = request_id(&headers);
    let Query(filters) = match filters {
        Ok(filters) => filters,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_directory_filter",
                "Directory filters are invalid or unsupported.",
                &request_id,
                json!({}),
            );
        }
    };
    if filters.device_class.as_ref().is_some_and(|value| {
        value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    }) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_directory_filter",
            "Directory filters are invalid or unsupported.",
            &request_id,
            json!({}),
        );
    }
    let Some(resolver) = state.control_session_resolver.as_ref() else {
        return retry_after(
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "control_unavailable",
                "Device control is temporarily unavailable.",
                &request_id,
                json!({}),
            ),
            1,
        );
    };
    let principal = match authenticate_control_ingress(&headers, resolver.as_ref()).await {
        Ok(principal) => principal,
        Err(DeviceControlAuthenticationError::InvalidCredential) => {
            return unauthorized_response(&request_id);
        }
        Err(DeviceControlAuthenticationError::Unavailable) => {
            return retry_after(
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "control_auth_unavailable",
                    "Device control authentication is temporarily unavailable.",
                    &request_id,
                    json!({}),
                ),
                1,
            );
        }
    };
    let (Some(account_store), Some(control_store)) =
        (state.account_store.as_ref(), state.control_store.as_ref())
    else {
        return retry_after(
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "control_unavailable",
                "Device control is temporarily unavailable.",
                &request_id,
                json!({}),
            ),
            1,
        );
    };
    let caller_manifest = match control_store
        .load_manifest(
            principal.user_id,
            crate::device_control::DeviceId(principal.device_id),
        )
        .await
    {
        Ok(Some(manifest)) => manifest,
        Ok(None) => {
            return error_response(
                StatusCode::FORBIDDEN,
                "directory_forbidden",
                "Directory access is not permitted.",
                &request_id,
                json!({}),
            );
        }
        Err(_) => {
            return retry_after(
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "control_projection_unavailable",
                    "Device control projection is temporarily unavailable.",
                    &request_id,
                    json!({}),
                ),
                1,
            );
        }
    };
    let scopes = granted_scopes(&caller_manifest);
    if !scopes.contains(&DeviceControlScope::DirectoryRead) {
        return error_response(
            StatusCode::FORBIDDEN,
            "directory_forbidden",
            "Directory access is not permitted.",
            &request_id,
            json!({}),
        );
    }
    let devices = match account_store.list_owned_devices(principal.user_id).await {
        Ok(devices) if devices.len() <= MAX_DIRECTORY_DEVICES => devices,
        Ok(_) => {
            return retry_after(
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "control_projection_unavailable",
                    "Device control projection is temporarily unavailable.",
                    &request_id,
                    json!({}),
                ),
                1,
            );
        }
        Err(_) => {
            return retry_after(
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "control_projection_unavailable",
                    "Device control projection is temporarily unavailable.",
                    &request_id,
                    json!({}),
                ),
                1,
            );
        }
    };
    let cursor = state.control_state_hub.cursor(principal.user_id);
    let mut entries = Vec::new();
    for device in devices {
        let device_id = crate::device_control::DeviceId(device.id);
        let Some(manifest) = (match control_store
            .load_manifest(principal.user_id, device_id)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                return retry_after(
                    error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "control_projection_unavailable",
                        "Device control projection is temporarily unavailable.",
                        &request_id,
                        json!({}),
                    ),
                    1,
                );
            }
        }) else {
            continue;
        };
        if !matches_filters(&manifest, &filters) {
            continue;
        }
        let state_snapshot = match control_store
            .load_device_state(principal.user_id, device_id)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                return retry_after(
                    error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "control_projection_unavailable",
                        "Device control projection is temporarily unavailable.",
                        &request_id,
                        json!({}),
                    ),
                    1,
                );
            }
        };
        let include_states = scopes.contains(&DeviceControlScope::EntityStateRead);
        let mut entities = Vec::with_capacity(manifest.entities.len());
        for entity in manifest.entities.clone() {
            let entity_id = entity.entity_id.clone();
            let entity_state = if include_states {
                match control_store
                    .load_entity_state(principal.user_id, device_id, &entity_id)
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        return retry_after(
                            error_response(
                                StatusCode::SERVICE_UNAVAILABLE,
                                "control_projection_unavailable",
                                "Device control projection is temporarily unavailable.",
                                &request_id,
                                json!({}),
                            ),
                            1,
                        );
                    }
                }
            } else {
                None
            };
            entities.push(DirectoryEntityDto {
                entity,
                state: entity_state,
            });
        }
        let online = state
            .control_registry
            .active_for(principal.user_id, device.id)
            .is_some();
        entries.push(DirectoryDeviceDto {
            device_id,
            device_display_name: device.device_display_name,
            device_type: device.device_type,
            roles: manifest.roles,
            capabilities: manifest.capabilities,
            presence: PresenceDto {
                status: if online { "online" } else { "offline" },
                last_seen_at: if online {
                    Some(now())
                } else {
                    device.last_seen_at
                },
            },
            state_freshness: freshness(state_snapshot.as_ref()),
            entities,
            surfaces: manifest.surfaces,
        });
    }
    let mut response = with_request_id(
        Json(DirectoryDto {
            protocol_version: 1,
            generated_at: now(),
            directory_revision: cursor.max(1),
            granted_scopes: scopes,
            devices: entries,
        })
        .into_response(),
        &request_id,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// Server policy for the existing registered controller class; manifest roles stay separate DTO facts.
pub(crate) fn granted_scopes(manifest: &DeviceManifest) -> Vec<DeviceControlScope> {
    if manifest
        .roles
        .contains(&crate::device_control::DeviceRole::Controller)
    {
        vec![
            DeviceControlScope::DirectoryRead,
            DeviceControlScope::EntityStateRead,
            DeviceControlScope::MediaControl,
            DeviceControlScope::DisplayControl,
            DeviceControlScope::ActuatorControl,
        ]
    } else {
        Vec::new()
    }
}

fn matches_filters(manifest: &DeviceManifest, filters: &DirectoryFilters) -> bool {
    manifest.entities.iter().any(|entity| {
        filters
            .domain
            .as_ref()
            .is_none_or(|domain| domain == &entity.domain)
            && filters
                .device_class
                .as_ref()
                .is_none_or(|class| class == &entity.device_class)
    }) || (filters.domain.is_none() && filters.device_class.is_none())
}

fn freshness(state: Option<&DeviceStateSnapshot>) -> FreshnessDto {
    let Some(state) = state else {
        return FreshnessDto {
            status: Freshness::Unknown,
            observed_at: None,
            received_at: None,
            stale_after: None,
        };
    };
    FreshnessDto {
        status: if state.received_at.is_some() {
            Freshness::Fresh
        } else {
            Freshness::Unknown
        },
        observed_at: Some(state.observed_at.as_str().to_owned()),
        received_at: state
            .received_at
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        stale_after: None,
    }
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("RFC3339 formatter is valid")
}

//! Separate authenticated WebSocket ingress for a Home Assistant custom integration.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::ha_bridge::{
    BridgePrincipal, BridgeStore, IngestOutcome, SensorBatch, SensorReading, WeatherBatch,
    WeatherReading, credential_hash, validate_sensors, validate_weather,
};

use super::{
    state::{AppState, PublicLimitState},
    transport::{error_response, request_id, unauthorized_response, with_request_id},
};

const MAX_FRAME_BYTES: usize = 65_536;
const MAX_FRAMES_PER_MINUTE: usize = 60;

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Incoming {
    #[serde(rename = "integration.hello")]
    Hello { protocol_version: u8 },
    #[serde(rename = "integration.snapshot")]
    Snapshot {
        revision: u64,
        sensors: Vec<SensorReading>,
    },
    #[serde(rename = "integration.state")]
    State {
        revision: u64,
        sensors: Vec<SensorReading>,
    },
    #[serde(rename = "integration.weather")]
    Weather {
        revision: u64,
        weather: WeatherReading,
    },
    #[serde(rename = "integration.heartbeat")]
    Heartbeat,
}

/// Authenticates an integration-specific bearer before upgrading the socket.
pub(super) async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let id = request_id(&headers);
    let Some(store) = state.ha_bridge else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "integration_unavailable",
            "Integration storage is unavailable.",
            &id,
            json!({}),
        );
    };
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.starts_with("ha1.") && (40..=128).contains(&v.len()))
    else {
        return unauthorized_response(&id);
    };
    let principal = match store.authenticate(&credential_hash(token)).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return unauthorized_response(&id),
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "integration_unavailable",
                "Integration storage is unavailable.",
                &id,
                json!({}),
            );
        }
    };
    let epoch = Uuid::new_v4();
    let limits = Arc::clone(&state.public_limits);
    if store.begin(&principal, epoch).await.is_err() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "integration_unavailable",
            "Integration storage is unavailable.",
            &id,
            json!({}),
        );
    }
    with_request_id(
        upgrade
            .max_message_size(MAX_FRAME_BYTES + 1)
            .on_upgrade(move |socket| run(socket, store, principal, epoch, limits)),
        &id,
    )
}

async fn run(
    mut socket: WebSocket,
    store: Arc<dyn BridgeStore>,
    principal: BridgePrincipal,
    epoch: Uuid,
    limits: Arc<Mutex<PublicLimitState>>,
) {
    let mut welcomed = false;
    let mut snapshotted = false;
    let mut revision = 0_u64;
    while let Some(result) = socket.recv().await {
        let Ok(Message::Text(text)) = result else {
            break;
        };
        if text.len() > MAX_FRAME_BYTES {
            close(&mut socket, 1009, "frame_too_large").await;
            break;
        }
        if !admitted(&limits, principal.integration_id) {
            close(&mut socket, 1008, "rate_limited").await;
            break;
        }
        let Ok(message) = serde_json::from_str::<Incoming>(&text) else {
            close(&mut socket, 1007, "invalid_message").await;
            break;
        };
        match message {
            Incoming::Hello {
                protocol_version: 1,
            } if !welcomed => {
                welcomed = true;
                let mut allowlist: Vec<_> = principal.sensors.iter().cloned().collect();
                allowlist.sort();
                let reply = json!({"type":"integration.welcome","protocol_version":1,
                    "integration_id":principal.integration_id,"epoch":epoch,
                    "allowed_sensors":allowlist,"weather_entity":principal.weather_entity,
                    "max_frame_bytes":MAX_FRAME_BYTES,"heartbeat_seconds":30});
                if send(&mut socket, reply).await.is_err() {
                    break;
                }
            }
            Incoming::Snapshot {
                revision: 1,
                sensors,
            } if welcomed && !snapshotted && validate_sensors(&principal, &sensors) => {
                let outcome = store
                    .sensors(
                        &principal,
                        epoch,
                        SensorBatch {
                            revision: 1,
                            sensors,
                        },
                        true,
                    )
                    .await;
                if !matches!(outcome, Ok(IngestOutcome::Accepted)) {
                    close(&mut socket, 1008, "stale_session").await;
                    break;
                }
                revision = 1;
                snapshotted = true;
                if send(
                    &mut socket,
                    json!({"type":"integration.ack","revision":revision}),
                )
                .await
                .is_err()
                {
                    break;
                }
            }
            Incoming::State {
                revision: next,
                sensors,
            } if snapshotted && validate_sensors(&principal, &sensors) => {
                let outcome = store
                    .sensors(
                        &principal,
                        epoch,
                        SensorBatch {
                            revision: next,
                            sensors,
                        },
                        false,
                    )
                    .await;
                if !ack_outcome(&mut socket, outcome, next, &mut revision).await {
                    break;
                }
            }
            Incoming::Weather {
                revision: next,
                weather,
            } if snapshotted && validate_weather(&principal, &weather) => {
                let outcome = store
                    .weather(
                        &principal,
                        epoch,
                        WeatherBatch {
                            revision: next,
                            weather,
                        },
                    )
                    .await;
                if !ack_outcome(&mut socket, outcome, next, &mut revision).await {
                    break;
                }
            }
            Incoming::Heartbeat if snapshotted => {
                if !matches!(store.heartbeat(&principal, epoch).await, Ok(true)) {
                    close(&mut socket, 1008, "stale_session").await;
                    break;
                }
                if send(
                    &mut socket,
                    json!({"type":"integration.heartbeat","revision":revision}),
                )
                .await
                .is_err()
                {
                    break;
                }
            }
            _ => {
                close(&mut socket, 1008, "invalid_sequence_or_payload").await;
                break;
            }
        }
    }
}

fn admitted(limits: &Mutex<PublicLimitState>, integration_id: Uuid) -> bool {
    let now = Instant::now();
    let mut limits = limits.lock().expect("bridge admission mutex");
    let recent = limits
        .device_requests
        .entry(format!("ha:{integration_id}"))
        .or_default();
    recent.retain(|old| now.duration_since(*old) < Duration::from_secs(60));
    if recent.len() >= MAX_FRAMES_PER_MINUTE {
        return false;
    }
    recent.push(now);
    true
}

async fn ack_outcome(
    socket: &mut WebSocket,
    outcome: Result<IngestOutcome, crate::ha_bridge::BridgeStoreError>,
    next: u64,
    revision: &mut u64,
) -> bool {
    match outcome {
        Ok(IngestOutcome::Accepted) if next == *revision + 1 => {
            *revision = next;
        }
        Ok(IngestOutcome::Replay) if next == *revision => {}
        _ => {
            close(socket, 1008, "resync_required").await;
            return false;
        }
    }
    send(
        socket,
        json!({"type":"integration.ack","revision":*revision}),
    )
    .await
    .is_ok()
}

async fn send(socket: &mut WebSocket, value: serde_json::Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(value.to_string().into())).await
}

async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

#[cfg(test)]
#[path = "ha_bridge/tests.rs"]
mod tests;

#[path = "ha_bridge/read.rs"]
mod read;
pub(super) use read::{overview, weather};

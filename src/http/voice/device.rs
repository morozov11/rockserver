//! Device voice intent planning, surface capability validation, and command execution.

use std::time::Duration;

use axum::extract::ws::WebSocket;
use serde_json::json;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    device_control::{
        CommandId, CommandStatus, Device, DeviceCapability, DeviceId, DeviceRole, SurfaceKind,
        Timestamp,
    },
    device_control_auth::DeviceControlPrincipal,
    device_control_intent::{
        CurrentTarget, DirectoryDevice, DirectoryProjection, IntentErrorCode, IntentTarget,
        MediaIntentAction, ResolutionActor, ResolutionContext, ResolutionRequest, ResolutionResult,
        UserIntent,
    },
    personal_data::{CollectionChanges, FavouriteUpsert, PersonalDataError, SyncRequest},
    search::{SearchConstraints, normalize_query},
    voice::{Intent as VoiceIntent, VoiceCommand},
};

use super::super::state::AppState;
use super::protocol::{
    DEFAULT_STREAM_OPERATION_TIMEOUT, ValidatedVoiceStreamStart, VoiceStreamErrorCode,
    VoiceStreamServerEvent, send_stream_error, send_stream_event,
};

/// Validates that an authenticated device has declared voice input capabilities and the voice.main surface.
pub(super) fn validate_device_voice_start(
    state: &AppState,
    principal: DeviceControlPrincipal,
    start: &ValidatedVoiceStreamStart,
) -> Result<(), (VoiceStreamErrorCode, &'static str)> {
    if start.surface_id.as_deref() != Some("voice.main") {
        return Err((
            VoiceStreamErrorCode::ValidationFailed,
            "A device voice session requires the voice.main surface.",
        ));
    }
    let active = state
        .control_registry
        .active_for(principal.user_id, principal.device_id)
        .ok_or((
            VoiceStreamErrorCode::TargetOffline,
            "The source device is offline.",
        ))?;
    let manifest = &active.manifest;
    let voice_surface = manifest
        .surfaces
        .iter()
        .any(|surface| surface.surface_id == "voice.main" && surface.kind == SurfaceKind::Voice);
    let voice_input = manifest.capabilities.items.iter().any(|capability| {
        matches!(capability, DeviceCapability::VoiceInput { formats }
            if formats.iter().any(|format| format == "pcm16_mono_16000"))
    });
    if !manifest.roles.contains(&DeviceRole::VoiceEndpoint) || !voice_surface || !voice_input {
        return Err((
            VoiceStreamErrorCode::CapabilityNotSupported,
            "The source device does not support this voice surface.",
        ));
    }
    Ok(())
}

/// Orchestrates intent resolution, command routing, and terminal result polling for a device voice command.
pub(super) async fn finish_device_voice(
    socket: &mut WebSocket,
    state: &AppState,
    request_id: &str,
    start: &ValidatedVoiceStreamStart,
    principal: DeviceControlPrincipal,
    transcript: String,
) {
    let voice_command = match tokio::time::timeout(
        DEFAULT_STREAM_OPERATION_TIMEOUT,
        state
            .voice_command_interpreter
            .interpret(&transcript, &start.locale),
    )
    .await
    {
        Ok(Ok(command)) => command,
        Ok(Err(_)) | Err(_) => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::IntentResolutionFailed,
                "The voice intent could not be resolved.",
                json!({}),
            )
            .await;
            return;
        }
    };
    // Favourites are account data, not device playback: they skip the command
    // pipeline and use the same personal-data store as POST /api/v1/sync.
    if voice_command.intent == VoiceIntent::AddFavorite {
        add_favourite_voice(socket, state, request_id, &principal).await;
        return;
    }
    let intent = match device_user_intent(state, start, principal, transcript, voice_command).await
    {
        Ok(intent) => intent,
        Err((code, message)) => {
            let _ = send_stream_error(socket, request_id, code, message, json!({})).await;
            return;
        }
    };
    let Some(active) = state
        .control_registry
        .active_for(principal.user_id, principal.device_id)
    else {
        let _ = send_stream_error(
            socket,
            request_id,
            VoiceStreamErrorCode::TargetOffline,
            "The target device is offline.",
            json!({}),
        )
        .await;
        return;
    };
    let target = DeviceId(principal.device_id);
    let command_id = CommandId(Uuid::new_v4());
    let resolution = crate::device_control_intent::resolve(
        &ResolutionRequest {
            actor: ResolutionActor {
                user_id: principal.user_id,
                scopes: active.scopes.clone(),
            },
            directory: DirectoryProjection {
                owner_id: principal.user_id,
                devices: vec![DirectoryDevice {
                    device: Device {
                        device_id: target,
                        display_name: "Voice device".to_owned(),
                        device_type: "device".to_owned(),
                    },
                    manifest: active.manifest.clone(),
                    online: true,
                    runtime_state: state
                        .control_state_hub
                        .device_state(principal.user_id, target)
                        .map(|snapshot| snapshot.state),
                    entity_states: Vec::new(),
                }],
            },
            command_id,
            received_at: timestamp_now(),
            context: ResolutionContext {
                current_target: Some(CurrentTarget {
                    device_id: Some(target),
                    surface_id: start.surface_id.clone(),
                }),
                canonical_areas: Vec::new(),
            },
        },
        &intent,
    );
    let command = match resolution {
        ResolutionResult::Plan(mut plan) if plan.commands.len() == 1 => plan.commands.remove(0),
        ResolutionResult::Clarification(_) => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::ClarificationRequired,
                "The voice command needs clarification.",
                json!({}),
            )
            .await;
            return;
        }
        ResolutionResult::Confirmation(_) => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::UnsupportedIntent,
                "This voice intent is not supported.",
                json!({}),
            )
            .await;
            return;
        }
        ResolutionResult::Error(error) => {
            let (code, message) = map_intent_error(error.code);
            let _ = send_stream_error(socket, request_id, code, message, json!({})).await;
            return;
        }
        ResolutionResult::Plan(_) => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::InternalError,
                "The voice command could not be executed.",
                json!({}),
            )
            .await;
            return;
        }
    };
    let target_device_id = command.target.device_id;
    if let Err(error) = state
        .control_commands
        .submit(
            &state.control_registry,
            state.control_store.as_ref(),
            principal.user_id,
            principal.device_id,
            active.connection_id,
            request_id.to_owned(),
            command,
        )
        .await
    {
        let (code, message) = map_command_error(error.code);
        let _ = send_stream_error(socket, request_id, code, message, json!({})).await;
        return;
    }
    let Some(store) = state.control_store.as_ref() else {
        let _ = send_stream_error(
            socket,
            request_id,
            VoiceStreamErrorCode::PersistenceUnavailable,
            "Voice command state is temporarily unavailable.",
            json!({}),
        )
        .await;
        return;
    };
    let result = tokio::time::timeout(Duration::from_secs(31), async {
        loop {
            match store
                .load_command(principal.user_id, target_device_id, command_id)
                .await
            {
                Ok(Some(lifecycle)) => {
                    if let Some(result) = lifecycle.result {
                        break Ok(result);
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(_) => break Err(VoiceStreamErrorCode::PersistenceUnavailable),
            }
        }
    })
    .await;
    match result {
        Ok(Ok(result)) => {
            let _ = send_stream_event(
                socket,
                &VoiceStreamServerEvent::DeviceResult {
                    request_id: request_id.to_owned(),
                    status: result.status,
                },
            )
            .await;
        }
        Ok(Err(code)) => {
            let _ = send_stream_error(
                socket,
                request_id,
                code,
                "Voice command state is temporarily unavailable.",
                json!({}),
            )
            .await;
        }
        Err(_) => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::CommandTimeout,
                "The device command timed out.",
                json!({}),
            )
            .await;
        }
    }
}

/// Adds the device's currently reported station to the account favourites.
///
/// Uses the same personal-data store as `POST /api/v1/sync` (the RE-15 device star and
/// RockMobile), so every client observes the same collection. The write is idempotent:
/// an already-favourited station succeeds without a second record. Confirmation is sent
/// only after the stored outcome shows the live record.
async fn add_favourite_voice(
    socket: &mut WebSocket,
    state: &AppState,
    request_id: &str,
    principal: &DeviceControlPrincipal,
) {
    let Some(store) = state.personal_store.as_ref() else {
        let _ = send_stream_error(
            socket,
            request_id,
            VoiceStreamErrorCode::PersistenceUnavailable,
            "Favourite state is temporarily unavailable.",
            json!({}),
        )
        .await;
        return;
    };
    let target = DeviceId(principal.device_id);
    let Some(station_id) = current_station_id(state, principal.user_id, target) else {
        let _ = send_stream_error(
            socket,
            request_id,
            VoiceStreamErrorCode::StationNotFound,
            "No station is currently playing.",
            json!({}),
        )
        .await;
        return;
    };
    let is_live = |records: &[crate::personal_data::FavouriteRecord]| {
        records.iter().any(|record| {
            record.deleted_at.is_none() && record.station_id.as_deref() == Some(station_id.as_str())
        })
    };
    let snapshot = match store
        .synchronize(principal.user_id, SyncRequest::default())
        .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            send_favourite_store_failure(socket, request_id).await;
            return;
        }
    };
    if is_live(&snapshot.favourites) {
        let _ = send_stream_event(
            socket,
            &VoiceStreamServerEvent::DeviceResult {
                request_id: request_id.to_owned(),
                status: CommandStatus::Succeeded,
            },
        )
        .await;
        return;
    }
    let now_text = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("RFC3339 format is valid");
    let now = crate::personal_data::Timestamp::parse(now_text).expect("server timestamp is valid");
    let upsert = FavouriteUpsert {
        record_id: Uuid::new_v4(),
        station_id: station_id.clone(),
        added_at: now.clone(),
        updated_at: now,
    };
    let request = SyncRequest {
        since_revision: snapshot.server_revision,
        favourites: CollectionChanges {
            upserts: vec![upsert],
            deletes: Vec::new(),
        },
        history: CollectionChanges::default(),
    };
    match store.synchronize(principal.user_id, request).await {
        Ok(outcome) if is_live(&outcome.favourites) => {
            let _ = send_stream_event(
                socket,
                &VoiceStreamServerEvent::DeviceResult {
                    request_id: request_id.to_owned(),
                    status: CommandStatus::Succeeded,
                },
            )
            .await;
        }
        Ok(_) => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::InternalError,
                "The favourite could not be confirmed.",
                json!({}),
            )
            .await;
        }
        Err(PersonalDataError::FavouriteLimitExceeded) => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::FavouriteLimitExceeded,
                "The account favourites limit was reached.",
                json!({}),
            )
            .await;
        }
        Err(_) => send_favourite_store_failure(socket, request_id).await,
    }
}

/// Reports a transient personal-data failure without echoing store internals.
async fn send_favourite_store_failure(socket: &mut WebSocket, request_id: &str) {
    let _ = send_stream_error(
        socket,
        request_id,
        VoiceStreamErrorCode::PersistenceUnavailable,
        "Favourite state is temporarily unavailable.",
        json!({}),
    )
    .await;
}

/// Returns the station the device last reported as current, regardless of playback status:
/// a paused station is still the user's current station and may be favourited.
fn current_station_id(state: &AppState, user_id: Uuid, target: DeviceId) -> Option<String> {
    state
        .control_state_hub
        .device_state(user_id, target)
        .and_then(|snapshot| snapshot.state.playback)
        .and_then(|playback| playback.station_id)
        .filter(|station_id| !station_id.is_empty())
}

async fn device_user_intent(
    state: &AppState,
    start: &ValidatedVoiceStreamStart,
    principal: DeviceControlPrincipal,
    transcript: String,
    command: VoiceCommand,
) -> Result<UserIntent, (VoiceStreamErrorCode, &'static str)> {
    let target = Some(IntentTarget {
        device_id: Some(DeviceId(principal.device_id)),
        surface_id: None,
        area_id: None,
    });
    match command.intent {
        VoiceIntent::PlayRadio => {
            let query = normalize_query(transcript, start.locale.clone());
            let constraints = SearchConstraints {
                limit: 2,
                offset: 0,
                excluded_station_ids: start.exclude_station_ids.clone(),
            };
            let stations = match tokio::time::timeout(
                state.voice_command_timeout,
                state.search_service.search(&query, &constraints),
            )
            .await
            {
                Ok(Ok(stations)) => stations,
                Ok(Err(_)) => {
                    return Err((
                        VoiceStreamErrorCode::SearchUnavailable,
                        "Station search is temporarily unavailable.",
                    ));
                }
                Err(_) => {
                    return Err((
                        VoiceStreamErrorCode::SearchTimeout,
                        "Station search timed out.",
                    ));
                }
            };
            let Some(first) = stations.first() else {
                return Err((
                    VoiceStreamErrorCode::StationNotFound,
                    "No matching station was found.",
                ));
            };
            if stations
                .get(1)
                .is_some_and(|second| first.score == second.score)
            {
                return Err((
                    VoiceStreamErrorCode::ClarificationRequired,
                    "The station request needs clarification.",
                ));
            }
            Ok(UserIntent::PlayRadio {
                station_id: first.station.id.clone(),
                target,
            })
        }
        VoiceIntent::Stop => Ok(UserIntent::Media {
            action: MediaIntentAction::Stop,
            target,
        }),
        VoiceIntent::Resume => Ok(UserIntent::Media {
            action: MediaIntentAction::Play,
            target,
        }),
        VoiceIntent::NextStation => Ok(UserIntent::Media {
            action: MediaIntentAction::Next,
            target,
        }),
        VoiceIntent::PreviousStation => Ok(UserIntent::Media {
            action: MediaIntentAction::Previous,
            target,
        }),
        VoiceIntent::VolumeChange => match command.volume_delta {
            Some(delta) => Ok(UserIntent::Media {
                action: MediaIntentAction::ChangeVolume { delta },
                target,
            }),
            None => Err((
                VoiceStreamErrorCode::InvalidPayload,
                "The volume command is invalid.",
            )),
        },
        VoiceIntent::SetVolume => match command.volume_level {
            Some(level) => Ok(UserIntent::Media {
                action: MediaIntentAction::SetVolume { level },
                target,
            }),
            None => Err((
                VoiceStreamErrorCode::InvalidPayload,
                "The volume command is invalid.",
            )),
        },
        VoiceIntent::AddFavorite | VoiceIntent::Unknown => Err((
            VoiceStreamErrorCode::UnsupportedIntent,
            "This voice intent is not supported.",
        )),
    }
}

fn map_intent_error(code: IntentErrorCode) -> (VoiceStreamErrorCode, &'static str) {
    match code {
        IntentErrorCode::TargetOffline => (
            VoiceStreamErrorCode::TargetOffline,
            "The target device is offline.",
        ),
        IntentErrorCode::CapabilityNotSupported => (
            VoiceStreamErrorCode::CapabilityNotSupported,
            "The target does not support this command.",
        ),
        IntentErrorCode::Forbidden => (
            VoiceStreamErrorCode::Forbidden,
            "The voice command is not permitted.",
        ),
        IntentErrorCode::InvalidIntent => (
            VoiceStreamErrorCode::InvalidPayload,
            "The voice command is invalid.",
        ),
        IntentErrorCode::StateUnavailable | IntentErrorCode::UnsupportedSelector => (
            VoiceStreamErrorCode::UnsupportedIntent,
            "This voice intent is not supported.",
        ),
    }
}

fn map_command_error(code: &str) -> (VoiceStreamErrorCode, &'static str) {
    match code {
        "target_offline" => (
            VoiceStreamErrorCode::TargetOffline,
            "The target device is offline.",
        ),
        "capability_not_supported" => (
            VoiceStreamErrorCode::CapabilityNotSupported,
            "The target does not support this command.",
        ),
        "forbidden" => (
            VoiceStreamErrorCode::Forbidden,
            "The voice command is not permitted.",
        ),
        "invalid_payload" => (
            VoiceStreamErrorCode::InvalidPayload,
            "The device command is invalid.",
        ),
        "command_timeout" => (
            VoiceStreamErrorCode::CommandTimeout,
            "The device command timed out.",
        ),
        "duplicate_command" => (
            VoiceStreamErrorCode::DuplicateCommand,
            "The device command could not be admitted.",
        ),
        "too_many_in_flight" => (VoiceStreamErrorCode::TooManyInFlight, "The device is busy."),
        "persistence_unavailable" => (
            VoiceStreamErrorCode::PersistenceUnavailable,
            "Voice command state is temporarily unavailable.",
        ),
        _ => (
            VoiceStreamErrorCode::InternalError,
            "The voice command could not be executed.",
        ),
    }
}

fn timestamp_now() -> Timestamp {
    Timestamp::parse(
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .expect("RFC3339 format is valid"),
    )
    .expect("server timestamp is valid")
}

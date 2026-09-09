//! Transcript and streaming voice HTTP/WebSocket handlers.

use std::{
    collections::BTreeSet,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json,
    body::Body,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    device_control::{CommandStatus, Device, DeviceId, Timestamp},
    device_control_auth::{DeviceControlAuthenticationError, DeviceControlPrincipal},
    device_control_intent::{
        CurrentTarget, DirectoryDevice, DirectoryProjection, IntentErrorCode, IntentTarget,
        MediaIntentAction, ResolutionActor, ResolutionContext, ResolutionRequest, ResolutionResult,
        UserIntent,
    },
    search::{QueryParserInput, SearchConstraints, normalize_query},
    voice::{
        Intent as VoiceIntent, SpeechProviderError, SpeechRecognizerMode, SpeechStreamConfig,
        TranscriptUpdate, VoiceCommand,
    },
};

use super::{
    account,
    control_auth::authenticate_control_ingress,
    search::{SearchRequestDto, ValidatedSearchRequest},
    state::{AppState, PublicLimit, PublicLimitState},
    transport::{
        NormalizedQueryDto, StationResultDto, VoiceCommandResponseDto, error_response,
        parse_json_request, request_id, retry_after, unauthorized_response, with_request_id,
    },
};

const MAX_STREAM_AUDIO_CHUNK_BYTES: usize = 32 * 1024;
const MAX_STREAM_AUDIO_BYTES: usize = 2 * 1024 * 1024;
const MAX_STREAM_AUDIO_SECONDS: usize = 60;
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_WALL_TIMEOUT: Duration = Duration::from_secs(75);
const DEFAULT_STREAM_OPERATION_TIMEOUT: Duration = Duration::from_secs(15);
const VOICE_COMMAND_LIMIT: PublicLimit = PublicLimit {
    requests: 12,
    burst: 4,
};
const VOICE_UPGRADE_LIMIT: PublicLimit = PublicLimit {
    requests: 6,
    burst: 2,
};
/// Releases a reserved anonymous voice slot when a WebSocket session ends.
pub(super) struct VoiceSlot {
    pub(super) limiter: Arc<Mutex<PublicLimitState>>,
}

impl Drop for VoiceSlot {
    fn drop(&mut self) {
        if let Ok(mut state) = self.limiter.lock() {
            state.active_voice = state.active_voice.saturating_sub(1);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum VoiceStreamStartDto {
    Start {
        #[serde(default)]
        locale: Option<String>,
        sample_rate_hz: u32,
        #[serde(default)]
        surface_id: Option<String>,
        #[serde(default)]
        recognizer_mode: Option<String>,
        #[serde(default)]
        limit: Option<u8>,
        #[serde(default)]
        exclude_station_ids: Vec<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum VoiceStreamCommitDto {
    Commit,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum VoiceStreamCancelDto {
    Cancel,
}

struct ValidatedVoiceStreamStart {
    pub(super) locale: String,
    pub(super) sample_rate_hz: u32,
    pub(super) surface_id: Option<String>,
    pub(super) recognizer_mode: SpeechRecognizerMode,
    pub(super) limit: usize,
    pub(super) exclude_station_ids: BTreeSet<String>,
}

impl TryFrom<VoiceStreamStartDto> for ValidatedVoiceStreamStart {
    type Error = Map<String, Value>;

    fn try_from(value: VoiceStreamStartDto) -> Result<Self, Self::Error> {
        let VoiceStreamStartDto::Start {
            locale,
            sample_rate_hz,
            surface_id,
            recognizer_mode,
            limit,
            exclude_station_ids,
        } = value;
        let mut details = Map::new();
        let recognizer_mode = match recognizer_mode.as_deref().unwrap_or("buffered_v1") {
            "buffered_v1" => SpeechRecognizerMode::BufferedV1,
            "streaming_v3" => SpeechRecognizerMode::StreamingV3,
            _ => {
                details.insert(
                    "recognizer_mode".to_owned(),
                    json!("must be buffered_v1 or streaming_v3"),
                );
                SpeechRecognizerMode::default()
            }
        };
        if sample_rate_hz != 16_000 {
            details.insert("sample_rate_hz".to_owned(), json!("must equal 16000"));
        }
        if surface_id
            .as_deref()
            .is_some_and(|value| value != "voice.main")
        {
            details.insert("surface_id".to_owned(), json!("must equal voice.main"));
        }
        let validated = ValidatedSearchRequest::try_from(SearchRequestDto {
            query: "stream".to_owned(),
            locale,
            limit,
            exclude_station_ids,
        });
        match validated {
            Ok(validated) if details.is_empty() => Ok(Self {
                locale: validated.locale,
                sample_rate_hz,
                surface_id,
                recognizer_mode,
                limit: validated.limit.min(10),
                exclude_station_ids: validated.exclude_station_ids,
            }),
            Ok(_) => Err(details),
            Err(mut search_details) => {
                search_details.remove("query");
                details.extend(search_details);
                Err(details)
            }
        }
    }
}

enum StreamOperationError {
    Provider(SpeechProviderError),
    Timeout,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VoiceStreamServerEvent {
    Ready {
        request_id: String,
        audio_format: String,
        sample_rate_hz: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_device_id: Option<Uuid>,
        #[serde(skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    Transcript {
        request_id: String,
        transcript: String,
        is_final: bool,
    },
    Result {
        #[serde(flatten)]
        result: Box<VoiceStreamResultPayload>,
    },
    #[serde(rename = "result")]
    DeviceResult {
        request_id: String,
        status: CommandStatus,
    },
    Error {
        code: VoiceStreamErrorCode,
        message: String,
        request_id: String,
        details: Value,
    },
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum VoiceStreamErrorCode {
    ProtocolError,
    ValidationFailed,
    SpeechProviderUnavailable,
    SpeechProviderError,
    SpeechTimeout,
    SpeechNotRecognized,
    VoiceTimeout,
    AudioChunkInvalid,
    AudioTooLarge,
    Cancelled,
    IntentResolutionFailed,
    UnsupportedIntent,
    ClarificationRequired,
    StationNotFound,
    SearchTimeout,
    SearchUnavailable,
    TargetOffline,
    CapabilityNotSupported,
    Forbidden,
    InvalidPayload,
    CommandTimeout,
    DuplicateCommand,
    TooManyInFlight,
    PersistenceUnavailable,
    InternalError,
}

#[derive(Serialize)]
struct VoiceStreamResultPayload {
    pub(super) request_id: String,
    pub(super) transcript: String,
    pub(super) normalized_query: NormalizedQueryDto,
    pub(super) selected_station: Option<StationResultDto>,
    pub(super) stations: Vec<StationResultDto>,
}

/// JSON transport input for one already-recognized voice command.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VoiceCommandRequestDto {
    pub(super) transcript: String,
    #[serde(default)]
    pub(super) locale: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<u8>,
    #[serde(default)]
    pub(super) exclude_station_ids: Vec<String>,
}

struct ValidatedVoiceCommandRequest {
    pub(super) transcript: String,
    pub(super) locale: String,
    pub(super) limit: usize,
    pub(super) exclude_station_ids: BTreeSet<String>,
}

impl TryFrom<VoiceCommandRequestDto> for ValidatedVoiceCommandRequest {
    type Error = Map<String, Value>;

    fn try_from(value: VoiceCommandRequestDto) -> Result<Self, Self::Error> {
        let transcript = value.transcript;
        match ValidatedSearchRequest::try_from(SearchRequestDto {
            query: transcript,
            locale: value.locale,
            limit: value.limit,
            exclude_station_ids: value.exclude_station_ids,
        }) {
            Ok(validated) => Ok(Self {
                transcript: validated.query,
                locale: validated.locale,
                limit: validated.limit,
                exclude_station_ids: validated.exclude_station_ids,
            }),
            Err(mut details) => {
                if let Some(query_error) = details.remove("query") {
                    details.insert("transcript".to_owned(), query_error);
                }
                Err(details)
            }
        }
    }
}

/// Upgrades an anonymous or authenticated request to the streaming voice WebSocket.
pub(super) async fn voice_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !headers.contains_key(axum::http::header::AUTHORIZATION) {
        return public_voice_stream(State(state), headers, upgrade).await;
    }
    let request_id = request_id(&headers);
    if state.is_authorized(&headers) {
        return voice_stream_impl(state, headers, upgrade, request_id, None, None).await;
    }
    let Some(resolver) = state.control_session_resolver.as_ref() else {
        return unauthorized_response(&request_id);
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
                    "Device authentication is temporarily unavailable.",
                    &request_id,
                    json!({}),
                ),
                1,
            );
        }
    };
    voice_stream_impl(state, headers, upgrade, request_id, None, Some(principal)).await
}

/// Admits the approved anonymous WebSocket voice session without trusting forwarded headers.
pub(super) async fn public_voice_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let request_id = request_id(&headers);
    if let Err(response) =
        state.public_request_allowed("voice_stream", VOICE_UPGRADE_LIMIT, &request_id)
    {
        return *response;
    }
    let slot = match state.reserve_voice_slot(&request_id) {
        Ok(slot) => slot,
        Err(response) => return *response,
    };
    voice_stream_impl(state, headers, upgrade, request_id, Some(slot), None).await
}

/// Runs one authenticated or anonymous voice WebSocket session.
async fn voice_stream_impl(
    state: AppState,
    _headers: HeaderMap,
    upgrade: WebSocketUpgrade,
    request_id: String,
    slot: Option<VoiceSlot>,
    principal: Option<DeviceControlPrincipal>,
) -> Response {
    let socket_request_id = request_id.clone();
    let response = upgrade
        .max_message_size(MAX_STREAM_AUDIO_CHUNK_BYTES + 1024)
        .on_upgrade(move |socket| async move {
            let _slot = slot;
            run_voice_stream(socket, state, socket_request_id, principal).await
        });
    with_request_id(response, &request_id)
}

/// Processes audio, transcript updates, and the final station search.
async fn run_voice_stream(
    mut socket: WebSocket,
    state: AppState,
    request_id: String,
    principal: Option<DeviceControlPrincipal>,
) {
    let Some(Ok(Message::Text(start_message))) =
        tokio::time::timeout(STREAM_IDLE_TIMEOUT, socket.recv())
            .await
            .ok()
            .flatten()
    else {
        let _ = send_stream_error(
            &mut socket,
            &request_id,
            VoiceStreamErrorCode::ProtocolError,
            "The first WebSocket message must be a JSON start event.",
            json!({}),
        )
        .await;
        return;
    };
    let start = match parse_stream_start(&start_message) {
        Ok(start) => start,
        Err(details) => {
            let _ = send_stream_error(
                &mut socket,
                &request_id,
                VoiceStreamErrorCode::ValidationFailed,
                "Streaming session validation failed.",
                details,
            )
            .await;
            return;
        }
    };
    if let Some(principal) = principal
        && let Err((code, message)) = validate_device_voice_start(&state, principal, &start)
    {
        let _ = send_stream_error(&mut socket, &request_id, code, message, json!({})).await;
        return;
    }

    let mut session = match tokio::time::timeout(
        DEFAULT_STREAM_OPERATION_TIMEOUT,
        state.speech_recognizers.start(
            start.recognizer_mode,
            SpeechStreamConfig {
                locale: start.locale.clone(),
                sample_rate_hz: start.sample_rate_hz,
            },
        ),
    )
    .await
    {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            log_speech_error(&request_id, &error);
            let _ = send_stream_error(
                &mut socket,
                &request_id,
                VoiceStreamErrorCode::SpeechProviderUnavailable,
                "Streaming speech recognition is unavailable.",
                json!({}),
            )
            .await;
            return;
        }
        Err(_) => {
            let _ = send_stream_error(
                &mut socket,
                &request_id,
                VoiceStreamErrorCode::SpeechTimeout,
                "Streaming speech provider timed out.",
                json!({"timeout_ms": DEFAULT_STREAM_OPERATION_TIMEOUT.as_millis()}),
            )
            .await;
            return;
        }
    };
    if send_stream_event(
        &mut socket,
        &VoiceStreamServerEvent::Ready {
            request_id: request_id.clone(),
            audio_format: "pcm_s16le".to_owned(),
            sample_rate_hz: start.sample_rate_hz,
            source_device_id: principal.map(|principal| principal.device_id),
            surface_id: principal.and_then(|_| start.surface_id.clone()),
        },
    )
    .await
    .is_err()
    {
        return;
    }

    let started_at = tokio::time::Instant::now();
    let mut audio_bytes = 0usize;
    let mut last_final_transcript = None;
    while started_at.elapsed() < STREAM_WALL_TIMEOUT {
        let Some(message) = tokio::time::timeout(STREAM_IDLE_TIMEOUT, socket.recv())
            .await
            .ok()
            .flatten()
        else {
            let _ = send_stream_error(
                &mut socket,
                &request_id,
                VoiceStreamErrorCode::VoiceTimeout,
                "Voice session timed out.",
                json!({"timeout_ms": STREAM_IDLE_TIMEOUT.as_millis()}),
            )
            .await;
            return;
        };
        match message {
            Ok(Message::Binary(audio)) => {
                if audio.is_empty()
                    || audio.len() > MAX_STREAM_AUDIO_CHUNK_BYTES
                    || audio.len() % 2 != 0
                {
                    let _ = send_stream_error(
                        &mut socket,
                        &request_id,
                        VoiceStreamErrorCode::AudioChunkInvalid,
                        "Audio frames must be bounded PCM16 data.",
                        json!({"max_chunk_bytes": MAX_STREAM_AUDIO_CHUNK_BYTES}),
                    )
                    .await;
                    return;
                }
                audio_bytes = audio_bytes.saturating_add(audio.len());
                if audio_bytes > MAX_STREAM_AUDIO_BYTES
                    || audio_bytes / 2 / 16_000 > MAX_STREAM_AUDIO_SECONDS
                {
                    let _ = send_stream_error(
                        &mut socket,
                        &request_id,
                        VoiceStreamErrorCode::AudioTooLarge,
                        "Streaming session audio limit was exceeded.",
                        json!({"max_bytes": MAX_STREAM_AUDIO_BYTES}),
                    )
                    .await;
                    return;
                }
                match speech_operation(session.push_audio(&audio)).await {
                    Ok(updates) => {
                        if let Some(transcript) = newest_final_transcript(&updates) {
                            last_final_transcript = Some(transcript);
                        }
                        if send_transcript_updates(&mut socket, &request_id, updates)
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        send_speech_failure(&mut socket, &request_id, error).await;
                        return;
                    }
                }
            }
            Ok(Message::Text(text)) if is_cancel_event(&text) => {
                drop(session);
                let _ = send_stream_error(
                    &mut socket,
                    &request_id,
                    VoiceStreamErrorCode::Cancelled,
                    "Voice session was cancelled.",
                    json!({}),
                )
                .await;
                return;
            }
            Ok(Message::Text(text)) if is_commit_event(&text) => {
                let updates = match speech_operation(session.finish()).await {
                    Ok(updates) => updates,
                    Err(error) => {
                        send_speech_failure(&mut socket, &request_id, error).await;
                        return;
                    }
                };
                let final_transcript = newest_final_transcript(&updates).or(last_final_transcript);
                if send_transcript_updates(&mut socket, &request_id, updates)
                    .await
                    .is_err()
                {
                    return;
                }
                let Some(transcript) = final_transcript else {
                    let _ = send_stream_error(
                        &mut socket,
                        &request_id,
                        VoiceStreamErrorCode::SpeechNotRecognized,
                        "No final speech transcript was recognized.",
                        json!({}),
                    )
                    .await;
                    return;
                };
                if let Some(principal) = principal {
                    finish_device_voice(
                        &mut socket,
                        &state,
                        &request_id,
                        &start,
                        principal,
                        transcript,
                    )
                    .await;
                } else {
                    finish_stream_search(&mut socket, &state, &request_id, &start, transcript)
                        .await;
                }
                return;
            }
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(Message::Ping(payload)) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    return;
                }
            }
            Ok(Message::Pong(_)) => {}
            _ => {
                let _ = send_stream_error(
                    &mut socket,
                    &request_id,
                    VoiceStreamErrorCode::ProtocolError,
                    "Expected a binary audio chunk or JSON commit event.",
                    json!({}),
                )
                .await;
                return;
            }
        }
    }
    let _ = send_stream_error(
        &mut socket,
        &request_id,
        VoiceStreamErrorCode::VoiceTimeout,
        "Voice session timed out.",
        json!({"timeout_ms": STREAM_WALL_TIMEOUT.as_millis()}),
    )
    .await;
}

fn newest_final_transcript(updates: &[TranscriptUpdate]) -> Option<String> {
    updates
        .iter()
        .rev()
        .find(|update| update.is_final)
        .map(|update| update.transcript.trim().to_owned())
        .filter(|transcript| !transcript.is_empty())
}

/// Resolves an already-recognized voice transcript through the existing search service.
///
/// The route does not accept audio and does not call an STT provider. This keeps provider
/// credentials and audio-upload policy outside the stable JSON command contract.
pub(super) async fn voice_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !headers.contains_key(axum::http::header::AUTHORIZATION) {
        return public_voice_command(State(state), headers, body).await;
    }
    let request_id = request_id(&headers);
    if !state.is_authorized(&headers)
        && account::match_native_session(&state, &headers, &request_id)
            .await
            .is_none()
    {
        return unauthorized_response(&request_id);
    }
    voice_command_impl(state, headers, body, request_id, 50).await
}

/// Serves the approved anonymous, transcript-only voice command operation.
pub(super) async fn public_voice_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let request_id = request_id(&headers);
    if let Err(response) =
        state.public_request_allowed("voice_command", VOICE_COMMAND_LIMIT, &request_id)
    {
        return *response;
    }
    voice_command_impl(state, headers, body, request_id, 10).await
}

async fn voice_command_impl(
    state: AppState,
    headers: HeaderMap,
    body: Body,
    request_id: String,
    max_limit: u8,
) -> Response {
    let request =
        match parse_json_request::<VoiceCommandRequestDto>(&headers, body, &request_id).await {
            Ok(request) => request,
            Err(response) => return response,
        };
    let validated = match ValidatedVoiceCommandRequest::try_from(request) {
        Ok(request) => request,
        Err(details) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "Request validation failed.",
                &request_id,
                Value::Object(details),
            );
        }
    };
    if validated.limit > usize::from(max_limit) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "Request validation failed.",
            &request_id,
            json!({"limit": format!("must be between 1 and {max_limit}")}),
        );
    }
    let constraints = SearchConstraints {
        limit: validated.limit,
        excluded_station_ids: validated.exclude_station_ids,
    };
    let outcome = match tokio::time::timeout(
        state.voice_command_timeout,
        state.search_service.interpret_and_search_private(
            QueryParserInput {
                query: validated.transcript.clone(),
                locale: validated.locale,
            },
            &constraints,
        ),
    )
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_error)) => {
            tracing::warn!(%request_id, endpoint = "voice_command", "public-safe voice command failure");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An unexpected server error occurred.",
                &request_id,
                json!({}),
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::GATEWAY_TIMEOUT,
                "search_timeout",
                "Voice command search timed out.",
                &request_id,
                json!({"timeout_ms": state.voice_command_timeout.as_millis()}),
            );
        }
    };

    let stations = outcome
        .stations
        .iter()
        .map(StationResultDto::from)
        .collect::<Vec<_>>();
    let selected_station = stations.first().cloned();
    tracing::info!(%request_id, endpoint = "voice_command", status = 200, stations = stations.len(), "public request completed");
    with_request_id(
        Json(VoiceCommandResponseDto {
            request_id: request_id.clone(),
            transcript: validated.transcript,
            normalized_query: NormalizedQueryDto::from(outcome.query),
            selected_station,
            stations,
        })
        .into_response(),
        &request_id,
    )
}

fn parse_stream_start(text: &str) -> Result<ValidatedVoiceStreamStart, Value> {
    let request = serde_json::from_str::<VoiceStreamStartDto>(text)
        .map_err(|error| json!({"start": format!("must be valid JSON: {error}")}))?;
    ValidatedVoiceStreamStart::try_from(request).map_err(Value::Object)
}

fn is_commit_event(text: &str) -> bool {
    serde_json::from_str::<VoiceStreamCommitDto>(text).is_ok()
}

fn is_cancel_event(text: &str) -> bool {
    serde_json::from_str::<VoiceStreamCancelDto>(text).is_ok()
}

fn validate_device_voice_start(
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
    let voice_surface = manifest.surfaces.iter().any(|surface| {
        surface.surface_id == "voice.main"
            && surface.kind == crate::device_control::SurfaceKind::Voice
    });
    let voice_input = manifest.capabilities.items.iter().any(|capability| {
        matches!(capability, crate::device_control::DeviceCapability::VoiceInput { formats }
            if formats.iter().any(|format| format == "pcm16_mono_16000"))
    });
    if !manifest
        .roles
        .contains(&crate::device_control::DeviceRole::VoiceEndpoint)
        || !voice_surface
        || !voice_input
    {
        return Err((
            VoiceStreamErrorCode::CapabilityNotSupported,
            "The source device does not support this voice surface.",
        ));
    }
    Ok(())
}

async fn finish_device_voice(
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
    let command_id = crate::device_control::CommandId(Uuid::new_v4());
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
        VoiceIntent::NextStation
        | VoiceIntent::PreviousStation
        | VoiceIntent::VolumeChange
        | VoiceIntent::Unknown => Err((
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

async fn speech_operation<T>(
    operation: impl Future<Output = Result<T, SpeechProviderError>>,
) -> Result<T, StreamOperationError> {
    match tokio::time::timeout(DEFAULT_STREAM_OPERATION_TIMEOUT, operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(StreamOperationError::Provider(error)),
        Err(_) => Err(StreamOperationError::Timeout),
    }
}

async fn send_transcript_updates(
    socket: &mut WebSocket,
    request_id: &str,
    updates: Vec<TranscriptUpdate>,
) -> Result<(), axum::Error> {
    for update in updates {
        send_stream_event(
            socket,
            &VoiceStreamServerEvent::Transcript {
                request_id: request_id.to_owned(),
                transcript: update.transcript,
                is_final: update.is_final,
            },
        )
        .await?;
    }
    Ok(())
}

async fn send_speech_failure(
    socket: &mut WebSocket,
    request_id: &str,
    error: StreamOperationError,
) {
    match error {
        StreamOperationError::Provider(error) => {
            log_speech_error(request_id, &error);
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::SpeechProviderError,
                "Streaming speech recognition failed.",
                json!({}),
            )
            .await;
        }
        StreamOperationError::Timeout => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::SpeechTimeout,
                "Streaming speech provider timed out.",
                json!({"timeout_ms": DEFAULT_STREAM_OPERATION_TIMEOUT.as_millis()}),
            )
            .await;
        }
    }
}

fn log_speech_error(request_id: &str, _error: &SpeechProviderError) {
    tracing::warn!(%request_id, "streaming speech recognition failed");
}

async fn finish_stream_search(
    socket: &mut WebSocket,
    state: &AppState,
    request_id: &str,
    start: &ValidatedVoiceStreamStart,
    transcript: String,
) {
    tracing::info!(
        %request_id,
        locale = %start.locale,
        limit = start.limit,
        audio_search = true,
        "voice transcript search started"
    );
    let constraints = SearchConstraints {
        limit: start.limit,
        excluded_station_ids: start.exclude_station_ids.clone(),
    };
    let outcome = match tokio::time::timeout(
        state.voice_command_timeout,
        state.search_service.interpret_and_search_private(
            QueryParserInput {
                query: transcript.clone(),
                locale: start.locale.clone(),
            },
            &constraints,
        ),
    )
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_)) => {
            tracing::error!(%request_id, "streaming voice search failed");
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::InternalError,
                "An unexpected server error occurred.",
                json!({}),
            )
            .await;
            return;
        }
        Err(_) => {
            let _ = send_stream_error(
                socket,
                request_id,
                VoiceStreamErrorCode::SearchTimeout,
                "Voice command search timed out.",
                json!({"timeout_ms": state.voice_command_timeout.as_millis()}),
            )
            .await;
            return;
        }
    };
    let stations = outcome
        .stations
        .iter()
        .map(StationResultDto::from)
        .collect::<Vec<_>>();
    let selected_station = stations.first().cloned();
    tracing::info!(%request_id, stations = stations.len(), "voice transcript search completed");
    let _ = send_stream_event(
        socket,
        &VoiceStreamServerEvent::Result {
            result: Box::new(VoiceStreamResultPayload {
                request_id: request_id.to_owned(),
                transcript,
                normalized_query: NormalizedQueryDto::from(outcome.query),
                selected_station,
                stations,
            }),
        },
    )
    .await;
}

async fn send_stream_error(
    socket: &mut WebSocket,
    request_id: &str,
    code: VoiceStreamErrorCode,
    message: &str,
    details: Value,
) -> Result<(), axum::Error> {
    send_stream_event(
        socket,
        &VoiceStreamServerEvent::Error {
            code,
            message: message.to_owned(),
            request_id: request_id.to_owned(),
            details,
        },
    )
    .await
}

async fn send_stream_event(
    socket: &mut WebSocket,
    event: &VoiceStreamServerEvent,
) -> Result<(), axum::Error> {
    let payload = serde_json::to_string(event)
        .expect("stream server events contain only serializable transport values");
    socket.send(Message::Text(payload.into())).await
}

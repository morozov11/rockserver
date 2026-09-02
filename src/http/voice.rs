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

use crate::{
    search::{QueryParserInput, SearchConstraints},
    voice::{SpeechProviderError, SpeechRecognizerMode, SpeechStreamConfig, TranscriptUpdate},
};

use super::{
    account,
    search::{SearchRequestDto, ValidatedSearchRequest},
    state::{AppState, PublicLimit, PublicLimitState},
    transport::{
        NormalizedQueryDto, StationResultDto, VoiceCommandResponseDto, error_response,
        parse_json_request, request_id, unauthorized_response, with_request_id,
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

struct ValidatedVoiceStreamStart {
    pub(super) locale: String,
    pub(super) sample_rate_hz: u32,
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
    Error {
        code: String,
        message: String,
        request_id: String,
        details: Value,
    },
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
    if !state.is_authorized(&headers)
        && account::match_native_session(&state, &headers, &request_id)
            .await
            .is_none()
    {
        return unauthorized_response(&request_id);
    }
    voice_stream_impl(state, headers, upgrade, request_id, None).await
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
    voice_stream_impl(state, headers, upgrade, request_id, Some(slot)).await
}

/// Runs one authenticated or anonymous voice WebSocket session.
async fn voice_stream_impl(
    state: AppState,
    _headers: HeaderMap,
    upgrade: WebSocketUpgrade,
    request_id: String,
    slot: Option<VoiceSlot>,
) -> Response {
    let socket_request_id = request_id.clone();
    let response = upgrade
        .max_message_size(MAX_STREAM_AUDIO_CHUNK_BYTES + 1024)
        .on_upgrade(move |socket| async move {
            let _slot = slot;
            run_voice_stream(socket, state, socket_request_id).await
        });
    with_request_id(response, &request_id)
}

/// Processes audio, transcript updates, and the final station search.
async fn run_voice_stream(mut socket: WebSocket, state: AppState, request_id: String) {
    let Some(Ok(Message::Text(start_message))) =
        tokio::time::timeout(STREAM_IDLE_TIMEOUT, socket.recv())
            .await
            .ok()
            .flatten()
    else {
        let _ = send_stream_error(
            &mut socket,
            &request_id,
            "protocol_error",
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
                "validation_failed",
                "Streaming session validation failed.",
                details,
            )
            .await;
            return;
        }
    };

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
                "speech_provider_unavailable",
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
                "speech_timeout",
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
                "voice_timeout",
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
                        "audio_chunk_invalid",
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
                        "audio_too_large",
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
                        "speech_not_recognized",
                        "No final speech transcript was recognized.",
                        json!({}),
                    )
                    .await;
                    return;
                };
                finish_stream_search(&mut socket, &state, &request_id, &start, transcript).await;
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
                    "protocol_error",
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
        "voice_timeout",
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
        state.search_service.interpret_and_search(
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
                "speech_provider_error",
                "Streaming speech recognition failed.",
                json!({}),
            )
            .await;
        }
        StreamOperationError::Timeout => {
            let _ = send_stream_error(
                socket,
                request_id,
                "speech_timeout",
                "Streaming speech provider timed out.",
                json!({"timeout_ms": DEFAULT_STREAM_OPERATION_TIMEOUT.as_millis()}),
            )
            .await;
        }
    }
}

fn log_speech_error(request_id: &str, error: &SpeechProviderError) {
    tracing::warn!(%request_id, %error, "streaming speech provider failed");
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
        transcript = %transcript,
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
        state.search_service.interpret_and_search(
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
        Ok(Err(error)) => {
            tracing::error!(%error, %request_id, "streaming voice search failed");
            let _ = send_stream_error(
                socket,
                request_id,
                "internal_error",
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
                "search_timeout",
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
    tracing::info!(
        %request_id,
        transcript = %transcript,
        terms = ?outcome.query.terms,
        tags = ?outcome.query.tags,
        language = ?outcome.query.language,
        country_code = ?outcome.query.country_code,
        stations = stations.len(),
        selected_station = ?selected_station.as_ref().map(|station| station.name.as_str()),
        "voice transcript search completed"
    );
    for (rank, station) in stations.iter().enumerate() {
        tracing::info!(
            %request_id,
            rank,
            station_id = %station.id,
            station = %station.name,
            country_code = ?station.country_code,
            stream_url = %station.stream_url,
            "voice station candidate"
        );
    }
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
    code: &str,
    message: &str,
    details: Value,
) -> Result<(), axum::Error> {
    send_stream_event(
        socket,
        &VoiceStreamServerEvent::Error {
            code: code.to_owned(),
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

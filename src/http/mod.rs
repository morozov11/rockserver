//! Axum routes and transport DTOs for the RockServer HTTP API.

use std::{
    collections::BTreeSet,
    future::Future,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::{
    search::{
        InMemoryStationRepository, QueryParserInput, RankedStation, SearchConstraints,
        SearchService, StationHealth, StationRepository, UnavailableStationRepository,
    },
    voice::{
        SpeechProviderError, SpeechRecognizerMode, SpeechRecognizers, SpeechStreamConfig,
        StreamingSpeechRecognizer, TranscriptUpdate, UnavailableSpeechRecognizer,
    },
};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Deterministic credential used only by convenience routers in offline tests and examples.
///
/// Production startup must supply a unique secret through [`router_with_services_and_bearer_token`].
pub const TEST_API_BEARER_TOKEN: &str = "rockserver-offline-test-token";

const MAX_JSON_REQUEST_BODY_BYTES: usize = 64 * 1024;
const REQUEST_ID_HEADER: &str = "x-request-id";
const MAX_STREAM_AUDIO_CHUNK_BYTES: usize = 64 * 1024;
const MAX_STREAM_AUDIO_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_STREAM_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum duration the voice-command transport waits for query interpretation and search.
pub const DEFAULT_VOICE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

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
    let repository: Arc<dyn StationRepository + Send + Sync> =
        match InMemoryStationRepository::with_builtin_catalog() {
            Ok(repository) => Arc::new(repository),
            Err(error) => Arc::new(UnavailableStationRepository::from_preflight_error(error)),
        };
    router_with_repository(repository)
}

/// Creates the application router with a supplied station repository backend.
pub fn router_with_repository(repository: Arc<dyn StationRepository + Send + Sync>) -> Router {
    router_with_search_service(SearchService::new(repository))
}

/// Creates the application router with fully configured search orchestration.
pub fn router_with_search_service(search_service: SearchService) -> Router {
    router_with_services(
        search_service,
        Arc::new(UnavailableSpeechRecognizer),
        DEFAULT_VOICE_COMMAND_TIMEOUT,
    )
}

/// Creates the application router with an explicit voice-command service timeout.
///
/// The timeout covers query interpretation and repository search only. Audio capture and speech
/// recognition are intentionally outside this JSON transport boundary.
pub fn router_with_search_service_and_voice_timeout(
    search_service: SearchService,
    voice_command_timeout: Duration,
) -> Router {
    router_with_services(
        search_service,
        Arc::new(UnavailableSpeechRecognizer),
        voice_command_timeout,
    )
}

/// Creates the router with explicit search and streaming speech-recognition services.
///
/// Tests and production startup use this boundary to select a provider without coupling the
/// public WebSocket protocol to Yandex SpeechKit or OpenAI Realtime transport details.
pub fn router_with_services(
    search_service: SearchService,
    speech_recognizer: Arc<dyn StreamingSpeechRecognizer>,
    voice_command_timeout: Duration,
) -> Router {
    router_with_services_and_bearer_token(
        search_service,
        speech_recognizer,
        voice_command_timeout,
        TEST_API_BEARER_TOKEN,
    )
}

/// Creates the router with explicit services and the Bearer credential required by application APIs.
///
/// Liveness and readiness endpoints intentionally remain unauthenticated so local process
/// supervision can observe the service. All `/v1` and `/api/v1` application endpoints, including
/// the WebSocket handshake, require `Authorization: Bearer <token>`.
pub fn router_with_services_and_bearer_token(
    search_service: SearchService,
    speech_recognizer: Arc<dyn StreamingSpeechRecognizer>,
    voice_command_timeout: Duration,
    api_bearer_token: impl Into<String>,
) -> Router {
    router_with_speech_recognizers_and_bearer_token(
        search_service,
        SpeechRecognizers::same(speech_recognizer),
        voice_command_timeout,
        api_bearer_token,
    )
}

/// Creates the router with the recognizers selectable by each voice WebSocket session.
///
/// The compatibility constructor above uses one recognizer for both modes, which keeps existing
/// integrations and deterministic tests unchanged.
pub fn router_with_speech_recognizers_and_bearer_token(
    search_service: SearchService,
    speech_recognizers: SpeechRecognizers,
    voice_command_timeout: Duration,
    api_bearer_token: impl Into<String>,
) -> Router {
    let state = AppState {
        search_service,
        speech_recognizers,
        voice_command_timeout,
        api_bearer_token: api_bearer_token.into(),
    };

    Router::new()
        .route("/admin", get(admin_console))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/search", post(search))
        .route("/v1/search", post(search))
        .route("/api/v1/voice/command", post(voice_command))
        .route("/v1/voice/command", post(voice_command))
        .route("/api/v1/voice/stream", get(voice_stream))
        .route("/v1/voice/stream", get(voice_stream))
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
}

/// Serves the local administration-console preview.
///
/// The page keeps the configured Bearer credential only in the browser tab's memory and uses it
/// to call the existing protected API. It deliberately does not implement administrator accounts,
/// sessions, or any state-changing administrative operation.
async fn admin_console() -> Html<&'static str> {
    Html(ADMIN_CONSOLE_HTML)
}

const ADMIN_CONSOLE_HTML: &str = r#"<!doctype html>
<html lang="ru">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>RockServer — администрирование</title>
  <style>
    :root { color-scheme: dark; font-family: Inter, system-ui, sans-serif; background: #10151b; color: #edf3f8; }
    body { max-width: 1040px; margin: 0 auto; padding: 42px 24px 72px; }
    header { display: flex; align-items: center; justify-content: space-between; gap: 20px; margin-bottom: 32px; }
    h1 { font-size: clamp(1.7rem, 4vw, 2.5rem); margin: 0; } h2 { margin: 0 0 14px; font-size: 1.1rem; }
    .brand { color: #77d4ff; font-weight: 800; letter-spacing: .08em; font-size: .82rem; }
    .notice, .panel { background: #18212b; border: 1px solid #2c3d4d; border-radius: 14px; }
    .notice { padding: 14px 16px; color: #b9c9d7; margin-bottom: 22px; }
    .grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; margin-bottom: 22px; }
    .panel { padding: 20px; } .metric { font-size: 1.8rem; font-weight: 750; color: #77d4ff; }
    .muted { color: #a8b8c6; font-size: .92rem; } label { display: block; margin: 12px 0 7px; font-weight: 650; }
    input { box-sizing: border-box; width: 100%; padding: 12px; border: 1px solid #405464; border-radius: 8px; background: #0e141a; color: inherit; }
    button { border: 0; border-radius: 8px; padding: 12px 16px; background: #43b8ed; color: #06131a; font-weight: 750; cursor: pointer; }
    button.secondary { background: #2a3946; color: #e8f2f8; } .actions { display: flex; gap: 10px; margin-top: 14px; }
    #workspace { display: none; } #message { min-height: 1.3em; margin-top: 12px; } .error { color: #ff9b9b; } .ok { color: #9de6ad; }
    table { width: 100%; border-collapse: collapse; margin-top: 14px; } th, td { text-align: left; padding: 11px 8px; border-bottom: 1px solid #2b3b49; vertical-align: top; }
    th { color: #a8c2d3; font-size: .8rem; text-transform: uppercase; } a { color: #81d8ff; } code { color: #bed5e5; }
    @media (max-width: 680px) { body { padding: 26px 16px; } header { display: block; } .grid { grid-template-columns: 1fr; } table { font-size: .85rem; } }
  </style>
</head>
<body>
  <header><div><div class="brand">ROCKSERVER</div><h1>Панель администратора</h1></div><span class="muted">локальный предпросмотр</span></header>
  <p class="notice">Это первый просмотр интерфейса: токен не сохраняется в браузере и используется только для запросов в этой вкладке. Управление пользователями и изменение каталога пока не реализованы.</p>
  <section id="login" class="panel"><h2>Подключить консоль</h2><p class="muted">Введите значение <code>ROCKSERVER_API_BEARER_TOKEN</code> текущего сервера.</p><label for="token">Bearer token</label><input id="token" type="password" autocomplete="off" placeholder="Токен из переменной окружения"><div class="actions"><button id="connect">Подключиться</button></div><div id="message" aria-live="polite"></div></section>
  <main id="workspace">
    <section class="grid"><article class="panel"><h2>Сервис</h2><div class="metric" id="ready">—</div><span class="muted">готовность каталога</span></article><article class="panel"><h2>Доступ</h2><div class="metric">Bearer</div><span class="muted">токен только в памяти</span></article><article class="panel"><h2>Каталог</h2><div class="metric" id="result-count">—</div><span class="muted">найдено последним запросом</span></article></section>
    <section class="panel"><h2>Поиск по станциям</h2><label for="query">Запрос</label><input id="query" value="rock" maxlength="500"><div class="actions"><button id="search">Найти станции</button><button id="disconnect" class="secondary">Отключиться</button></div><div id="search-message" aria-live="polite"></div><table><thead><tr><th>Станция</th><th>Теги</th><th>Страна</th><th>Поток</th></tr></thead><tbody id="stations"><tr><td colspan="4" class="muted">Введите запрос и нажмите «Найти станции».</td></tr></tbody></table></section>
  </main>
  <script>
    let token = '';
    const $ = id => document.getElementById(id);
    const setMessage = (id, text, error = false) => { const node = $(id); node.textContent = text; node.className = error ? 'error' : 'ok'; };
    const escape = value => String(value).replace(/[&<>'"]/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', "'": '&#39;', '"': '&quot;' })[char]);
    async function api(path, options = {}) { const headers = new Headers(options.headers || {}); headers.set('Authorization', `Bearer ${token}`); return fetch(path, { ...options, headers }); }
    async function readiness() { const response = await fetch('/health/ready'); $('ready').textContent = response.ok ? 'Готов' : 'Недоступен'; }
    async function runSearch() {
      const query = $('query').value.trim(); if (!query) { setMessage('search-message', 'Введите поисковый запрос.', true); return; }
      const response = await api('/v1/search', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ query, locale: 'en-US', limit: 20 }) });
      if (!response.ok) { setMessage('search-message', response.status === 401 ? 'Токен не принят сервером.' : 'Не удалось выполнить поиск.', true); return; }
      const data = await response.json(); const stations = data.stations || []; $('result-count').textContent = stations.length;
      $('stations').innerHTML = stations.length ? stations.map(station => `<tr><td><strong>${escape(station.name)}</strong><br><span class="muted">${escape(station.id)}</span></td><td>${escape(station.tags.join(', '))}</td><td>${escape(station.country_code || '—')}</td><td><a href="${escape(station.stream_url)}" target="_blank" rel="noopener">открыть</a></td></tr>`).join('') : '<tr><td colspan="4" class="muted">Станции не найдены.</td></tr>';
      setMessage('search-message', `Запрос обработан: ${data.request_id}`);
    }
    $('connect').addEventListener('click', async () => { token = $('token').value; if (!token) { setMessage('message', 'Введите токен.', true); return; } const response = await api('/v1/search', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ query: 'rock', limit: 1 }) }); if (!response.ok) { const unauthorized = response.status === 401; setMessage('message', unauthorized ? 'Токен не принят сервером.' : `Сервер ответил HTTP ${response.status}; токен не проверен.`, true); if (unauthorized) token = ''; return; } $('login').style.display = 'none'; $('workspace').style.display = 'block'; await readiness(); runSearch(); });
    $('search').addEventListener('click', runSearch); $('query').addEventListener('keydown', event => { if (event.key === 'Enter') runSearch(); });
    $('disconnect').addEventListener('click', () => { token = ''; $('token').value = ''; $('workspace').style.display = 'none'; $('login').style.display = 'block'; setMessage('message', 'Токен очищен из памяти вкладки.'); });
  </script>
</body>
</html>"#;

/// Upgrades a request to the provider-neutral streaming voice protocol.
async fn voice_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let request_id = request_id(&headers);
    if !state.is_authorized(&headers) {
        return unauthorized_response(&request_id);
    }
    tracing::info!(%request_id, "voice websocket upgrade requested");
    let socket_request_id = request_id.clone();
    let response = upgrade
        .max_message_size(MAX_STREAM_AUDIO_CHUNK_BYTES + 1024)
        .on_upgrade(move |socket| run_voice_stream(socket, state, socket_request_id));
    with_request_id(response, &request_id)
}

async fn run_voice_stream(mut socket: WebSocket, state: AppState, request_id: String) {
    let Some(Ok(Message::Text(start_message))) = socket.recv().await else {
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
    tracing::info!(
        %request_id,
        locale = %start.locale,
        sample_rate_hz = start.sample_rate_hz,
        limit = start.limit,
        excluded_stations = start.exclude_station_ids.len(),
        "voice websocket session started"
    );

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

    let mut audio_bytes = 0usize;
    let mut last_final_transcript = None;
    while let Some(message) = socket.recv().await {
        match message {
            Ok(Message::Binary(audio)) => {
                if audio.is_empty() || audio.len() > MAX_STREAM_AUDIO_CHUNK_BYTES {
                    let _ = send_stream_error(
                        &mut socket,
                        &request_id,
                        "audio_chunk_invalid",
                        "Audio chunks must contain between 1 and 65536 bytes.",
                        json!({"max_chunk_bytes": MAX_STREAM_AUDIO_CHUNK_BYTES}),
                    )
                    .await;
                    return;
                }
                audio_bytes = audio_bytes.saturating_add(audio.len());
                if audio_bytes > MAX_STREAM_AUDIO_BYTES {
                    let _ = send_stream_error(
                        &mut socket,
                        &request_id,
                        "audio_limit_exceeded",
                        "Streaming session audio limit was exceeded.",
                        json!({"max_audio_bytes": MAX_STREAM_AUDIO_BYTES}),
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
                tracing::info!(%request_id, audio_bytes, "voice audio committed");
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
}

fn newest_final_transcript(updates: &[TranscriptUpdate]) -> Option<String> {
    updates
        .iter()
        .rev()
        .find(|update| update.is_final)
        .map(|update| update.transcript.trim().to_owned())
        .filter(|transcript| !transcript.is_empty())
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

async fn search(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let request_id = request_id(&headers);
    if !state.is_authorized(&headers) {
        return unauthorized_response(&request_id);
    }
    let request = match parse_json_request::<SearchRequestDto>(&headers, body, &request_id).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let validated = match ValidatedSearchRequest::try_from(request) {
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
    tracing::info!(
        %request_id,
        query = %validated.query,
        locale = %validated.locale,
        limit = validated.limit,
        excluded_stations = validated.exclude_station_ids.len(),
        "station search request"
    );

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
                &request_id,
                json!({}),
            );
        }
    };
    tracing::info!(
        %request_id,
        terms = ?outcome.query.terms,
        tags = ?outcome.query.tags,
        language = ?outcome.query.language,
        country_code = ?outcome.query.country_code,
        stations = outcome.stations.len(),
        "station search completed"
    );
    for (rank, station) in outcome.stations.iter().enumerate() {
        tracing::info!(
            %request_id,
            rank,
            station_id = %station.station.id,
            station = %station.station.name,
            score = station.score,
            "station search candidate"
        );
    }

    with_request_id(
        Json(SearchResponseDto {
            request_id: request_id.clone(),
            normalized_query: NormalizedQueryDto::from(outcome.query),
            stations: outcome
                .stations
                .iter()
                .map(StationResultDto::from)
                .collect(),
        })
        .into_response(),
        &request_id,
    )
}

/// Resolves an already-recognized voice transcript through the existing search service.
///
/// The route does not accept audio and does not call an STT provider. This keeps provider
/// credentials and audio-upload policy outside the stable JSON command contract.
async fn voice_command(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let request_id = request_id(&headers);
    if !state.is_authorized(&headers) {
        return unauthorized_response(&request_id);
    }
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
    tracing::info!(
        %request_id,
        transcript = %validated.transcript,
        locale = %validated.locale,
        limit = validated.limit,
        "voice command request"
    );
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
        Ok(Err(error)) => {
            tracing::error!(%error, %request_id, "voice command search failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An unexpected server error occurred.",
                &request_id,
                json!({}),
            );
        }
        Err(_) => {
            tracing::warn!(%request_id, timeout_ms = state.voice_command_timeout.as_millis(), "voice command search timed out");
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
    tracing::info!(
        %request_id,
        transcript = %validated.transcript,
        terms = ?outcome.query.terms,
        tags = ?outcome.query.tags,
        language = ?outcome.query.language,
        country_code = ?outcome.query.country_code,
        stations = stations.len(),
        selected_station = ?selected_station.as_ref().map(|station| station.name.as_str()),
        "voice command completed"
    );
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

async fn parse_json_request<T>(
    headers: &HeaderMap,
    body: Body,
    request_id: &str,
) -> Result<T, Response>
where
    T: DeserializeOwned,
{
    if !is_json_content_type(headers) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "malformed_request",
            "Request body must contain valid JSON.",
            request_id,
            json!({"content_type": "application/json is required"}),
        ));
    }
    let body = to_bytes(body, MAX_JSON_REQUEST_BODY_BYTES)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "Request body exceeds the allowed size.",
                request_id,
                json!({"max_bytes": MAX_JSON_REQUEST_BODY_BYTES}),
            )
        })?;
    let value = serde_json::from_slice::<Value>(&body).map_err(|error| {
        error_response(
            StatusCode::BAD_REQUEST,
            "malformed_request",
            "Request body must contain valid JSON.",
            request_id,
            json!({"body": error.to_string()}),
        )
    })?;
    serde_json::from_value(value).map_err(|error| {
        error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "Request validation failed.",
            request_id,
            json!({"request": error.to_string()}),
        )
    })
}

fn request_id(headers: &HeaderMap) -> String {
    headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| is_valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(next_request_id)
}

fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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
    request_id: &str,
    details: Value,
) -> Response {
    with_request_id(
        (
            status,
            Json(ErrorResponseDto {
                code: code.to_owned(),
                message: message.to_owned(),
                request_id: request_id.to_owned(),
                details,
            }),
        )
            .into_response(),
        request_id,
    )
}

/// Creates the contract-shaped response for a missing or invalid Bearer credential.
fn unauthorized_response(request_id: &str) -> Response {
    let mut response = error_response(
        StatusCode::UNAUTHORIZED,
        "authentication_required",
        "A valid Bearer token is required.",
        request_id,
        json!({}),
    );
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn with_request_id(mut response: Response, request_id: &str) -> Response {
    let header_value = HeaderValue::from_str(request_id)
        .expect("request IDs are generated or constrained to valid header characters");
    response
        .headers_mut()
        .insert(REQUEST_ID_HEADER, header_value);
    response
}

#[derive(Clone)]
struct AppState {
    search_service: SearchService,
    speech_recognizers: SpeechRecognizers,
    voice_command_timeout: Duration,
    api_bearer_token: String,
}

impl AppState {
    /// Verifies the configured opaque application credential without logging the supplied value.
    fn is_authorized(&self, headers: &HeaderMap) -> bool {
        let Some(value) = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };
        let Some(token) = value.strip_prefix("Bearer ") else {
            return false;
        };
        constant_time_eq(token.as_bytes(), self.api_bearer_token.as_bytes())
    }
}

/// Compares opaque credentials without returning early for a matching prefix.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
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
    locale: String,
    sample_rate_hz: u32,
    recognizer_mode: SpeechRecognizerMode,
    limit: usize,
    exclude_station_ids: BTreeSet<String>,
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
        if !matches!(sample_rate_hz, 8_000 | 16_000 | 24_000 | 48_000) {
            details.insert(
                "sample_rate_hz".to_owned(),
                json!("must be one of 8000, 16000, 24000, or 48000"),
            );
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
                limit: validated.limit,
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
    request_id: String,
    transcript: String,
    normalized_query: NormalizedQueryDto,
    selected_station: Option<StationResultDto>,
    stations: Vec<StationResultDto>,
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

/// JSON transport input for one already-recognized voice command.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VoiceCommandRequestDto {
    transcript: String,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    limit: Option<u8>,
    #[serde(default)]
    exclude_station_ids: Vec<String>,
}

struct ValidatedVoiceCommandRequest {
    transcript: String,
    locale: String,
    limit: usize,
    exclude_station_ids: BTreeSet<String>,
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
    action: crate::search::SearchAction,
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
            action: query.action,
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
#[derive(Clone, Serialize)]
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

/// Successful response for the stable voice-command JSON boundary.
#[derive(Serialize)]
struct VoiceCommandResponseDto {
    request_id: String,
    transcript: String,
    normalized_query: NormalizedQueryDto,
    selected_station: Option<StationResultDto>,
    stations: Vec<StationResultDto>,
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

    use super::{HealthResponse, HealthStatus, TEST_API_BEARER_TOKEN, router};

    #[tokio::test]
    async fn liveness_returns_stable_json_response() {
        assert_health_endpoint("/health/live").await;
    }

    #[tokio::test]
    async fn admin_console_is_available_without_exposing_protected_data() {
        let response = router()
            .oneshot(Request::get("/admin").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let page = std::str::from_utf8(&body).unwrap();
        assert!(page.contains("Панель администратора"));
        assert!(page.contains("ROCKSERVER_API_BEARER_TOKEN"));
        assert!(!page.contains(TEST_API_BEARER_TOKEN));
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

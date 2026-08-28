//! Axum routes and transport DTOs for the RockServer HTTP API.

use std::{
    collections::BTreeSet,
    env,
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use passkey_auth::{AuthenticationResponse, CredentialId, RegistrationResponse};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;
use uuid::Uuid;

use crate::{
    auth::{
        NewBrowserSession, NewPairingRequest, NewPairingSession, NewWebAuthnChallenge, SecretHash,
        WebAuthnCeremony, webauthn,
    },
    persistence::PostgresAccountStore,
    search::{
        InMemoryStationRepository, QueryParserInput, RankedStation, SearchConstraints,
        SearchService, StationHealth, StationRepository, UnavailableStationRepository,
    },
    voice::{
        SpeechProviderError, SpeechRecognizerMode, SpeechRecognizers, SpeechStreamConfig,
        StreamingSpeechRecognizer, TranscriptUpdate, UnavailableSpeechRecognizer,
    },
};

#[path = "state.rs"]
mod state;
use state::{AppState, PublicLimitState};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Deterministic credential used only by convenience routers in offline tests and examples.
///
/// Production startup must supply a unique secret through [`router_with_services_and_bearer_token`].
pub const TEST_API_BEARER_TOKEN: &str = "rockserver-offline-test-token";

const MAX_PUBLIC_JSON_REQUEST_BODY_BYTES: usize = 16 * 1024;
const REQUEST_ID_HEADER: &str = "x-request-id";
const MAX_STREAM_AUDIO_CHUNK_BYTES: usize = 32 * 1024;
const MAX_STREAM_AUDIO_BYTES: usize = 2 * 1024 * 1024;
const MAX_STREAM_AUDIO_SECONDS: usize = 60;
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_WALL_TIMEOUT: Duration = Duration::from_secs(75);
const DEFAULT_STREAM_OPERATION_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum duration the voice-command transport waits for query interpretation and search.
pub const DEFAULT_VOICE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// Environment variable containing the secret Caddy injects into trusted browser requests.
pub const TRUSTED_PROXY_TOKEN_ENV: &str = "ROCKSERVER_TRUSTED_PROXY_TOKEN";
const TRUSTED_PROXY_HEADER: &str = "x-rockserver-proxy";

const RATE_WINDOW: Duration = Duration::from_secs(60);
const RETRY_AFTER_SECONDS: u64 = 60;
const VOICE_CAPACITY_RETRY_AFTER_SECONDS: u64 = 30;
const GLOBAL_VOICE_SESSIONS: usize = 100;

#[derive(Clone, Copy)]
struct PublicLimit {
    requests: usize,
    burst: usize,
}

const CATALOG_LIST_LIMIT: PublicLimit = PublicLimit {
    requests: 60,
    burst: 20,
};
const CATALOG_GET_LIMIT: PublicLimit = PublicLimit {
    requests: 120,
    burst: 40,
};
const SEARCH_LIMIT: PublicLimit = PublicLimit {
    requests: 30,
    burst: 10,
};
const VOICE_COMMAND_LIMIT: PublicLimit = PublicLimit {
    requests: 12,
    burst: 4,
};
const VOICE_UPGRADE_LIMIT: PublicLimit = PublicLimit {
    requests: 6,
    burst: 2,
};

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
        account_store: None,
        trusted_proxy_token: None,
        public_limits: Arc::new(Mutex::new(PublicLimitState::default())),
    };

    Router::new()
        .route("/admin", get(admin_console))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/v1/catalog/stations", get(public_catalog_list))
        .route("/v1/catalog/stations/{station_id}", get(public_catalog_get))
        .route("/api/v1/search", post(search))
        .route("/v1/search", post(public_search))
        .route("/api/v1/voice/command", post(voice_command))
        .route("/v1/voice/command", post(public_voice_command))
        .route("/api/v1/voice/stream", get(voice_stream))
        .route("/v1/voice/stream", get(public_voice_stream))
        .route("/v1/pairing-requests", post(create_pairing_request))
        .route("/v1/pairing-requests/lookup", get(lookup_pairing_request))
        .route("/v1/auth/browser-session", post(browser_session))
        .route("/v1/browser/account", get(browser_account))
        .route("/v1/auth/browser-logout", post(logout_browser_session))
        .route(
            "/v1/pairing-requests/{request_id}/approve",
            post(approve_pairing_request),
        )
        .route(
            "/v1/auth/passkeys/registration/options",
            post(registration_options),
        )
        .route(
            "/v1/auth/passkeys/registration/verify",
            post(registration_verify),
        )
        .route(
            "/v1/auth/passkeys/authentication/options",
            post(authentication_options),
        )
        .route(
            "/v1/auth/passkeys/authentication/verify",
            post(authentication_verify),
        )
        .route(
            "/v1/pairing-requests/{request_id}/complete",
            post(complete_pairing_request),
        )
        .route("/v1/auth/refresh", post(refresh_native_session))
        .route("/v1/auth/logout", post(logout_native_session))
        .route("/v1/account/profile", get(account_profile))
        .route("/v1/account", delete(delete_account))
        .route("/v1/devices", get(list_devices))
        .route("/v1/devices/{device_id}", delete(revoke_device))
        .route(
            "/v1/browser/devices/{device_id}",
            patch(rename_browser_device).delete(revoke_browser_device),
        )
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
}

/// Creates a router with the PostgreSQL account store required by passkey and pairing endpoints.
pub fn router_with_speech_recognizers_bearer_and_account_store(
    search_service: SearchService,
    speech_recognizers: SpeechRecognizers,
    voice_command_timeout: Duration,
    api_bearer_token: impl Into<String>,
    account_store: PostgresAccountStore,
) -> Router {
    router_with_speech_recognizers_bearer_account_store_and_proxy(
        search_service,
        speech_recognizers,
        voice_command_timeout,
        api_bearer_token,
        account_store,
        env::var(TRUSTED_PROXY_TOKEN_ENV).unwrap_or_default(),
    )
}

/// Creates the production router with account state and an authenticated Caddy proxy token.
pub fn router_with_speech_recognizers_bearer_account_store_and_proxy(
    search_service: SearchService,
    speech_recognizers: SpeechRecognizers,
    voice_command_timeout: Duration,
    api_bearer_token: impl Into<String>,
    account_store: PostgresAccountStore,
    trusted_proxy_token: impl Into<String>,
) -> Router {
    let state = AppState {
        search_service,
        speech_recognizers,
        voice_command_timeout,
        api_bearer_token: api_bearer_token.into(),
        account_store: Some(account_store),
        trusted_proxy_token: Some(trusted_proxy_token.into()),
        public_limits: Arc::new(Mutex::new(PublicLimitState::default())),
    };
    Router::new()
        .route("/admin", get(admin_console))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/v1/catalog/stations", get(public_catalog_list))
        .route("/v1/catalog/stations/{station_id}", get(public_catalog_get))
        .route("/api/v1/search", post(search))
        .route("/v1/search", post(public_search))
        .route("/api/v1/voice/command", post(voice_command))
        .route("/v1/voice/command", post(public_voice_command))
        .route("/api/v1/voice/stream", get(voice_stream))
        .route("/v1/voice/stream", get(public_voice_stream))
        .route("/v1/pairing-requests", post(create_pairing_request))
        .route("/v1/pairing-requests/lookup", get(lookup_pairing_request))
        .route("/v1/auth/browser-session", post(browser_session))
        .route("/v1/browser/account", get(browser_account))
        .route("/v1/auth/browser-logout", post(logout_browser_session))
        .route(
            "/v1/pairing-requests/{request_id}/approve",
            post(approve_pairing_request),
        )
        .route(
            "/v1/auth/passkeys/registration/options",
            post(registration_options),
        )
        .route(
            "/v1/auth/passkeys/registration/verify",
            post(registration_verify),
        )
        .route(
            "/v1/auth/passkeys/authentication/options",
            post(authentication_options),
        )
        .route(
            "/v1/auth/passkeys/authentication/verify",
            post(authentication_verify),
        )
        .route(
            "/v1/pairing-requests/{request_id}/complete",
            post(complete_pairing_request),
        )
        .route("/v1/auth/refresh", post(refresh_native_session))
        .route("/v1/auth/logout", post(logout_native_session))
        .route("/v1/account/profile", get(account_profile))
        .route("/v1/account", delete(delete_account))
        .route("/v1/devices", get(list_devices))
        .route("/v1/devices/{device_id}", delete(revoke_device))
        .route(
            "/v1/browser/devices/{device_id}",
            patch(rename_browser_device).delete(revoke_browser_device),
        )
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

const PAIRING_LIFETIME_MINUTES: i32 = 10;
const PAIRING_CREATE_LIMIT: i64 = 10;
const PAIRING_RATE_LIMIT_MINUTES: i32 = 15;
const FIRST_PARTY_ORIGIN: &str = "https://alex.vault57.ru";

/// Supplies the migration-safe label for account records created before naming is configurable.
fn default_account_display_name() -> String {
    "Rock account".to_owned()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePairingRequestDto {
    device_display_name: String,
    device_type: String,
    #[serde(default)]
    app_version: Option<String>,
}

#[derive(Serialize)]
struct CreatedPairingRequestDto {
    pairing_request_id: String,
    desktop_token: String,
    approval_secret: String,
    short_code: String,
    verification_phrase: String,
    device_display_name: String,
    device_type: String,
    expires_at: String,
    status: &'static str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingLookupDto {
    code: String,
}

#[derive(Serialize)]
struct PairingPreviewDto {
    request_id: String,
    device_display_name: String,
    device_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_version: Option<String>,
    verification_phrase: String,
    short_code: String,
    expires_at: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingApprovalDto {
    approval_secret: String,
    verification_phrase: String,
}

#[derive(Serialize)]
struct BrowserSessionDto {
    account_display_name: String,
    csrf_token: String,
}

#[derive(Serialize)]
struct BrowserAccountDto {
    account_display_name: String,
    device_limit: u8,
    devices: Vec<BrowserDeviceDto>,
}

#[derive(Serialize)]
struct BrowserDeviceDto {
    device_id: String,
    device_display_name: String,
    device_type: String,
    connected_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<String>,
    session_status: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBrowserDeviceDto {
    device_display_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingCompletionDto {
    desktop_token: String,
}

#[derive(Serialize)]
struct PairingCompletionResponseDto {
    user_id: String,
    device_id: String,
    session_id: String,
    access_token: String,
    refresh_token: String,
    account_display_name: String,
    device_display_name: String,
    device_type: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefreshRequestDto {
    refresh_token: String,
}

#[derive(Serialize)]
struct NativeTokenPairDto {
    access_token: String,
    refresh_token: String,
}

#[derive(Serialize)]
struct AccountProfileDto {
    user_id: String,
    session_id: String,
    device_id: String,
    account_display_name: String,
    created_at: String,
    device_display_name: String,
    device_type: String,
    device_created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<String>,
}

#[derive(Serialize)]
struct DeviceDto {
    device_id: String,
    user_id: String,
    device_display_name: String,
    device_type: String,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<String>,
}

#[derive(Serialize)]
struct DeviceListDto {
    devices: Vec<DeviceDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationVerifyDto {
    challenge_id: Uuid,
    #[serde(flatten)]
    response: RegistrationResponse,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationOptionsRequestDto {
    #[serde(default = "default_account_display_name")]
    account_display_name: String,
}

#[derive(Serialize)]
struct RegistrationOptionsDto {
    challenge_id: Uuid,
    options: passkey_auth::RegistrationChallenge,
}

#[derive(Serialize)]
struct AuthenticationOptionsResponseDto {
    challenge_id: Uuid,
    options: passkey_auth::AuthenticationChallenge,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticationVerifyDto {
    challenge_id: Uuid,
    #[serde(flatten)]
    response: AuthenticationResponse,
}

/// Starts a first-party passkey registration and persists its opaque verifier state.
async fn registration_options(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let request_id = request_id(&headers);
    let payload: RegistrationOptionsRequestDto =
        match parse_json_request(&headers, body, &request_id).await {
            Ok(payload) => payload,
            Err(response) => return response,
        };
    let account_display_name = payload.account_display_name.trim();
    if account_display_name.is_empty() || account_display_name.len() > 128 {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "Request validation failed.",
            &request_id,
            json!({"field":"account_display_name"}),
        );
    }
    if !is_trusted_browser_request(&headers)
        || !trusted_proxy_header_matches(&headers, state.trusted_proxy_token.as_deref())
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            &request_id,
            json!({}),
        );
    };
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let user_id = Uuid::new_v4();
    if store
        .create_user_with_display_name(user_id, account_display_name)
        .await
        .is_err()
    {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is temporarily unavailable.",
            &request_id,
            json!({}),
        );
    }
    let (options, state_blob) = webauthn::start_registration(user_id, account_display_name);
    let encoded = match webauthn::encode_state(&state_blob) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "auth_unavailable",
                "Authentication service is temporarily unavailable.",
                &request_id,
                json!({}),
            );
        }
    };
    let challenge_id = Uuid::new_v4();
    let challenge_hash = token_hash(&state_blob.challenge.to_b64url());
    let challenge = NewWebAuthnChallenge {
        challenge_id,
        challenge_hash: &challenge_hash,
        state_blob: &encoded,
        ceremony: WebAuthnCeremony::Registration,
        rp_id: webauthn::RP_ID,
        origin: webauthn::ORIGIN,
        expires_at_rfc3339: "unused: database clock",
        user_id: Some(user_id),
        browser_session_id: None,
        pairing_request_id: None,
    };
    match store
        .create_webauthn_challenge_for_minutes(challenge, 5)
        .await
    {
        Ok(true) => with_request_id(
            Json(RegistrationOptionsDto {
                challenge_id,
                options,
            })
            .into_response(),
            &request_id,
        ),
        Ok(false) | Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Authentication service is temporarily unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Verifies a passkey registration and establishes a Secure, HttpOnly browser session.
async fn registration_verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<RegistrationVerifyDto>,
) -> Response {
    let request_id = request_id(&headers);
    if !is_trusted_browser_request(&headers)
        || !trusted_proxy_header_matches(&headers, state.trusted_proxy_token.as_deref())
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            &request_id,
            json!({}),
        );
    }
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let Some(encoded) = store
        .load_webauthn_challenge_state(payload.challenge_id)
        .await
        .ok()
        .flatten()
    else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "challenge_rejected",
            "The passkey challenge is expired or already used.",
            &request_id,
            json!({}),
        );
    };
    let state_blob: passkey_auth::RegistrationState = match webauthn::decode_state(&encoded) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "challenge_rejected",
                "The passkey challenge is invalid.",
                &request_id,
                json!({}),
            );
        }
    };
    let credential = match webauthn::finish_registration(&state_blob, &payload.response) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "webauthn_rejected",
                "The passkey response was rejected.",
                &request_id,
                json!({}),
            );
        }
    };
    let challenge_hash = token_hash(&state_blob.challenge.to_b64url());
    if !store
        .consume_webauthn_challenge(
            payload.challenge_id,
            &challenge_hash,
            WebAuthnCeremony::Registration,
            webauthn::ORIGIN,
            webauthn::RP_ID,
        )
        .await
        .unwrap_or(false)
    {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "challenge_rejected",
            "The passkey challenge is expired or already used.",
            &request_id,
            json!({}),
        );
    }
    if !store
        .create_passkey_credential(
            Uuid::new_v4(),
            Uuid::from_slice(&state_blob.user_id).unwrap_or_default(),
            credential.id.as_bytes(),
            credential.public_key_cose.as_bytes(),
            i64::from(credential.counter),
            &credential.transports,
        )
        .await
        .unwrap_or(false)
    {
        return error_response(
            StatusCode::CONFLICT,
            "credential_rejected",
            "The passkey credential is already registered.",
            &request_id,
            json!({}),
        );
    }
    let registered_user_id = Uuid::from_slice(&state_blob.user_id).unwrap_or_default();
    let session_token = Uuid::new_v4().simple().to_string();
    let csrf_token = Uuid::new_v4().simple().to_string();
    let browser = NewBrowserSession {
        session_id: Uuid::new_v4(),
        user_id: registered_user_id,
        session_token_hash: &token_hash(&session_token),
        csrf_hash: &token_hash(&csrf_token),
        passkey_reauthenticated_at_rfc3339: "unused: database clock",
        expires_at_rfc3339: "unused: database clock",
    };
    if !store
        .create_browser_session_for_minutes(browser, 30)
        .await
        .unwrap_or(false)
    {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Authentication service is temporarily unavailable.",
            &request_id,
            json!({}),
        );
    }
    let account_display_name = match store.account_display_name(registered_user_id).await {
        Ok(Some(name)) => name,
        Ok(None) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "credential_rejected",
                "The passkey response was rejected.",
                &request_id,
                json!({}),
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Authentication service is temporarily unavailable.",
                &request_id,
                json!({}),
            );
        }
    };
    let mut response = with_request_id(
        Json(json!({"account_display_name": account_display_name, "csrf_token": csrf_token}))
            .into_response(),
        &request_id,
    );
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&format!("rockserver_browser={session_token}; Path=/; Max-Age=1800; HttpOnly; Secure; SameSite=Strict")).expect("generated cookie is valid"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Starts a discoverable authentication ceremony without requiring an account identifier.
async fn authentication_options(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    if !is_trusted_browser_request(&headers)
        || !trusted_proxy_header_matches(&headers, state.trusted_proxy_token.as_deref())
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            &request_id,
            json!({}),
        );
    }
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let (options, state_blob) = webauthn::start_authentication();
    let encoded = match webauthn::encode_state(&state_blob) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "auth_unavailable",
                "Authentication service is temporarily unavailable.",
                &request_id,
                json!({}),
            );
        }
    };
    let challenge_id = Uuid::new_v4();
    let challenge_hash = token_hash(&state_blob.state.challenge.to_b64url());
    let challenge = NewWebAuthnChallenge {
        challenge_id,
        challenge_hash: &challenge_hash,
        state_blob: &encoded,
        ceremony: WebAuthnCeremony::Authentication,
        rp_id: webauthn::RP_ID,
        origin: webauthn::ORIGIN,
        expires_at_rfc3339: "unused: database clock",
        user_id: None,
        browser_session_id: None,
        pairing_request_id: None,
    };
    match store
        .create_webauthn_challenge_for_minutes(challenge, 5)
        .await
    {
        Ok(true) => with_request_id(
            Json(AuthenticationOptionsResponseDto {
                challenge_id,
                options,
            })
            .into_response(),
            &request_id,
        ),
        Ok(false) | Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Authentication service is temporarily unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Verifies a passkey assertion, applies the rollback guard and issues a browser session cookie.
async fn authentication_verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AuthenticationVerifyDto>,
) -> Response {
    let request_id = request_id(&headers);
    if !is_trusted_browser_request(&headers)
        || !trusted_proxy_header_matches(&headers, state.trusted_proxy_token.as_deref())
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            &request_id,
            json!({}),
        );
    }
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let Some(encoded) = store
        .load_webauthn_challenge_state(payload.challenge_id)
        .await
        .ok()
        .flatten()
    else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "challenge_rejected",
            "The passkey challenge is expired or already used.",
            &request_id,
            json!({}),
        );
    };
    let state_blob: webauthn::AuthenticationStateContext = match webauthn::decode_state(&encoded) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "challenge_rejected",
                "The passkey challenge is invalid.",
                &request_id,
                json!({}),
            );
        }
    };
    let user_id = match webauthn::user_id_from_handle(payload.response.user_handle.as_deref()) {
        Ok(user_id) => user_id,
        Err(_) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "webauthn_rejected",
                "The passkey response was rejected.",
                &request_id,
                json!({}),
            );
        }
    };
    if state_blob
        .user_id
        .is_some_and(|expected| expected != user_id)
    {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "webauthn_rejected",
            "The passkey response was rejected.",
            &request_id,
            json!({}),
        );
    }
    let credential_id = match CredentialId::from_b64url(&payload.response.id) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "webauthn_rejected",
                "The passkey response was rejected.",
                &request_id,
                json!({}),
            );
        }
    };
    let Some(credential) = store
        .find_passkey_credential_for_user(user_id, credential_id.as_bytes())
        .await
        .ok()
        .flatten()
    else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "webauthn_rejected",
            "The passkey response was rejected.",
            &request_id,
            json!({}),
        );
    };
    let outcome = match webauthn::finish_authentication(&state_blob, &payload.response, &credential)
    {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "webauthn_rejected",
                "The passkey response was rejected.",
                &request_id,
                json!({}),
            );
        }
    };
    if !store
        .consume_webauthn_challenge(
            payload.challenge_id,
            &token_hash(&state_blob.state.challenge.to_b64url()),
            WebAuthnCeremony::Authentication,
            webauthn::ORIGIN,
            webauthn::RP_ID,
        )
        .await
        .unwrap_or(false)
        || !store
            .advance_passkey_sign_count(credential_id.as_bytes(), i64::from(outcome.new_counter))
            .await
            .unwrap_or(false)
    {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "challenge_rejected",
            "The passkey challenge is expired, replayed or rolled back.",
            &request_id,
            json!({}),
        );
    }
    let session_token = Uuid::new_v4().simple().to_string();
    let csrf_token = Uuid::new_v4().simple().to_string();
    let session = NewBrowserSession {
        session_id: Uuid::new_v4(),
        user_id,
        session_token_hash: &token_hash(&session_token),
        csrf_hash: &token_hash(&csrf_token),
        passkey_reauthenticated_at_rfc3339: "unused: database clock",
        expires_at_rfc3339: "unused: database clock",
    };
    if !store
        .create_browser_session_for_minutes(session, 30)
        .await
        .unwrap_or(false)
    {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Authentication service is temporarily unavailable.",
            &request_id,
            json!({}),
        );
    }
    let mut response = with_request_id(
        Json(json!({"csrf_token": csrf_token})).into_response(),
        &request_id,
    );
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&format!("rockserver_browser={session_token}; Path=/; Max-Age=1800; HttpOnly; Secure; SameSite=Strict")).expect("generated cookie is valid"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Starts a short-lived desktop pairing request without disclosing its hashed server-side proofs.
async fn create_pairing_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let request_id = request_id(&headers);
    let payload: CreatePairingRequestDto =
        match parse_json_request(&headers, body, &request_id).await {
            Ok(payload) => payload,
            Err(response) => return response,
        };
    if payload.device_display_name.trim().is_empty()
        || payload.device_display_name.len() > 128
        || payload.device_type.trim().is_empty()
        || payload.device_type.len() > 64
        || payload
            .app_version
            .as_ref()
            .is_some_and(|value| value.len() > 64)
    {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "Request validation failed.",
            &request_id,
            json!({"field":"device_display_name or device_type"}),
        );
    }
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let rate_scope = request_rate_limit_scope(&headers, state.trusted_proxy_token.as_deref());
    let rate_key = token_hash(&format!("pairing-create:{rate_scope}"));
    match store
        .consume_rate_limit_for_minutes(&rate_key, PAIRING_RATE_LIMIT_MINUTES, PAIRING_CREATE_LIMIT)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return retry_after(
                error_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    "Request rate limit exceeded.",
                    &request_id,
                    json!({"limit_scope":"pairing_create"}),
                ),
                (PAIRING_RATE_LIMIT_MINUTES as u64) * 60,
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "pairing_unavailable",
                "Pairing service is temporarily unavailable.",
                &request_id,
                json!({}),
            );
        }
    }
    let pairing_request_id = Uuid::new_v4();
    let desktop_token = Uuid::new_v4().simple().to_string();
    let approval_secret = Uuid::new_v4().simple().to_string();
    let short_code = Uuid::new_v4().simple().to_string()[..8].to_ascii_uppercase();
    let verification_phrase = pairing_phrase(&pairing_request_id);
    let desktop_token_hash = token_hash(&desktop_token);
    let approval_secret_hash = token_hash(&approval_secret);
    let short_code_hash = token_hash(&short_code);
    let request = NewPairingRequest {
        request_id: pairing_request_id,
        desktop_token_hash: &desktop_token_hash,
        approval_secret_hash: &approval_secret_hash,
        short_code_hash: &short_code_hash,
        verification_phrase: &verification_phrase,
        device_display_name: payload.device_display_name.trim(),
        device_type: payload.device_type.trim(),
        app_version: payload.app_version.as_deref(),
        expires_at_rfc3339: "unused: database clock",
    };
    match store
        .create_pairing_request_for_minutes(request, PAIRING_LIFETIME_MINUTES)
        .await
    {
        Ok(Some(expires_at)) => {
            let mut response = with_request_id(
                (
                    StatusCode::CREATED,
                    Json(CreatedPairingRequestDto {
                        pairing_request_id: pairing_request_id.to_string(),
                        desktop_token,
                        approval_secret,
                        short_code,
                        verification_phrase,
                        device_display_name: payload.device_display_name.trim().to_owned(),
                        device_type: payload.device_type.trim().to_owned(),
                        expires_at,
                        status: "pending",
                    }),
                )
                    .into_response(),
                &request_id,
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Ok(None) | Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "pairing_unavailable",
            "Pairing service is temporarily unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Resolves a short code to non-secret device information for a browser pairing screen.
async fn lookup_pairing_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PairingLookupDto>,
) -> Response {
    let request_id = request_id(&headers);
    if query.code.len() != 8 || !query.code.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return error_response(
            StatusCode::NOT_FOUND,
            "pairing_not_found",
            "Pairing request is unavailable.",
            &request_id,
            json!({}),
        );
    }
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let account_display_name = match cookie_value(&headers, "rockserver_browser") {
        Some(cookie) => store
            .browser_account_display_name(&token_hash(cookie))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    match store
        .lookup_pairing_request(&token_hash(&query.code.to_ascii_uppercase()))
        .await
    {
        Ok(Some(preview)) => with_request_id(
            Json(PairingPreviewDto {
                request_id: preview.request_id.to_string(),
                device_display_name: preview.device_display_name,
                device_type: preview.device_type,
                app_version: preview.app_version,
                verification_phrase: preview.verification_phrase,
                short_code: query.code.to_ascii_uppercase(),
                expires_at: preview.expires_at,
                status: "pending",
                account_display_name,
            })
            .into_response(),
            &request_id,
        ),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "pairing_not_found",
            "Pairing request is unavailable.",
            &request_id,
            json!({}),
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "pairing_unavailable",
            "Pairing service is temporarily unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Refreshes the tab-local CSRF proof for an active browser account session.
async fn browser_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    if !is_trusted_browser_request(&headers)
        || !trusted_proxy_header_matches(&headers, state.trusted_proxy_token.as_deref())
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            &request_id,
            json!({}),
        );
    }
    let Some(cookie) = cookie_value(&headers, "rockserver_browser") else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A browser session is required.",
            &request_id,
            json!({}),
        );
    };
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let csrf_token = Uuid::new_v4().simple().to_string();
    match store
        .rotate_browser_csrf(&token_hash(cookie), &token_hash(&csrf_token))
        .await
    {
        Ok(Some(account_display_name)) => {
            let mut response = with_request_id(
                Json(BrowserSessionDto {
                    account_display_name,
                    csrf_token,
                })
                .into_response(),
                &request_id,
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Ok(None) => error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A browser session is required.",
            &request_id,
            json!({}),
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Returns the authenticated browser account's safe device-management projection.
async fn browser_account(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    if !trusted_proxy_header_matches(&headers, state.trusted_proxy_token.as_deref()) {
        return error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            &request_id,
            json!({}),
        );
    }
    let Some(cookie) = cookie_value(&headers, "rockserver_browser") else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A browser session is required.",
            &request_id,
            json!({}),
        );
    };
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let account_display_name = match store
        .browser_account_display_name(&token_hash(cookie))
        .await
    {
        Ok(Some(name)) => name,
        Ok(None) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "authentication_required",
                "A browser session is required.",
                &request_id,
                json!({}),
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Account service is unavailable.",
                &request_id,
                json!({}),
            );
        }
    };
    let user_id = match store.browser_session_user(&token_hash(cookie)).await {
        Ok(Some(user_id)) => user_id,
        Ok(None) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "authentication_required",
                "A browser session is required.",
                &request_id,
                json!({}),
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Account service is unavailable.",
                &request_id,
                json!({}),
            );
        }
    };
    match store.list_browser_devices(user_id).await {
        Ok(devices) => {
            let mut response = with_request_id(
                Json(BrowserAccountDto {
                    account_display_name,
                    device_limit: 10,
                    devices: devices
                        .into_iter()
                        .map(|device| BrowserDeviceDto {
                            device_id: device.id.to_string(),
                            device_display_name: device.device_display_name,
                            device_type: device.device_type,
                            connected_at: device.created_at,
                            last_seen_at: device.last_seen_at,
                            session_status: device.session_status,
                        })
                        .collect(),
                })
                .into_response(),
                &request_id,
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Validates a browser state-changing request and derives its account owner from cookie and CSRF proofs.
async fn browser_mutation_owner(
    state: &AppState,
    headers: &HeaderMap,
    request_id: &str,
) -> Result<Uuid, Response> {
    if !is_trusted_browser_request(headers)
        || !trusted_proxy_header_matches(headers, state.trusted_proxy_token.as_deref())
    {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            request_id,
            json!({}),
        ));
    }
    let Some(cookie) = cookie_value(headers, "rockserver_browser") else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A browser session is required.",
            request_id,
            json!({}),
        ));
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 256)
    else {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "A valid CSRF token is required.",
            request_id,
            json!({}),
        ));
    };
    let Some(store) = state.account_store.as_ref() else {
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            request_id,
            json!({}),
        ));
    };
    match store
        .browser_session_user_with_csrf(&token_hash(cookie), &token_hash(csrf))
        .await
    {
        Ok(Some(user_id)) => Ok(user_id),
        Ok(None) => Err(error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A browser session is required.",
            request_id,
            json!({}),
        )),
        Err(_) => Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            request_id,
            json!({}),
        )),
    }
}

/// Renames a caller-owned device through the first-party browser management boundary.
async fn rename_browser_device(
    State(state): State<AppState>,
    Path(device_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<RenameBrowserDeviceDto>,
) -> Response {
    let request_id = request_id(&headers);
    let user_id = match browser_mutation_owner(&state, &headers, &request_id).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let Some(name) = validated_device_display_name(&payload.device_display_name) else {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_device_name",
            "Device name must be 1 to 128 printable characters.",
            &request_id,
            json!({}),
        );
    };
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    match store.rename_owned_device(user_id, device_id, name).await {
        Ok(true) => with_request_id(StatusCode::NO_CONTENT.into_response(), &request_id),
        Ok(false) => error_response(
            StatusCode::NOT_FOUND,
            "device_not_found",
            "Device is unavailable.",
            &request_id,
            json!({}),
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Normalizes a user-visible device name while rejecting control characters that could affect logs or UI text.
fn validated_device_display_name(value: &str) -> Option<&str> {
    let name = value.trim();
    (!name.is_empty() && name.chars().count() <= 128 && !name.chars().any(char::is_control))
        .then_some(name)
}

/// Revokes another caller-owned native device from the browser account centre.
async fn revoke_browser_device(
    State(state): State<AppState>,
    Path(device_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let request_id = request_id(&headers);
    let user_id = match browser_mutation_owner(&state, &headers, &request_id).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    match store.revoke_owned_device(user_id, device_id).await {
        Ok(true) => with_request_id(StatusCode::NO_CONTENT.into_response(), &request_id),
        Ok(false) => error_response(
            StatusCode::NOT_FOUND,
            "device_not_found",
            "Device is unavailable.",
            &request_id,
            json!({}),
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Logs out only the current browser cookie session; native device sessions remain unchanged.
async fn logout_browser_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    let user_id = match browser_mutation_owner(&state, &headers, &request_id).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let Some(cookie) = cookie_value(&headers, "rockserver_browser") else {
        return unauthorized_response(&request_id);
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
    else {
        return unauthorized_response(&request_id);
    };
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    match store
        .logout_browser_session(user_id, &token_hash(cookie), &token_hash(csrf))
        .await
    {
        Ok(true) => with_request_id(StatusCode::NO_CONTENT.into_response(), &request_id),
        Ok(false) => unauthorized_response(&request_id),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Approves a pairing request with a fresh browser session and double-submit CSRF proof.
async fn approve_pairing_request(
    State(state): State<AppState>,
    Path(pairing_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<PairingApprovalDto>,
) -> Response {
    let request_id_header = request_id(&headers);
    if !is_trusted_browser_request(&headers)
        || !trusted_proxy_header_matches(&headers, state.trusted_proxy_token.as_deref())
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            &request_id_header,
            json!({}),
        );
    }
    let Some(cookie) = cookie_value(&headers, "rockserver_browser") else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A browser session is required.",
            &request_id_header,
            json!({}),
        );
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 256)
    else {
        return error_response(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "A valid CSRF token is required.",
            &request_id_header,
            json!({}),
        );
    };
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id_header,
            json!({}),
        );
    };
    let approval_hash = token_hash(&payload.approval_secret);
    let session_hash = token_hash(cookie);
    let csrf_hash = token_hash(csrf);
    let approved = store
        .approve_pairing_request_with_browser_proof(
            pairing_id,
            &approval_hash,
            &payload.verification_phrase,
            &session_hash,
            &csrf_hash,
        )
        .await;
    match approved {
        Ok(true) => with_request_id(StatusCode::NO_CONTENT.into_response(), &request_id_header),
        Ok(false) => error_response(
            StatusCode::CONFLICT,
            "pairing_not_approvable",
            "Pairing request cannot be approved.",
            &request_id_header,
            json!({}),
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "pairing_unavailable",
            "Pairing service is temporarily unavailable.",
            &request_id_header,
            json!({}),
        ),
    }
}

/// Atomically consumes an approved desktop request and returns native bearer credentials.
async fn complete_pairing_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pairing_id): Path<Uuid>,
    Json(payload): Json<PairingCompletionDto>,
) -> Response {
    let request_id = request_id(&headers);
    if payload.desktop_token.len() < 16 || payload.desktop_token.len() > 128 {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "pairing_rejected",
            "Pairing proof is invalid or expired.",
            &request_id,
            json!({}),
        );
    }
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let access_token = Uuid::new_v4().simple().to_string();
    let refresh_token = Uuid::new_v4().simple().to_string();
    let device_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let access_hash = token_hash(&access_token);
    let refresh_hash = token_hash(&refresh_token);
    let session = NewPairingSession {
        session_id,
        device_id,
        access_hash: &access_hash,
        access_expires_at_rfc3339: "db:15m",
        refresh_id: Uuid::new_v4(),
        refresh_hash: &refresh_hash,
        refresh_expires_at_rfc3339: "db:30d",
    };
    match store
        .complete_pairing(pairing_id, &token_hash(&payload.desktop_token), session)
        .await
    {
        Ok(Some(result)) => {
            let mut response = with_request_id(
                Json(PairingCompletionResponseDto {
                    user_id: result.user_id.to_string(),
                    device_id: result.device_id.to_string(),
                    session_id: result.session_id.to_string(),
                    access_token,
                    refresh_token,
                    account_display_name: result.account_display_name,
                    device_display_name: result.device_display_name,
                    device_type: result.device_type,
                })
                .into_response(),
                &request_id,
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Ok(None) => error_response(
            StatusCode::UNAUTHORIZED,
            "pairing_rejected",
            "Pairing proof is invalid, expired or already used.",
            &request_id,
            json!({}),
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "pairing_unavailable",
            "Pairing service is temporarily unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Rotates a native refresh token and returns a fresh opaque access/refresh pair.
async fn refresh_native_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let request_id = request_id(&headers);
    let payload: RefreshRequestDto = match parse_json_request(&headers, body, &request_id).await {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    if payload.refresh_token.len() < 16 || payload.refresh_token.len() > 512 {
        return unauthorized_response(&request_id);
    }
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let access_token = Uuid::new_v4().simple().to_string();
    let refresh_token = Uuid::new_v4().simple().to_string();
    let result = store
        .rotate_refresh_with_access(
            &token_hash(&payload.refresh_token),
            Uuid::new_v4(),
            &token_hash(&refresh_token),
            "db:30d",
            &token_hash(&access_token),
            "db:15m",
        )
        .await;
    match result {
        Ok(_) => {
            let mut response = with_request_id(
                Json(NativeTokenPairDto {
                    access_token,
                    refresh_token,
                })
                .into_response(),
                &request_id,
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => unauthorized_response(&request_id),
    }
}

/// Revokes the current native session and its refresh-token family.
async fn logout_native_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let Some(token) = bearer_token(&headers) else {
        return unauthorized_response(&request_id);
    };
    let session = match store
        .find_active_session_by_access_hash(&token_hash(token))
        .await
    {
        Ok(Some(session)) => session,
        Ok(None) => return unauthorized_response(&request_id),
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Account service is unavailable.",
                &request_id,
                json!({}),
            );
        }
    };
    match store
        .revoke_session(session.session_id, session.user_id)
        .await
    {
        Ok(true) => with_request_id(StatusCode::NO_CONTENT.into_response(), &request_id),
        Ok(false) => unauthorized_response(&request_id),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Returns the caller's account and current device projection.
async fn account_profile(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    let Some(session) = match_native_session(&state, &headers, &request_id).await else {
        return unauthorized_response(&request_id);
    };
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let projection = match store
        .account_projection(session.user_id, session.device_id)
        .await
    {
        Ok(Some(projection)) => projection,
        Ok(None) => return unauthorized_response(&request_id),
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Account service is unavailable.",
                &request_id,
                json!({}),
            );
        }
    };
    with_request_id(
        Json(AccountProfileDto {
            user_id: session.user_id.to_string(),
            session_id: session.session_id.to_string(),
            device_id: session.device_id.to_string(),
            account_display_name: projection.account_display_name,
            created_at: projection.created_at,
            device_display_name: projection.device_display_name,
            device_type: projection.device_type,
            device_created_at: projection.device_created_at,
            last_seen_at: projection.last_seen_at,
        })
        .into_response(),
        &request_id,
    )
}

/// Tombstones the account only after native authentication and fresh browser passkey proof.
async fn delete_account(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    if !is_trusted_browser_request(&headers)
        || !trusted_proxy_header_matches(&headers, state.trusted_proxy_token.as_deref())
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "untrusted_request",
            "The request must originate from the trusted first-party proxy.",
            &request_id,
            json!({}),
        );
    }
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let Some(session) = match_native_session(&state, &headers, &request_id).await else {
        return unauthorized_response(&request_id);
    };
    let Some(cookie) = cookie_value(&headers, "rockserver_browser") else {
        return unauthorized_response(&request_id);
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 256)
    else {
        return error_response(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "A valid CSRF token is required.",
            &request_id,
            json!({}),
        );
    };
    match store
        .browser_session_is_fresh_for_user(session.user_id, &token_hash(cookie), &token_hash(csrf))
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return error_response(
                StatusCode::FORBIDDEN,
                "reauthentication_required",
                "A recent passkey assertion is required.",
                &request_id,
                json!({}),
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Account service is unavailable.",
                &request_id,
                json!({}),
            );
        }
    }
    match store.delete_account(session.user_id).await {
        Ok(true) => with_request_id(StatusCode::ACCEPTED.into_response(), &request_id),
        Ok(false) => unauthorized_response(&request_id),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Lists active native devices owned by the authenticated account.
async fn list_devices(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let Some(session) = match_native_session(&state, &headers, &request_id).await else {
        return unauthorized_response(&request_id);
    };
    match store.list_owned_devices(session.user_id).await {
        Ok(devices) => with_request_id(
            Json(DeviceListDto {
                devices: devices
                    .into_iter()
                    .map(|device| DeviceDto {
                        device_id: device.id.to_string(),
                        user_id: device.user_id.to_string(),
                        device_display_name: device.device_display_name,
                        device_type: device.device_type,
                        created_at: device.created_at,
                        last_seen_at: device.last_seen_at,
                    })
                    .collect(),
            })
            .into_response(),
            &request_id,
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Revokes one device only when it belongs to the authenticated account.
async fn revoke_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_id): Path<Uuid>,
) -> Response {
    let request_id = request_id(&headers);
    let Some(store) = state.account_store.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    let Some(session) = match_native_session(&state, &headers, &request_id).await else {
        return unauthorized_response(&request_id);
    };
    match store.revoke_owned_device(session.user_id, device_id).await {
        Ok(true) => with_request_id(StatusCode::NO_CONTENT.into_response(), &request_id),
        Ok(false) => error_response(
            StatusCode::NOT_FOUND,
            "device_not_found",
            "Device is unavailable.",
            &request_id,
            json!({}),
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

async fn match_native_session(
    state: &AppState,
    headers: &HeaderMap,
    request_id: &str,
) -> Option<crate::auth::ActiveSession> {
    let token = bearer_token(headers)?;
    let store = state.account_store.as_ref()?;
    match store
        .find_active_session_by_access_hash(&token_hash(token))
        .await
    {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!(%request_id, "native session lookup failed");
            None
        }
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty() && value.len() <= 512)
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').map(str::trim).find_map(|cookie| {
                cookie
                    .strip_prefix(name)
                    .and_then(|value| value.strip_prefix('='))
            })
        })
}

/// Enforces the first-party HTTPS origin and proxy protocol marker for browser state changes.
fn is_trusted_browser_request(headers: &HeaderMap) -> bool {
    headers
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin == FIRST_PARTY_ORIGIN)
        && headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|protocol| protocol.eq_ignore_ascii_case("https"))
}

/// Requires the shared header that only the configured Caddy proxy may inject.
fn trusted_proxy_header_matches(headers: &HeaderMap, expected: Option<&str>) -> bool {
    let Some(expected) = expected.filter(|value| !value.is_empty()) else {
        return false;
    };
    let Some(actual) = headers
        .get(TRUSTED_PROXY_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    constant_time_eq(actual.as_bytes(), expected.as_bytes())
}

/// Selects a stable, bounded rate-limit scope without trusting forwarded identity from direct peers.
fn request_rate_limit_scope(headers: &HeaderMap, expected_proxy_token: Option<&str>) -> String {
    if trusted_proxy_header_matches(headers, expected_proxy_token) {
        if let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        {
            let bounded: String = forwarded.chars().take(256).collect();
            if !bounded.trim().is_empty() {
                return bounded;
            }
        }
        return "trusted-proxy-unknown-client".to_owned();
    }
    "direct-peer".to_owned()
}

/// Hashes an opaque proof before it reaches the persistence boundary.
fn token_hash(value: &str) -> SecretHash {
    SecretHash::new(Sha256::digest(value.as_bytes()).into())
}

/// Produces a human-verifiable phrase from random server-generated request material.
fn pairing_phrase(request_id: &Uuid) -> String {
    const WORDS: [&str; 16] = [
        "AMBER", "BIRCH", "CORAL", "DAWN", "EMBER", "FJORD", "GROVE", "HARBOR", "IVORY", "JUNIPER",
        "KITE", "LUNAR", "MAPLE", "NOVA", "OCEAN", "PINE",
    ];
    let bytes = request_id.as_bytes();
    format!(
        "{}-{}",
        WORDS[usize::from(bytes[0] & 15)],
        WORDS[usize::from(bytes[1] & 15)]
    )
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
    voice_stream_impl(state, headers, upgrade, request_id, None).await
}

/// Admits the approved anonymous WebSocket voice session without trusting forwarded headers.
async fn public_voice_stream(
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

#[derive(Deserialize)]
struct CatalogQuery {
    #[serde(default)]
    limit: Option<u8>,
    #[serde(default)]
    cursor: Option<String>,
}

/// Lists the bounded active public catalog using an opaque stable-ID cursor.
async fn public_catalog_list(
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
async fn public_catalog_get(
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

async fn search(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let request_id = request_id(&headers);
    if !state.is_authorized(&headers) {
        return unauthorized_response(&request_id);
    }
    search_impl(state, headers, body, request_id, 50).await
}

/// Serves the approved anonymous, bounded station-search operation.
async fn public_search(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let request_id = request_id(&headers);
    if let Err(response) = state.public_request_allowed("search", SEARCH_LIMIT, &request_id) {
        return *response;
    }
    search_impl(state, headers, body, request_id, 20).await
}

async fn search_impl(
    state: AppState,
    headers: HeaderMap,
    body: Body,
    request_id: String,
    max_limit: u8,
) -> Response {
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
                query: validated.query,
                locale: validated.locale,
            },
            &constraints,
        ),
    )
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_)) => {
            tracing::warn!(%request_id, endpoint = "search", "public-safe search failure");
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
                "Station search timed out.",
                &request_id,
                json!({"timeout_ms": state.voice_command_timeout.as_millis()}),
            );
        }
    };
    tracing::info!(%request_id, endpoint = "search", status = 200, stations = outcome.stations.len(), "public request completed");

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
    voice_command_impl(state, headers, body, request_id, 50).await
}

/// Serves the approved anonymous, transcript-only voice command operation.
async fn public_voice_command(
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
    let body = to_bytes(body, MAX_PUBLIC_JSON_REQUEST_BODY_BYTES)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "Request body exceeds the allowed size.",
                request_id,
                json!({"max_bytes": MAX_PUBLIC_JSON_REQUEST_BODY_BYTES}),
            )
        })?;
    let value = serde_json::from_slice::<Value>(&body).map_err(|_error| {
        error_response(
            StatusCode::BAD_REQUEST,
            "malformed_request",
            "Request body must contain valid JSON.",
            request_id,
            json!({"field":"body"}),
        )
    })?;
    serde_json::from_value(value).map_err(|_error| {
        error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "Request validation failed.",
            request_id,
            json!({"field":"request"}),
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

struct VoiceSlot {
    limiter: Arc<Mutex<PublicLimitState>>,
}
impl Drop for VoiceSlot {
    fn drop(&mut self) {
        if let Ok(mut state) = self.limiter.lock() {
            state.active_voice = state.active_voice.saturating_sub(1);
        }
    }
}

fn retry_after(mut response: Response, seconds: u64) -> Response {
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&seconds.to_string())
            .expect("positive retry-after is a valid header"),
    );
    response
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

/// Minimal public station representation that excludes ranking and provider metadata.
#[derive(Clone, Serialize)]
struct PublicStationDto {
    id: String,
    name: String,
    stream_url: String,
    homepage_url: Option<String>,
    tags: Vec<String>,
    language: Option<String>,
    country_code: Option<String>,
    codec: Option<String>,
    bitrate_kbps: Option<u32>,
}

impl From<&crate::search::Station> for PublicStationDto {
    fn from(station: &crate::search::Station) -> Self {
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
        }
    }
}

/// Bounded public catalog page with an opaque next cursor.
#[derive(Serialize)]
struct CatalogPageDto {
    request_id: String,
    stations: Vec<PublicStationDto>,
    next_cursor: Option<String>,
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
        http::{HeaderMap, Request, StatusCode, header},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::{
        FIRST_PARTY_ORIGIN, HealthResponse, HealthStatus, PairingPreviewDto, TEST_API_BEARER_TOKEN,
        is_trusted_browser_request, request_rate_limit_scope, router, trusted_proxy_header_matches,
        validated_device_display_name,
    };

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

    #[tokio::test]
    async fn pairing_creation_fails_closed_without_the_postgres_account_store() {
        let response = router()
            .oneshot(
                Request::post("/v1/pairing-requests")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"device_display_name":"Test desktop","device_type":"windows"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "auth_unavailable"
        );
    }

    #[tokio::test]
    async fn pairing_completion_rejects_a_client_supplied_owner() {
        let response = router()
            .oneshot(
                Request::post("/v1/pairing-requests/00000000-0000-0000-0000-000000000000/complete")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"desktop_token":"0123456789abcdef","user_id":"00000000-0000-0000-0000-000000000000"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn passkey_authentication_rejects_a_client_supplied_account_identifier() {
        let response = router()
            .oneshot(
                Request::post("/v1/auth/passkeys/authentication/verify")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"challenge_id":"00000000-0000-0000-0000-000000000000","user_id":"00000000-0000-0000-0000-000000000000","id":"x","authenticatorData":"x","signature":"x","clientDataJSON":"x","userHandle":"x"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn browser_session_refresh_rejects_direct_requests() {
        let response = router()
            .oneshot(
                Request::post("/v1/auth/browser-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn pairing_preview_serializes_only_browser_safe_context() {
        let value = serde_json::to_value(PairingPreviewDto {
            request_id: "00000000-0000-0000-0000-000000000000".into(),
            device_display_name: "RockCast — This PC".into(),
            device_type: "rockcast_windows".into(),
            app_version: None,
            verification_phrase: "AMBER-FJORD".into(),
            short_code: "A1B2C3D4".into(),
            expires_at: "2030-01-01T00:00:00Z".into(),
            status: "pending",
            account_display_name: Some("Alex's Rock account".into()),
        })
        .unwrap();
        assert_eq!(value["status"], "pending");
        assert!(value.get("desktop_token").is_none());
        assert!(value.get("approval_secret").is_none());
        assert!(value.get("credential_id").is_none());
        assert!(value.get("refresh_token").is_none());
    }

    #[test]
    fn browser_state_changes_require_first_party_https_proxy_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("origin", FIRST_PARTY_ORIGIN.parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(is_trusted_browser_request(&headers));
        headers.insert("origin", "https://evil.example".parse().unwrap());
        assert!(!is_trusted_browser_request(&headers));
    }

    #[test]
    fn browser_state_changes_require_the_configured_proxy_secret() {
        let mut headers = HeaderMap::new();
        headers.insert("x-rockserver-proxy", "proxy-secret".parse().unwrap());
        assert!(trusted_proxy_header_matches(&headers, Some("proxy-secret")));
        assert!(!trusted_proxy_header_matches(
            &headers,
            Some("wrong-secret")
        ));
        assert!(!trusted_proxy_header_matches(&headers, None));
    }

    #[test]
    fn rate_limit_scope_ignores_forwarded_identity_from_direct_peers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "198.51.100.10".parse().unwrap());
        assert_eq!(
            request_rate_limit_scope(&headers, Some("proxy-secret")),
            "direct-peer"
        );
        headers.insert("x-rockserver-proxy", "proxy-secret".parse().unwrap());
        assert_eq!(
            request_rate_limit_scope(&headers, Some("proxy-secret")),
            "198.51.100.10"
        );
    }

    #[test]
    fn device_name_validation_trims_but_rejects_empty_control_and_overlong_values() {
        assert_eq!(
            validated_device_display_name("  Living room PC  "),
            Some("Living room PC")
        );
        assert_eq!(validated_device_display_name("Rock\nCast"), None);
        assert_eq!(validated_device_display_name(&"a".repeat(129)), None);
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

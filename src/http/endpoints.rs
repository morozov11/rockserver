//! HTTP route composition for the RockServer API.

use std::{
    env,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::Router;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::{
    persistence::{PostgresAccountStore, PostgresAdminStore},
    search::{
        InMemoryStationRepository, SearchService, StationRepository, UnavailableStationRepository,
    },
    voice::{SpeechRecognizers, UnavailableSpeechRecognizer},
};

#[path = "account.rs"]
mod account;
#[path = "admin_auth.rs"]
mod admin_auth;
#[path = "auth.rs"]
mod auth;
#[path = "catalog.rs"]
mod catalog;
#[path = "health.rs"]
mod health;
#[path = "pairing.rs"]
mod pairing;
#[path = "search.rs"]
mod search;
#[path = "state.rs"]
mod state;
#[path = "transport.rs"]
mod transport;
#[path = "voice.rs"]
mod voice;

pub use health::{HealthResponse, HealthStatus};
use state::{AppState, PublicLimitState};

/// Deterministic credential used only by convenience routers in offline tests and examples.
///
/// Production startup must supply a unique secret through [`router_with_services_and_bearer_token`].
pub const TEST_API_BEARER_TOKEN: &str = "rockserver-offline-test-token";

/// Environment variable containing the secret Caddy injects into trusted browser requests.
pub const TRUSTED_PROXY_TOKEN_ENV: &str = "ROCKSERVER_TRUSTED_PROXY_TOKEN";

/// Maximum duration the voice-command transport waits for query interpretation and search.
pub const DEFAULT_VOICE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

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
pub fn router_with_services(
    search_service: SearchService,
    speech_recognizer: Arc<dyn crate::voice::StreamingSpeechRecognizer>,
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
pub fn router_with_services_and_bearer_token(
    search_service: SearchService,
    speech_recognizer: Arc<dyn crate::voice::StreamingSpeechRecognizer>,
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

/// Creates the router with recognizers selectable by each voice WebSocket session.
pub fn router_with_speech_recognizers_and_bearer_token(
    search_service: SearchService,
    speech_recognizers: SpeechRecognizers,
    voice_command_timeout: Duration,
    api_bearer_token: impl Into<String>,
) -> Router {
    build_router(AppState {
        search_service,
        speech_recognizers,
        voice_command_timeout,
        api_bearer_token: api_bearer_token.into(),
        account_store: None,
        admin_store: None,
        trusted_proxy_token: None,
        public_limits: Arc::new(Mutex::new(PublicLimitState::default())),
    })
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
    build_router(AppState {
        search_service,
        speech_recognizers,
        voice_command_timeout,
        api_bearer_token: api_bearer_token.into(),
        account_store: Some(account_store),
        admin_store: None,
        trusted_proxy_token: Some(trusted_proxy_token.into()),
        public_limits: Arc::new(Mutex::new(PublicLimitState::default())),
    })
}

/// Creates the production router with the separate durable administrator-authentication store.
pub fn router_with_speech_recognizers_bearer_account_admin_store_and_proxy(
    search_service: SearchService,
    speech_recognizers: SpeechRecognizers,
    voice_command_timeout: Duration,
    api_bearer_token: impl Into<String>,
    account_store: PostgresAccountStore,
    admin_store: PostgresAdminStore,
    trusted_proxy_token: impl Into<String>,
) -> Router {
    build_router(AppState {
        search_service,
        speech_recognizers,
        voice_command_timeout,
        api_bearer_token: api_bearer_token.into(),
        account_store: Some(account_store),
        admin_store: Some(Arc::new(admin_store)),
        trusted_proxy_token: Some(trusted_proxy_token.into()),
        public_limits: Arc::new(Mutex::new(PublicLimitState::default())),
    })
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/v1/admin/auth/login",
            axum::routing::post(admin_auth::login),
        )
        .route(
            "/v1/admin/auth/refresh",
            axum::routing::post(admin_auth::refresh),
        )
        .route(
            "/v1/admin/auth/logout",
            axum::routing::post(admin_auth::logout),
        )
        .route("/v1/admin/session", axum::routing::get(admin_auth::session))
        .route("/admin", axum::routing::get(health::admin_console))
        .route("/health/live", axum::routing::get(health::live))
        .route("/health/ready", axum::routing::get(health::ready))
        .route(
            "/v1/catalog/stations",
            axum::routing::get(catalog::public_catalog_list),
        )
        .route(
            "/v1/catalog/stations/{station_id}",
            axum::routing::get(catalog::public_catalog_get),
        )
        .route("/api/v1/search", axum::routing::post(search::search))
        .route("/v1/search", axum::routing::post(search::public_search))
        .route(
            "/api/v1/voice/command",
            axum::routing::post(voice::voice_command),
        )
        .route(
            "/v1/voice/command",
            axum::routing::post(voice::public_voice_command),
        )
        .route(
            "/api/v1/voice/stream",
            axum::routing::get(voice::voice_stream),
        )
        .route(
            "/v1/voice/stream",
            axum::routing::get(voice::public_voice_stream),
        )
        .route(
            "/v1/pairing-requests",
            axum::routing::post(pairing::create_pairing_request),
        )
        .route(
            "/v1/pairing-requests/lookup",
            axum::routing::get(pairing::lookup_pairing_request),
        )
        .route(
            "/v1/auth/browser-session",
            axum::routing::post(auth::browser_session),
        )
        .route(
            "/v1/browser/account",
            axum::routing::get(account::browser_account),
        )
        .route(
            "/v1/auth/browser-logout",
            axum::routing::post(account::logout_browser_session),
        )
        .route(
            "/v1/pairing-requests/{request_id}/approve",
            axum::routing::post(pairing::approve_pairing_request),
        )
        .route(
            "/v1/auth/passkeys/registration/options",
            axum::routing::post(auth::registration_options),
        )
        .route(
            "/v1/auth/passkeys/registration/verify",
            axum::routing::post(auth::registration_verify),
        )
        .route(
            "/v1/auth/passkeys/authentication/options",
            axum::routing::post(auth::authentication_options),
        )
        .route(
            "/v1/auth/passkeys/authentication/verify",
            axum::routing::post(auth::authentication_verify),
        )
        .route(
            "/v1/pairing-requests/{request_id}/complete",
            axum::routing::post(pairing::complete_pairing_request),
        )
        .route(
            "/v1/auth/device-session",
            axum::routing::post(auth::create_device_session),
        )
        .route(
            "/v1/account/profile",
            axum::routing::get(account::account_profile),
        )
        .route(
            "/v1/account",
            axum::routing::delete(account::delete_account),
        )
        .route("/v1/devices", axum::routing::get(account::list_devices))
        .route(
            "/v1/devices/{device_id}",
            axum::routing::delete(account::revoke_device),
        )
        .route(
            "/v1/browser/devices/{device_id}",
            axum::routing::patch(account::rename_browser_device)
                .delete(account::revoke_browser_device),
        )
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::{
        TEST_API_BEARER_TOKEN,
        health::{HealthResponse, HealthStatus},
        router,
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

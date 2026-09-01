//! Shared HTTP state and process-local admission controls.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use serde_json::json;

use crate::{
    admin::AdminStore, persistence::PostgresAccountStore, search::SearchService,
    voice::SpeechRecognizers,
};

use super::{
    transport::{constant_time_eq, error_response, retry_after},
    voice::VoiceSlot,
};

const GLOBAL_VOICE_SESSIONS: usize = 100;
const RATE_WINDOW: Duration = Duration::from_secs(60);
const RETRY_AFTER_SECONDS: u64 = 60;
const VOICE_CAPACITY_RETRY_AFTER_SECONDS: u64 = 30;

#[derive(Clone, Copy)]
/// Per-endpoint anonymous request and burst limits.
pub(super) struct PublicLimit {
    pub(super) requests: usize,
    pub(super) burst: usize,
}

#[derive(Clone)]
pub(super) struct AppState {
    pub(super) search_service: SearchService,
    pub(super) speech_recognizers: SpeechRecognizers,
    pub(super) voice_command_timeout: Duration,
    pub(super) api_bearer_token: String,
    pub(super) account_store: Option<PostgresAccountStore>,
    pub(super) admin_store: Option<Arc<dyn AdminStore>>,
    pub(super) trusted_proxy_token: Option<String>,
    pub(super) local_admin_origin: Option<String>,
    pub(super) public_limits: Arc<Mutex<PublicLimitState>>,
}

#[derive(Default)]
/// Process-local buckets used by anonymous HTTP admission control.
pub(super) struct PublicLimitState {
    pub(super) requests: HashMap<&'static str, Vec<std::time::Instant>>,
    pub(super) active_voice: usize,
}

impl AppState {
    /// Applies the fail-closed direct-peer anonymous quota.
    pub(super) fn public_request_allowed(
        &self,
        endpoint: &'static str,
        limit: PublicLimit,
        request_id: &str,
    ) -> Result<(), Box<Response>> {
        let mut state = self
            .public_limits
            .lock()
            .expect("public limiter mutex is not poisoned");
        let now = std::time::Instant::now();
        let bucket = state.requests.entry(endpoint).or_default();
        bucket.retain(|seen| now.duration_since(*seen) < RATE_WINDOW);
        if bucket.len() >= limit.burst {
            tracing::warn!(%request_id, endpoint, limit_scope = "direct_peer", requests_per_minute = limit.requests, "public rate limit rejected");
            return Err(Box::new(retry_after(
                error_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    "Request rate limit exceeded.",
                    request_id,
                    json!({"limit_scope":"direct_peer"}),
                ),
                RETRY_AFTER_SECONDS,
            )));
        }
        bucket.push(now);
        Ok(())
    }

    /// Reserves a global public voice slot before a WebSocket upgrade.
    pub(super) fn reserve_voice_slot(&self, request_id: &str) -> Result<VoiceSlot, Box<Response>> {
        let mut state = self
            .public_limits
            .lock()
            .expect("public limiter mutex is not poisoned");
        if state.active_voice >= GLOBAL_VOICE_SESSIONS {
            return Err(Box::new(retry_after(
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "voice_capacity_exhausted",
                    "Voice service is temporarily at capacity.",
                    request_id,
                    json!({}),
                ),
                VOICE_CAPACITY_RETRY_AFTER_SECONDS,
            )));
        }
        state.active_voice += 1;
        Ok(VoiceSlot {
            limiter: Arc::clone(&self.public_limits),
        })
    }

    /// Verifies the configured opaque application credential without logging the supplied value.
    pub(super) fn is_authorized(&self, headers: &HeaderMap) -> bool {
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

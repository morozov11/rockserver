//! PostgreSQL persistence for the approved passkey-only account ownership model.

use passkey_auth::{CosePublicKey, CredentialId, PasskeyCredential};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::account_cleanup::{
    CleanupAccount, CleanupAction, CleanupActionResult, CleanupCounts, CleanupDependency,
    CleanupError, CleanupPreview, validate_confirmation,
};
use crate::auth::{
    AccountProjection, ActiveSession, BrowserDevice, MAX_ACCOUNT_DEVICES, NativeSessionLookupError,
    NativeSessionResolver, NewBrowserSession, NewPairingRequest, NewPairingSession,
    NewPasskeyRegistration, NewSession, NewWebAuthnChallenge, OwnedDevice, PairingCompletion,
    PairingCompletionOutcome, PairingPreview, PasskeyRegistrationOutcome, SecretHash,
    WebAuthnCeremony, is_safe_audit_event,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// PostgreSQL store that persists only token hashes and safe audit classifications.
#[derive(Clone, Debug)]
pub struct PostgresAccountStore {
    pub(super) pool: PgPool,
}

/// Secret-free device projection for the administrator read-only inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminDeviceReadModel {
    /// Product-neutral device type supplied during pairing.
    pub device_type: String,
    /// User-facing pairing label.
    pub device_display_name: String,
    /// Active, inactive, or revoked lifecycle state.
    pub status: String,
    /// Device creation time in RFC 3339 UTC form.
    pub created_at: String,
    /// Last device or session activity time in RFC 3339 UTC form.
    pub last_seen_at: Option<String>,
}

#[path = "account_postgres/accounts.rs"]
mod accounts;
#[path = "account_postgres/browser.rs"]
mod browser;
#[path = "account_postgres/cleanup.rs"]
mod cleanup;
#[path = "account_postgres/core.rs"]
mod core;
#[path = "account_postgres/native_session.rs"]
mod native_session;
#[path = "account_postgres/pairing.rs"]
mod pairing;
#[path = "account_postgres/passkeys.rs"]
mod passkeys;
#[path = "account_postgres/rate_limits.rs"]
mod rate_limits;
#[path = "account_postgres/rows.rs"]
mod rows;

//! Private SQL row mappings and shared transactional helpers for the account store.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct CleanupAccountRow {
    pub(super) account_id: Uuid,
    pub(super) status: String,
    pub(super) created_at: String,
    pub(super) deleted_at: Option<String>,
    pub(super) protected: bool,
}

#[derive(sqlx::FromRow)]
pub(super) struct CleanupDependencyRow {
    pub(super) account_id: Uuid,
    pub(super) kind: String,
    pub(super) id: Uuid,
    pub(super) status: String,
    pub(super) created_at: String,
    pub(super) last_activity_at: Option<String>,
    pub(super) expires_at: Option<String>,
    pub(super) related_id: Option<Uuid>,
    pub(super) event_type: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(super) struct OperatorAccountRow {
    pub(super) status: String,
    pub(super) protected: bool,
}

/// Revokes all live account-owned access while retaining rows for audit and recovery review.
pub(super) async fn revoke_account_dependencies(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<CleanupCounts, sqlx::Error> {
    let devices = sqlx::query(
        "UPDATE devices SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    let sessions = sqlx::query(
        "UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    let passkeys = sqlx::query(
        "UPDATE passkey_credentials SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    let browser_sessions = sqlx::query(
        "UPDATE browser_sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    let account_identities = sqlx::query(
        "UPDATE account_identities SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    let pairing_requests = sqlx::query("UPDATE pairing_requests SET revoked_at = now() WHERE approved_by_user_id = $1 AND revoked_at IS NULL")
        .bind(user_id)
        .execute(&mut **transaction)
        .await?;
    let webauthn_challenges = sqlx::query(
        "UPDATE webauthn_challenges SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    Ok(CleanupCounts {
        passkeys: affected_count(passkeys),
        account_identities: affected_count(account_identities),
        devices: affected_count(devices),
        sessions: affected_count(sessions),
        browser_sessions: affected_count(browser_sessions),
        pairing_requests: affected_count(pairing_requests),
        webauthn_challenges: affected_count(webauthn_challenges),
        ..CleanupCounts::default()
    })
}

pub(super) fn affected_count(result: sqlx::postgres::PgQueryResult) -> usize {
    result.rows_affected().try_into().unwrap_or(usize::MAX)
}

pub(super) fn cleanup_dependency_reason(kind: &str, status: &str) -> String {
    if status != "active" && status != "approved" && status != "pending" {
        return "retained history; not actionable by this operator command".to_owned();
    }
    match kind {
        "passkey" => "manual review; credential display name is not stored server-side; retain the last working passkey".to_owned(),
        "device" => "manual review; select only an exact staging device ID after the browser account check".to_owned(),
        "account_identity" => "protected identity; do not select without explicit owner review".to_owned(),
        _ => "dependent record; revoke through its owning account or device action".to_owned(),
    }
}

#[derive(sqlx::FromRow)]
pub(super) struct DeviceRow {
    pub(super) id: Uuid,
    pub(super) user_id: Uuid,
    pub(super) device_display_name: String,
    pub(super) device_type: String,
    pub(super) created_at: String,
    pub(super) last_seen_at: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(super) struct BrowserDeviceRow {
    pub(super) id: Uuid,
    pub(super) device_display_name: String,
    pub(super) device_type: String,
    pub(super) created_at: String,
    pub(super) last_seen_at: Option<String>,
    pub(super) session_status: String,
}

#[derive(sqlx::FromRow)]
pub(super) struct AdminDeviceReadModelRow {
    pub(super) device_type: String,
    pub(super) device_display_name: String,
    pub(super) status: String,
    pub(super) created_at: String,
    pub(super) last_seen_at: Option<String>,
}

impl From<AdminDeviceReadModelRow> for AdminDeviceReadModel {
    fn from(row: AdminDeviceReadModelRow) -> Self {
        Self {
            device_type: row.device_type,
            device_display_name: row.device_display_name,
            status: row.status,
            created_at: row.created_at,
            last_seen_at: row.last_seen_at,
        }
    }
}

#[derive(sqlx::FromRow)]
pub(super) struct AccountProjectionRow {
    pub(super) account_display_name: String,
    pub(super) created_at: String,
    pub(super) device_display_name: String,
    pub(super) device_type: String,
    pub(super) device_created_at: String,
    pub(super) last_seen_at: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(super) struct ActiveSessionRow {
    pub(super) session_id: Uuid,
    pub(super) user_id: Uuid,
    pub(super) device_id: Uuid,
}

impl From<ActiveSessionRow> for ActiveSession {
    fn from(row: ActiveSessionRow) -> Self {
        Self {
            session_id: row.session_id,
            user_id: row.user_id,
            device_id: row.device_id,
        }
    }
}

#[derive(sqlx::FromRow)]
pub(super) struct PasskeyCredentialRow {
    pub(super) credential_id: Vec<u8>,
    pub(super) public_key: Vec<u8>,
    pub(super) sign_count: i64,
    pub(super) transports: Vec<String>,
}
impl From<DeviceRow> for OwnedDevice {
    fn from(row: DeviceRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            device_display_name: row.device_display_name,
            device_type: row.device_type,
            created_at: row.created_at,
            last_seen_at: row.last_seen_at,
        }
    }
}

impl From<BrowserDeviceRow> for BrowserDevice {
    fn from(row: BrowserDeviceRow) -> Self {
        Self {
            id: row.id,
            device_display_name: row.device_display_name,
            device_type: row.device_type,
            created_at: row.created_at,
            last_seen_at: row.last_seen_at,
            session_status: row.session_status,
        }
    }
}
impl From<AccountProjectionRow> for AccountProjection {
    fn from(row: AccountProjectionRow) -> Self {
        Self {
            account_display_name: row.account_display_name,
            created_at: row.created_at,
            device_display_name: row.device_display_name,
            device_type: row.device_type,
            device_created_at: row.device_created_at,
            last_seen_at: row.last_seen_at,
        }
    }
}
#[derive(sqlx::FromRow)]
pub(super) struct PairingRow {
    pub(super) approved_by_user_id: Option<Uuid>,
    pub(super) device_display_name: String,
    pub(super) device_type: String,
    pub(super) app_version: Option<String>,
    pub(super) approved: bool,
    pub(super) consumed: bool,
    pub(super) revoked: bool,
    pub(super) expired: bool,
}

#[derive(sqlx::FromRow)]
pub(super) struct PairingPreviewRow {
    pub(super) id: Uuid,
    pub(super) device_display_name: String,
    pub(super) device_type: String,
    pub(super) app_version: Option<String>,
    pub(super) verification_phrase: String,
    pub(super) expires_at: String,
}

impl From<PairingPreviewRow> for PairingPreview {
    fn from(row: PairingPreviewRow) -> Self {
        Self {
            request_id: row.id,
            device_display_name: row.device_display_name,
            device_type: row.device_type,
            app_version: row.app_version,
            verification_phrase: row.verification_phrase,
            expires_at: row.expires_at,
        }
    }
}

pub(super) async fn audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Option<Uuid>,
    device_id: Option<Uuid>,
    event_type: &str,
) -> Result<Uuid, sqlx::Error> {
    debug_assert!(is_safe_audit_event(event_type));
    let event_id = Uuid::new_v4();
    sqlx::query("INSERT INTO account_audit_events (id, user_id, device_id, event_type) VALUES ($1, $2, $3, $4)")
        .bind(event_id)
        .bind(user_id)
        .bind(device_id)
        .bind(event_type)
        .execute(&mut **transaction)
        .await?;
    Ok(event_id)
}

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
    pool: PgPool,
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

impl PostgresAccountStore {
    /// Connects to PostgreSQL and applies the shared versioned migration sequence.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        if let Err(error) = MIGRATOR.run(&pool).await {
            pool.close().await;
            return Err(error.into());
        }
        Ok(Self { pool })
    }

    /// Reuses a caller-managed migrated pool, primarily for integration tests.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Closes the underlying pool for deterministic integration-test cleanup.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Creates one active passkey-only account without contact or password data.
    pub async fn create_user(&self, user_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO users (id) VALUES ($1)")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Creates an account with the explicit user-facing label used for its passkey and browser flow.
    pub async fn create_user_with_display_name(
        &self,
        user_id: Uuid,
        account_display_name: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO users (id, account_display_name) VALUES ($1, $2)")
            .bind(user_id)
            .bind(account_display_name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Creates a fixture device with a deterministic placeholder credential hash.
    ///
    /// Production devices are created only by pairing, which generates a random device secret.
    pub async fn create_device(&self, device: &OwnedDevice) -> Result<bool, sqlx::Error> {
        let mut test_only_secret_hash = [0_u8; 32];
        test_only_secret_hash[..16].copy_from_slice(device.id.as_bytes());
        test_only_secret_hash[16..].copy_from_slice(device.id.as_bytes());
        let inserted = sqlx::query(
            "INSERT INTO devices (id, user_id, device_display_name, device_type, device_secret_hash) \
             SELECT $1, id, $3, $4, $5 FROM users WHERE id = $2 AND status = 'active'",
        )
        .bind(device.id)
        .bind(device.user_id)
        .bind(&device.device_display_name)
        .bind(&device.device_type)
        .bind(test_only_secret_hash)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Returns a device only if the caller owns it and neither record has been revoked/deleted.
    pub async fn find_owned_device(
        &self,
        user_id: Uuid,
        device_id: Uuid,
    ) -> Result<Option<OwnedDevice>, sqlx::Error> {
        sqlx::query_as::<_, DeviceRow>(
            "SELECT d.id, d.user_id, d.device_display_name, d.device_type, to_char(d.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at, to_char(d.last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS last_seen_at FROM devices d \
             JOIN users u ON u.id = d.user_id \
             WHERE d.id = $1 AND d.user_id = $2 AND d.revoked_at IS NULL AND u.status = 'active'",
        )
        .bind(device_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(Into::into))
    }

    /// Resolves a non-expired access token hash to its active account/session owner.
    pub async fn find_active_session_by_access_hash(
        &self,
        access_hash: &SecretHash,
    ) -> Result<Option<ActiveSession>, sqlx::Error> {
        sqlx::query_as::<_, ActiveSessionRow>(
            "SELECT s.id AS session_id, s.user_id, s.device_id FROM sessions s \
             JOIN users u ON u.id = s.user_id JOIN devices d ON d.id = s.device_id \
             WHERE s.access_token_hash = $1 AND s.revoked_at IS NULL AND s.access_expires_at > now() \
             AND u.status = 'active' AND d.revoked_at IS NULL AND d.user_id = s.user_id",
        )
        .bind(access_hash.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(Into::into))
    }

    /// Returns the account and current-device labels only for an already authenticated native owner.
    pub async fn account_projection(
        &self,
        user_id: Uuid,
        device_id: Uuid,
    ) -> Result<Option<AccountProjection>, sqlx::Error> {
        sqlx::query_as::<_, AccountProjectionRow>(
            "SELECT u.account_display_name, to_char(u.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at, d.device_display_name, d.device_type, \
             to_char(d.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS device_created_at, to_char(d.last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS last_seen_at \
             FROM users u JOIN devices d ON d.user_id = u.id \
             WHERE u.id = $1 AND d.id = $2 AND u.status = 'active' AND d.revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(device_id)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(Into::into))
    }

    /// Resolves a browser account label from an unexpired opaque cookie hash without exposing its owner ID.
    pub async fn browser_account_display_name(
        &self,
        session_token_hash: &SecretHash,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT u.account_display_name FROM browser_sessions b JOIN users u ON u.id = b.user_id \
             WHERE b.session_token_hash = $1 AND b.revoked_at IS NULL AND b.expires_at > now() AND u.status = 'active'",
        )
        .bind(session_token_hash.as_bytes())
        .fetch_optional(&self.pool)
        .await
    }

    /// Rotates the tab-local CSRF proof and returns the active account display name.
    pub async fn rotate_browser_csrf(
        &self,
        session_token_hash: &SecretHash,
        csrf_token_hash: &SecretHash,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(
            "UPDATE browser_sessions b SET csrf_token_hash = $2 \
             FROM users u WHERE b.user_id = u.id AND b.session_token_hash = $1 \
             AND b.revoked_at IS NULL AND b.expires_at > now() AND u.status = 'active' \
             RETURNING u.account_display_name",
        )
        .bind(session_token_hash.as_bytes())
        .bind(csrf_token_hash.as_bytes())
        .fetch_optional(&self.pool)
        .await
    }

    /// Returns an active account's display name for a server-derived owner ID.
    pub async fn account_display_name(&self, user_id: Uuid) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT account_display_name FROM users WHERE id = $1 AND status = 'active'",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Lists only active devices owned by the requested account.
    pub async fn list_owned_devices(&self, user_id: Uuid) -> Result<Vec<OwnedDevice>, sqlx::Error> {
        sqlx::query_as::<_, DeviceRow>(
            "SELECT d.id, d.user_id, d.device_display_name, d.device_type, to_char(d.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at, to_char(d.last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS last_seen_at FROM devices d \
             JOIN users u ON u.id = d.user_id WHERE d.user_id = $1 AND d.revoked_at IS NULL \
             AND u.status = 'active' ORDER BY d.created_at, d.id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().map(Into::into).collect())
    }

    /// Revokes one device and all of its native access sessions.
    pub async fn revoke_owned_device(
        &self,
        user_id: Uuid,
        device_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE devices SET revoked_at = now() WHERE id = $1 AND user_id = $2 \
             AND revoked_at IS NULL AND EXISTS (SELECT 1 FROM users WHERE id = $2 AND status = 'active')",
        )
        .bind(device_id)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        sqlx::query("UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND device_id = $2 AND revoked_at IS NULL")
            .bind(user_id).bind(device_id).execute(&mut *transaction).await?;
        audit(
            &mut transaction,
            Some(user_id),
            Some(device_id),
            "device_revoked",
        )
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Persists one short-lived native access session.
    pub async fn create_session(&self, session: NewSession<'_>) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO sessions (id, user_id, device_id, access_token_hash, access_expires_at) \
             SELECT $1, $2, d.id, $4, $5::timestamptz FROM devices d JOIN users u ON u.id = d.user_id \
             WHERE d.id = $3 AND d.user_id = $2 AND d.revoked_at IS NULL AND u.status = 'active'",
        )
        .bind(session.session_id).bind(session.user_id).bind(session.device_id).bind(session.access_hash.as_bytes())
        .bind(session.access_expires_at_rfc3339).execute(&mut *transaction).await?;
        if inserted.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        transaction.commit().await?;
        Ok(true)
    }

    /// Replaces a device's access session after validating its persistent opaque credential.
    pub async fn issue_device_session(
        &self,
        device_id: Uuid,
        device_secret_hash: &SecretHash,
        session_id: Uuid,
        access_hash: &SecretHash,
    ) -> Result<Option<String>, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let owner = sqlx::query_scalar::<_, Uuid>(
            "SELECT d.user_id FROM devices d JOIN users u ON u.id = d.user_id \
             WHERE d.id = $1 AND d.device_secret_hash = $2 AND d.revoked_at IS NULL \
             AND u.status = 'active' FOR UPDATE",
        )
        .bind(device_id)
        .bind(device_secret_hash.as_bytes())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(user_id) = owner else {
            transaction.rollback().await?;
            return Ok(None);
        };
        sqlx::query(
            "UPDATE sessions SET revoked_at = now() WHERE device_id = $1 AND revoked_at IS NULL",
        )
        .bind(device_id)
        .execute(&mut *transaction)
        .await?;
        let access_expires_at = sqlx::query_scalar::<_, String>(
            "INSERT INTO sessions (id, user_id, device_id, access_token_hash, access_expires_at) \
             VALUES ($1, $2, $3, $4, now() + interval '15 minutes') \
             RETURNING to_char(access_expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(device_id)
        .bind(access_hash.as_bytes())
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("UPDATE devices SET last_seen_at = now() WHERE id = $1")
            .bind(device_id)
            .execute(&mut *transaction)
            .await?;
        audit(
            &mut transaction,
            Some(user_id),
            Some(device_id),
            "device_session_issued",
        )
        .await?;
        transaction.commit().await?;
        Ok(Some(access_expires_at))
    }

    /// Creates a first-party browser session only for an active account.
    pub async fn create_browser_session(
        &self,
        session: NewBrowserSession<'_>,
    ) -> Result<bool, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO browser_sessions (id, user_id, session_token_hash, csrf_token_hash, passkey_reauthenticated_at, expires_at) \
             SELECT $1, id, $3, $4, $5::timestamptz, $6::timestamptz FROM users WHERE id = $2 AND status = 'active'",
        )
        .bind(session.session_id)
        .bind(session.user_id)
        .bind(session.session_token_hash.as_bytes())
        .bind(session.csrf_hash.as_bytes())
        .bind(session.passkey_reauthenticated_at_rfc3339)
        .bind(session.expires_at_rfc3339)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Creates a browser session whose expiry and reauthentication time come from PostgreSQL's clock.
    pub async fn create_browser_session_for_minutes(
        &self,
        session: NewBrowserSession<'_>,
        lifetime_minutes: i32,
    ) -> Result<bool, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO browser_sessions (id, user_id, session_token_hash, csrf_token_hash, passkey_reauthenticated_at, expires_at) \
             SELECT $1, id, $3, $4, now(), now() + ($5 * interval '1 minute') FROM users WHERE id = $2 AND status = 'active'",
        )
        .bind(session.session_id)
        .bind(session.user_id)
        .bind(session.session_token_hash.as_bytes())
        .bind(session.csrf_hash.as_bytes())
        .bind(lifetime_minutes)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Revokes a browser session without exposing whether it was already revoked.
    pub async fn revoke_browser_session(&self, session_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE browser_sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Resolves an active browser session to its owner only when its current CSRF proof matches.
    pub async fn browser_session_user_with_csrf(
        &self,
        session_token_hash: &SecretHash,
        csrf_hash: &SecretHash,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT b.user_id FROM browser_sessions b JOIN users u ON u.id = b.user_id \
             WHERE b.session_token_hash = $1 AND b.csrf_token_hash = $2 AND b.revoked_at IS NULL \
             AND b.expires_at > now() AND u.status = 'active'",
        )
        .bind(session_token_hash.as_bytes())
        .bind(csrf_hash.as_bytes())
        .fetch_optional(&self.pool)
        .await
    }

    /// Resolves a live browser cookie to its owner without exposing that identifier to HTTP clients.
    pub async fn browser_session_user(
        &self,
        session_token_hash: &SecretHash,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT b.user_id FROM browser_sessions b JOIN users u ON u.id = b.user_id \
             WHERE b.session_token_hash = $1 AND b.revoked_at IS NULL AND b.expires_at > now() AND u.status = 'active'",
        )
        .bind(session_token_hash.as_bytes())
        .fetch_optional(&self.pool)
        .await
    }

    /// Lists only safe device metadata for the owner of an already authenticated browser session.
    pub async fn list_browser_devices(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<BrowserDevice>, sqlx::Error> {
        sqlx::query_as::<_, BrowserDeviceRow>(
            "SELECT d.id, d.device_display_name, d.device_type, \
             to_char(d.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at, \
             to_char(GREATEST(d.last_seen_at, (SELECT max(s.last_seen_at) FROM sessions s WHERE s.device_id = d.id)) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS last_seen_at, \
             CASE WHEN EXISTS (SELECT 1 FROM sessions s WHERE s.device_id = d.id AND s.revoked_at IS NULL AND s.access_expires_at > now()) THEN 'active' ELSE 'inactive' END AS session_status \
             FROM devices d JOIN users u ON u.id = d.user_id WHERE d.user_id = $1 AND d.revoked_at IS NULL AND u.status = 'active' ORDER BY d.created_at, d.id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().map(Into::into).collect())
    }

    /// Lists a stable, bounded administrator device page without returning account or credential IDs.
    pub async fn list_admin_devices(
        &self,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<AdminDeviceReadModel>, sqlx::Error> {
        sqlx::query_as::<_, AdminDeviceReadModelRow>(
            "SELECT d.device_type, d.device_display_name, \
             CASE WHEN d.revoked_at IS NOT NULL THEN 'revoked' WHEN EXISTS (SELECT 1 FROM sessions s WHERE s.device_id = d.id AND s.revoked_at IS NULL AND s.access_expires_at > now()) THEN 'active' ELSE 'inactive' END AS status, \
             to_char(d.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at, \
             to_char(GREATEST(d.last_seen_at, (SELECT max(s.last_seen_at) FROM sessions s WHERE s.device_id = d.id)) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS last_seen_at \
             FROM devices d ORDER BY d.created_at, d.device_display_name, d.id LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().map(Into::into).collect())
    }

    /// Renames one active device owned by the authenticated account and records a safe audit class.
    pub async fn rename_owned_device(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        name: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE devices SET device_display_name = $3 WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL \
             AND EXISTS (SELECT 1 FROM users WHERE id = $2 AND status = 'active')",
        )
        .bind(device_id)
        .bind(user_id)
        .bind(name)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        audit(
            &mut transaction,
            Some(user_id),
            Some(device_id),
            "device_renamed",
        )
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Revokes the caller's browser session and records a safe logout audit event.
    pub async fn logout_browser_session(
        &self,
        user_id: Uuid,
        session_token_hash: &SecretHash,
        csrf_hash: &SecretHash,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE browser_sessions SET revoked_at = now() WHERE user_id = $1 AND session_token_hash = $2 AND csrf_token_hash = $3 \
             AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(user_id)
        .bind(session_token_hash.as_bytes())
        .bind(csrf_hash.as_bytes())
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        audit(&mut transaction, Some(user_id), None, "browser_logout").await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Confirms a live browser session with a recent passkey assertion for one account.
    pub async fn browser_session_is_fresh_for_user(
        &self,
        user_id: Uuid,
        session_token_hash: &SecretHash,
        csrf_hash: &SecretHash,
    ) -> Result<bool, sqlx::Error> {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM browser_sessions b JOIN users u ON u.id = b.user_id \
             WHERE b.user_id = $1 AND b.session_token_hash = $2 AND b.csrf_token_hash = $3 \
             AND b.revoked_at IS NULL AND b.expires_at > now() \
             AND b.passkey_reauthenticated_at > now() - interval '2 minutes' AND u.status = 'active')",
        )
        .bind(user_id)
        .bind(session_token_hash.as_bytes())
        .bind(csrf_hash.as_bytes())
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    /// Stores a verified passkey public key and rejects duplicate authenticator credentials.
    pub async fn create_passkey_credential(
        &self,
        credential_id: Uuid,
        user_id: Uuid,
        credential_bytes: &[u8],
        public_key: &[u8],
        sign_count: i64,
        transports: &[String],
    ) -> Result<bool, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO passkey_credentials (id, user_id, credential_id, public_key, sign_count, transports) \
             SELECT $1, id, $3, $4, $5, $6 FROM users WHERE id = $2 AND status = 'active'",
        )
        .bind(credential_id)
        .bind(user_id)
        .bind(credential_bytes)
        .bind(public_key)
        .bind(sign_count)
        .bind(transports)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Atomically creates a new account, passkey, browser session, and consumed registration challenge.
    pub async fn complete_passkey_registration(
        &self,
        registration: NewPasskeyRegistration<'_>,
    ) -> Result<PasskeyRegistrationOutcome, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let consumed = sqlx::query(
            "UPDATE webauthn_challenges SET consumed_at = now() \
             WHERE id = $1 AND challenge_hash = $2 AND ceremony = 'registration' \
             AND origin = $3 AND rp_id = $4 AND user_id IS NULL \
             AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(registration.challenge_id)
        .bind(registration.challenge_hash.as_bytes())
        .bind(crate::auth::webauthn::ORIGIN)
        .bind(crate::auth::webauthn::RP_ID)
        .execute(&mut *transaction)
        .await?;
        if consumed.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(PasskeyRegistrationOutcome::ChallengeRejected);
        }

        let user_id = registration.user_id;
        sqlx::query("INSERT INTO users (id, account_display_name) VALUES ($1, $2)")
            .bind(user_id)
            .bind(registration.account_display_name)
            .execute(&mut *transaction)
            .await?;
        let credential_insert = sqlx::query(
            "INSERT INTO passkey_credentials (id, user_id, credential_id, public_key, sign_count, transports) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(registration.credential_id)
        .bind(user_id)
        .bind(registration.credential_bytes)
        .bind(registration.public_key)
        .bind(registration.sign_count)
        .bind(registration.transports)
        .execute(&mut *transaction)
        .await;
        if let Err(error) = credential_insert {
            if matches!(
                &error,
                sqlx::Error::Database(database_error)
                    if database_error.code().as_deref() == Some("23505")
            ) {
                transaction.rollback().await?;
                return Ok(PasskeyRegistrationOutcome::CredentialAlreadyRegistered);
            }
            return Err(error);
        }
        sqlx::query(
            "INSERT INTO browser_sessions (id, user_id, session_token_hash, csrf_token_hash, passkey_reauthenticated_at, expires_at) \
             VALUES ($1, $2, $3, $4, now(), now() + interval '30 minutes')",
        )
        .bind(registration.browser_session.session_id)
        .bind(user_id)
        .bind(registration.browser_session.session_token_hash.as_bytes())
        .bind(registration.browser_session.csrf_hash.as_bytes())
        .execute(&mut *transaction)
        .await?;
        audit(&mut transaction, Some(user_id), None, "account_registered").await?;
        transaction.commit().await?;
        Ok(PasskeyRegistrationOutcome::Created)
    }

    /// Returns active passkey credential identifiers for a live account.
    pub async fn list_passkey_credential_ids(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<CredentialId>, sqlx::Error> {
        let rows = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT credential_id FROM passkey_credentials WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(CredentialId).collect())
    }

    /// Loads the public material needed for cryptographic assertion verification.
    pub async fn find_passkey_credential(
        &self,
        credential_id: &[u8],
    ) -> Result<Option<PasskeyCredential>, sqlx::Error> {
        let row = sqlx::query_as::<_, PasskeyCredentialRow>(
            "SELECT credential_id, public_key, sign_count, transports FROM passkey_credentials WHERE credential_id = $1 AND revoked_at IS NULL",
        )
        .bind(credential_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| PasskeyCredential {
            id: CredentialId(row.credential_id),
            public_key_cose: CosePublicKey(row.public_key),
            counter: u32::try_from(row.sign_count).unwrap_or(u32::MAX),
            transports: row.transports,
            aaguid: [0; 16],
        }))
    }

    /// Loads a credential only when it belongs to the account named by the ceremony.
    pub async fn find_passkey_credential_for_user(
        &self,
        user_id: Uuid,
        credential_id: &[u8],
    ) -> Result<Option<PasskeyCredential>, sqlx::Error> {
        let row = sqlx::query_as::<_, PasskeyCredentialRow>(
            "SELECT credential_id, public_key, sign_count, transports FROM passkey_credentials WHERE user_id = $1 AND credential_id = $2 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(credential_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| PasskeyCredential {
            id: CredentialId(row.credential_id),
            public_key_cose: CosePublicKey(row.public_key),
            counter: u32::try_from(row.sign_count).unwrap_or(u32::MAX),
            transports: row.transports,
            aaguid: [0; 16],
        }))
    }

    /// Atomically advances a passkey counter after the caller validates the WebAuthn signature.
    pub async fn advance_passkey_sign_count(
        &self,
        credential_bytes: &[u8],
        new_sign_count: i64,
    ) -> Result<bool, sqlx::Error> {
        let updated = sqlx::query(
            "UPDATE passkey_credentials SET sign_count = $2, last_used_at = now() \
             WHERE credential_id = $1 AND revoked_at IS NULL AND $2 >= sign_count \
             AND NOT (sign_count > 0 AND $2 = 0)",
        )
        .bind(credential_bytes)
        .bind(new_sign_count)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Persists a single-use WebAuthn challenge with its expected origin and RP ID.
    pub async fn create_webauthn_challenge(
        &self,
        challenge: NewWebAuthnChallenge<'_>,
    ) -> Result<bool, sqlx::Error> {
        let ceremony = match challenge.ceremony {
            WebAuthnCeremony::Registration => "registration",
            WebAuthnCeremony::Authentication => "authentication",
        };
        let inserted = sqlx::query(
            "INSERT INTO webauthn_challenges (id, challenge_hash, state_blob, ceremony, rp_id, origin, expires_at, user_id, browser_session_id, pairing_request_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7::timestamptz, $8, $9, $10)",
        )
        .bind(challenge.challenge_id)
        .bind(challenge.challenge_hash.as_bytes())
        .bind(challenge.state_blob)
        .bind(ceremony)
        .bind(challenge.rp_id)
        .bind(challenge.origin)
        .bind(challenge.expires_at_rfc3339)
        .bind(challenge.user_id)
        .bind(challenge.browser_session_id)
        .bind(challenge.pairing_request_id)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Creates a WebAuthn challenge with a database-clock expiry and opaque serialized state.
    pub async fn create_webauthn_challenge_for_minutes(
        &self,
        challenge: NewWebAuthnChallenge<'_>,
        lifetime_minutes: i32,
    ) -> Result<bool, sqlx::Error> {
        let ceremony = match challenge.ceremony {
            WebAuthnCeremony::Registration => "registration",
            WebAuthnCeremony::Authentication => "authentication",
        };
        let inserted = sqlx::query(
            "INSERT INTO webauthn_challenges (id, challenge_hash, state_blob, ceremony, rp_id, origin, expires_at, user_id, browser_session_id, pairing_request_id) \
             VALUES ($1, $2, $3, $4, $5, $6, now() + ($7 * interval '1 minute'), $8, $9, $10)",
        )
        .bind(challenge.challenge_id)
        .bind(challenge.challenge_hash.as_bytes())
        .bind(challenge.state_blob)
        .bind(ceremony)
        .bind(challenge.rp_id)
        .bind(challenge.origin)
        .bind(lifetime_minutes)
        .bind(challenge.user_id)
        .bind(challenge.browser_session_id)
        .bind(challenge.pairing_request_id)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Loads an unconsumed WebAuthn state blob while the database enforces its expiry window.
    pub async fn load_webauthn_challenge_state(
        &self,
        challenge_id: Uuid,
    ) -> Result<Option<Vec<u8>>, sqlx::Error> {
        sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT state_blob FROM webauthn_challenges WHERE id = $1 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(challenge_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Consumes a challenge exactly once when all server-bound context matches.
    pub async fn consume_webauthn_challenge(
        &self,
        challenge_id: Uuid,
        challenge_hash: &SecretHash,
        ceremony: WebAuthnCeremony,
        origin: &str,
        rp_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let ceremony = match ceremony {
            WebAuthnCeremony::Registration => "registration",
            WebAuthnCeremony::Authentication => "authentication",
        };
        let updated = sqlx::query(
            "UPDATE webauthn_challenges SET consumed_at = now() \
             WHERE id = $1 AND challenge_hash = $2 AND ceremony = $3 AND origin = $4 AND rp_id = $5 \
             AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(challenge_id)
        .bind(challenge_hash.as_bytes())
        .bind(ceremony)
        .bind(origin)
        .bind(rp_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Creates an unapproved desktop pairing request with three independent hashed proofs.
    pub async fn create_pairing_request(
        &self,
        request: NewPairingRequest<'_>,
    ) -> Result<bool, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO pairing_requests (id, desktop_token_hash, approval_secret_hash, short_code_hash, verification_phrase, device_display_name, device_type, app_version, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::timestamptz)",
        )
        .bind(request.request_id)
        .bind(request.desktop_token_hash.as_bytes())
        .bind(request.approval_secret_hash.as_bytes())
        .bind(request.short_code_hash.as_bytes())
        .bind(request.verification_phrase)
        .bind(request.device_display_name)
        .bind(request.device_type)
        .bind(request.app_version)
        .bind(request.expires_at_rfc3339)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Creates a pairing request with a database-clock expiry, avoiding an application-clock trust boundary.
    pub async fn create_pairing_request_for_minutes(
        &self,
        request: NewPairingRequest<'_>,
        lifetime_minutes: i32,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO pairing_requests (id, desktop_token_hash, approval_secret_hash, short_code_hash, verification_phrase, device_display_name, device_type, app_version, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + ($9 * interval '1 minute')) RETURNING to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
        )
        .bind(request.request_id)
        .bind(request.desktop_token_hash.as_bytes())
        .bind(request.approval_secret_hash.as_bytes())
        .bind(request.short_code_hash.as_bytes())
        .bind(request.verification_phrase)
        .bind(request.device_display_name)
        .bind(request.device_type)
        .bind(request.app_version)
        .bind(lifetime_minutes)
        .fetch_optional(&self.pool)
        .await
    }

    /// Looks up an unexpired request by short-code hash without returning any secret proof.
    pub async fn lookup_pairing_request(
        &self,
        short_code_hash: &SecretHash,
    ) -> Result<Option<PairingPreview>, sqlx::Error> {
        sqlx::query_as::<_, PairingPreviewRow>(
            "SELECT id, device_display_name, device_type, app_version, verification_phrase, to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS expires_at FROM pairing_requests \
             WHERE short_code_hash = $1 AND approved_at IS NULL AND consumed_at IS NULL \
             AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(short_code_hash.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(Into::into))
    }

    /// Approves a pending request only from a live browser session with a fresh passkey assertion.
    pub async fn approve_pairing_request(
        &self,
        request_id: Uuid,
        user_id: Uuid,
        browser_session_id: Uuid,
        approval_secret_hash: &SecretHash,
        verification_phrase: &str,
    ) -> Result<bool, sqlx::Error> {
        let updated = sqlx::query(
            "UPDATE pairing_requests p SET approved_by_user_id = $2, approved_at = now() \
             FROM browser_sessions b JOIN users u ON u.id = b.user_id \
             WHERE p.id = $1 AND p.approval_secret_hash = $4 AND p.verification_phrase = $5 AND p.approved_at IS NULL AND p.consumed_at IS NULL AND p.revoked_at IS NULL AND p.expires_at > now() \
             AND b.id = $3 AND b.user_id = $2 AND b.revoked_at IS NULL AND b.expires_at > now() \
             AND b.passkey_reauthenticated_at > now() - interval '2 minutes' AND u.status = 'active'",
        )
        .bind(request_id)
        .bind(user_id)
        .bind(browser_session_id)
        .bind(approval_secret_hash.as_bytes())
        .bind(verification_phrase)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Approves a request only with the opaque browser cookie and its CSRF proof.
    pub async fn approve_pairing_request_with_browser_proof(
        &self,
        request_id: Uuid,
        approval_secret_hash: &SecretHash,
        verification_phrase: &str,
        session_token_hash: &SecretHash,
        csrf_hash: &SecretHash,
    ) -> Result<bool, sqlx::Error> {
        let updated = sqlx::query(
            "UPDATE pairing_requests p SET approved_by_user_id = b.user_id, approved_at = now() \
             FROM browser_sessions b JOIN users u ON u.id = b.user_id \
             WHERE p.id = $1 AND p.approval_secret_hash = $2 AND p.verification_phrase = $3 \
             AND p.approved_at IS NULL AND p.consumed_at IS NULL AND p.revoked_at IS NULL AND p.expires_at > now() \
             AND b.session_token_hash = $4 AND b.csrf_token_hash = $5 AND b.revoked_at IS NULL AND b.expires_at > now() \
             AND b.passkey_reauthenticated_at > now() - interval '2 minutes' AND u.status = 'active'",
        )
        .bind(request_id)
        .bind(approval_secret_hash.as_bytes())
        .bind(verification_phrase)
        .bind(session_token_hash.as_bytes())
        .bind(csrf_hash.as_bytes())
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Atomically consumes an approved request and creates an owner-scoped device/session pair.
    pub async fn complete_pairing(
        &self,
        request_id: Uuid,
        desktop_token_hash: &SecretHash,
        session: NewPairingSession<'_>,
    ) -> Result<PairingCompletionOutcome, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let request = sqlx::query_as::<_, PairingRow>(
            "SELECT approved_by_user_id, device_display_name, device_type, app_version, \
             approved_at IS NOT NULL AS approved, consumed_at IS NOT NULL AS consumed, \
             revoked_at IS NOT NULL AS revoked, expires_at <= now() AS expired \
             FROM pairing_requests WHERE id = $1 AND desktop_token_hash = $2 FOR UPDATE",
        )
        .bind(request_id)
        .bind(desktop_token_hash.as_bytes())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(request) = request else {
            transaction.rollback().await?;
            return Ok(PairingCompletionOutcome::InvalidProof);
        };
        if request.revoked || request.consumed || request.expired {
            transaction.rollback().await?;
            return Ok(PairingCompletionOutcome::NoLongerAvailable);
        }
        if !request.approved {
            transaction.rollback().await?;
            return Ok(PairingCompletionOutcome::Pending);
        }
        let Some(user_id) = request.approved_by_user_id else {
            transaction.rollback().await?;
            return Ok(PairingCompletionOutcome::NoLongerAvailable);
        };
        // Serialize completions for one account so concurrent requests cannot bypass the device cap.
        let account_display_name = sqlx::query_scalar::<_, String>(
            "SELECT account_display_name FROM users WHERE id = $1 AND status = 'active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(account_display_name) = account_display_name else {
            transaction.rollback().await?;
            return Ok(PairingCompletionOutcome::NoLongerAvailable);
        };
        let inserted = sqlx::query(
            "INSERT INTO devices (id, user_id, device_display_name, device_type, app_version, device_secret_hash) \
             SELECT $1, $2, $3, $4, $5, $6 WHERE (SELECT COUNT(*) FROM devices WHERE user_id = $2 AND revoked_at IS NULL) < $7",
        )
        .bind(session.device_id)
        .bind(user_id)
        .bind(&request.device_display_name)
        .bind(&request.device_type)
        .bind(request.app_version)
        .bind(session.device_secret_hash.as_bytes())
        .bind(i64::from(MAX_ACCOUNT_DEVICES))
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(PairingCompletionOutcome::DeviceLimit);
        }
        let access_expires_at = sqlx::query_scalar::<_, String>(
            "INSERT INTO sessions (id, user_id, device_id, access_token_hash, access_expires_at) VALUES ($1, $2, $3, $4, CASE WHEN $5 = 'db:15m' THEN now() + interval '15 minutes' ELSE $5::timestamptz END) \
             RETURNING to_char(access_expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
        )
        .bind(session.session_id)
        .bind(user_id)
        .bind(session.device_id)
        .bind(session.access_hash.as_bytes())
        .bind(session.access_expires_at_rfc3339)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("UPDATE pairing_requests SET consumed_at = now() WHERE id = $1")
            .bind(request_id)
            .execute(&mut *transaction)
            .await?;
        audit(
            &mut transaction,
            Some(user_id),
            Some(session.device_id),
            "pairing_completed",
        )
        .await?;
        transaction.commit().await?;
        Ok(PairingCompletionOutcome::Completed(PairingCompletion {
            user_id,
            device_id: session.device_id,
            session_id: session.session_id,
            access_expires_at,
            account_display_name,
            device_display_name: request.device_display_name,
            device_type: request.device_type,
        }))
    }

    /// Atomically increments a PostgreSQL-backed rate-limit bucket when capacity remains.
    pub async fn consume_rate_limit(
        &self,
        key_hash: &SecretHash,
        bucket_started_at_rfc3339: &str,
        expires_at_rfc3339: &str,
        limit: i64,
    ) -> Result<bool, sqlx::Error> {
        let row = sqlx::query_scalar::<_, i64>(
            "INSERT INTO rate_limit_buckets (key_hash, bucket_started_at, request_count, expires_at) VALUES ($1, $2::timestamptz, 1, $3::timestamptz) \
             ON CONFLICT (key_hash, bucket_started_at) DO UPDATE SET request_count = rate_limit_buckets.request_count + 1 \
             WHERE rate_limit_buckets.request_count < $4 RETURNING request_count",
        )
        .bind(key_hash.as_bytes())
        .bind(bucket_started_at_rfc3339)
        .bind(expires_at_rfc3339)
        .bind(limit)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Consumes a database-clock fifteen-minute rate-limit bucket for an opaque endpoint key.
    pub async fn consume_rate_limit_for_minutes(
        &self,
        key_hash: &SecretHash,
        lifetime_minutes: i32,
        limit: i64,
    ) -> Result<bool, sqlx::Error> {
        let row = sqlx::query_scalar::<_, i64>(
            "INSERT INTO rate_limit_buckets (key_hash, bucket_started_at, request_count, expires_at) \
             VALUES ($1, date_trunc('minute', now()), 1, now() + ($2 * interval '1 minute')) \
             ON CONFLICT (key_hash, bucket_started_at) DO UPDATE \
             SET request_count = rate_limit_buckets.request_count + 1 \
             WHERE rate_limit_buckets.request_count < $3 RETURNING request_count",
        )
        .bind(key_hash.as_bytes())
        .bind(lifetime_minutes)
        .bind(limit)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Returns a redacted account/dependency inventory for an operator's manual review.
    pub async fn account_cleanup_preview(&self) -> Result<CleanupPreview, sqlx::Error> {
        let accounts = sqlx::query_as::<_, CleanupAccountRow>(
            "SELECT u.id AS account_id, u.status, to_char(u.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at, to_char(u.deleted_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS deleted_at, EXISTS (SELECT 1 FROM account_identities ai WHERE ai.user_id = u.id AND ai.kind = 'admin' AND ai.revoked_at IS NULL) AS protected FROM users u ORDER BY u.created_at, u.id",
        )
        .fetch_all(&self.pool)
        .await?;
        let dependencies = sqlx::query_as::<_, CleanupDependencyRow>(
            r#"
SELECT pc.user_id AS account_id, 'passkey' AS kind, pc.id, CASE WHEN pc.revoked_at IS NULL THEN 'active' ELSE 'revoked' END AS status,
       to_char(pc.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at,
       to_char(pc.last_used_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS last_activity_at,
       NULL::text AS expires_at, NULL::uuid AS related_id, NULL::text AS event_type
FROM passkey_credentials pc
UNION ALL
SELECT ai.user_id, 'account_identity', ai.id, CASE WHEN ai.revoked_at IS NULL THEN 'active' ELSE 'revoked' END,
       to_char(ai.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
       to_char(ai.verified_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
       NULL::text, NULL::uuid, NULL::text
FROM account_identities ai
UNION ALL
SELECT d.user_id, 'device', d.id, CASE WHEN d.revoked_at IS NULL THEN 'active' ELSE 'revoked' END,
       to_char(d.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
       to_char(d.last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
       NULL::text, NULL::uuid, NULL::text
FROM devices d
UNION ALL
SELECT s.user_id, 'session', s.id,
       CASE WHEN s.revoked_at IS NOT NULL THEN 'revoked' WHEN s.access_expires_at <= now() THEN 'expired' ELSE 'active' END,
       to_char(s.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
       to_char(s.last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
       to_char(s.access_expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), s.device_id, NULL::text
FROM sessions s
UNION ALL
SELECT b.user_id, 'browser_session', b.id,
       CASE WHEN b.revoked_at IS NOT NULL THEN 'revoked' WHEN b.expires_at <= now() THEN 'expired' ELSE 'active' END,
       to_char(b.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
       to_char(b.last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
       to_char(b.expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), NULL::uuid, NULL::text
FROM browser_sessions b
UNION ALL
SELECT p.approved_by_user_id, 'pairing_request', p.id,
       CASE WHEN p.revoked_at IS NOT NULL THEN 'revoked' WHEN p.consumed_at IS NOT NULL THEN 'consumed' WHEN p.expires_at <= now() THEN 'expired' WHEN p.approved_at IS NOT NULL THEN 'approved' ELSE 'pending' END,
       to_char(p.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), NULL::text,
       to_char(p.expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), NULL::uuid, NULL::text
FROM pairing_requests p
WHERE p.approved_by_user_id IS NOT NULL
UNION ALL
SELECT w.user_id, 'webauthn_challenge', w.id,
       CASE WHEN w.revoked_at IS NOT NULL THEN 'revoked' WHEN w.consumed_at IS NOT NULL THEN 'consumed' WHEN w.expires_at <= now() THEN 'expired' ELSE 'pending' END,
       to_char(w.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), NULL::text,
       to_char(w.expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), w.pairing_request_id, NULL::text
FROM webauthn_challenges w
WHERE w.user_id IS NOT NULL
UNION ALL
SELECT a.user_id, 'audit_event', a.id, 'recorded',
       to_char(a.occurred_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), NULL::text, NULL::text,
       a.device_id, a.event_type
FROM account_audit_events a
WHERE a.user_id IS NOT NULL
ORDER BY account_id, created_at, id
"#,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut result = accounts
            .into_iter()
            .map(|row| {
                let (candidate_status, candidate_reason) = if row.protected {
                    (
                        "protected".to_owned(),
                        "active admin identity detected; operator deactivation is refused"
                            .to_owned(),
                    )
                } else if row.status == "active" {
                    (
                        "review_required".to_owned(),
                        "test-only status is not inferred; verify staging ownership and this exact account ID"
                            .to_owned(),
                    )
                } else {
                    (
                        "not_actionable".to_owned(),
                        "account is already deactivated; records are retained for audit and recovery review"
                            .to_owned(),
                    )
                };
                CleanupAccount {
                    account_id: row.account_id,
                    status: row.status,
                    created_at: row.created_at,
                    deleted_at: row.deleted_at,
                    candidate_status,
                    candidate_reason,
                    protected: row.protected,
                    dependency_counts: CleanupCounts::default(),
                    dependencies: Vec::new(),
                }
            })
            .collect::<Vec<_>>();
        for row in dependencies {
            let Some(account) = result
                .iter_mut()
                .find(|account| account.account_id == row.account_id)
            else {
                continue;
            };
            let kind = row.kind;
            let status = row.status;
            let candidate_reason = cleanup_dependency_reason(&kind, &status);
            let event_type = row
                .event_type
                .filter(|event_type| is_safe_audit_event(event_type));
            let dependency = CleanupDependency {
                id: row.id,
                kind,
                status,
                candidate_reason,
                created_at: row.created_at,
                last_activity_at: row.last_activity_at,
                expires_at: row.expires_at,
                related_id: row.related_id,
                event_type,
            };
            account.dependency_counts.count_dependency(&dependency.kind);
            account.dependencies.push(dependency);
        }
        Ok(CleanupPreview { accounts: result })
    }

    /// Deactivates one exact account after the caller has independently verified it is test-only.
    pub async fn deactivate_account_for_operator(
        &self,
        account_id: Uuid,
        confirmation: &str,
    ) -> Result<CleanupActionResult, CleanupError> {
        validate_confirmation(
            CleanupAction::DeactivateAccount,
            account_id,
            Some(confirmation),
        )
        .map_err(|_| CleanupError::InvalidConfirmation)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| CleanupError::Database)?;
        let Some(account) = sqlx::query_as::<_, OperatorAccountRow>(
            "SELECT u.status, EXISTS (SELECT 1 FROM account_identities ai WHERE ai.user_id = u.id AND ai.kind = 'admin' AND ai.revoked_at IS NULL) AS protected FROM users u WHERE u.id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| CleanupError::Database)?
        else {
            return Err(CleanupError::NotFound);
        };
        if account.status != "active" {
            return Err(CleanupError::NotActive);
        }
        if account.protected {
            return Err(CleanupError::ProtectedAccount);
        }
        sqlx::query("UPDATE users SET status = 'deleted', deleted_at = now(), updated_at = now() WHERE id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| CleanupError::Database)?;
        let revoked = revoke_account_dependencies(&mut transaction, account_id)
            .await
            .map_err(|_| CleanupError::Database)?;
        let audit_event_id = audit(
            &mut transaction,
            Some(account_id),
            None,
            "operator_account_deactivated",
        )
        .await
        .map_err(|_| CleanupError::Database)?;
        transaction
            .commit()
            .await
            .map_err(|_| CleanupError::Database)?;
        Ok(CleanupActionResult {
            action: "account_deactivated".to_owned(),
            target_id: account_id,
            account_id,
            status: "deactivated".to_owned(),
            revoked,
            audit_event_id,
        })
    }

    /// Revokes one exact device and all of its native sessions and refresh tokens.
    pub async fn revoke_device_for_operator(
        &self,
        device_id: Uuid,
        confirmation: &str,
    ) -> Result<CleanupActionResult, CleanupError> {
        validate_confirmation(CleanupAction::RevokeDevice, device_id, Some(confirmation))
            .map_err(|_| CleanupError::InvalidConfirmation)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| CleanupError::Database)?;
        let Some(account_id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM devices WHERE id = $1 AND revoked_at IS NULL FOR UPDATE",
        )
        .bind(device_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| CleanupError::Database)?
        else {
            return Err(CleanupError::NotFound);
        };
        let device_count = sqlx::query(
            "UPDATE devices SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(device_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CleanupError::Database)?;
        let sessions = sqlx::query(
            "UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND device_id = $2 AND revoked_at IS NULL",
        )
        .bind(account_id)
        .bind(device_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CleanupError::Database)?;
        let audit_event_id = audit(
            &mut transaction,
            Some(account_id),
            Some(device_id),
            "operator_device_revoked",
        )
        .await
        .map_err(|_| CleanupError::Database)?;
        transaction
            .commit()
            .await
            .map_err(|_| CleanupError::Database)?;
        Ok(CleanupActionResult {
            action: "device_revoked".to_owned(),
            target_id: device_id,
            account_id,
            status: "revoked".to_owned(),
            revoked: CleanupCounts {
                devices: affected_count(device_count),
                sessions: affected_count(sessions),
                ..CleanupCounts::default()
            },
            audit_event_id,
        })
    }

    /// Revokes one exact passkey row but refuses to remove the last working passkey of a live account.
    pub async fn revoke_credential_for_operator(
        &self,
        credential_id: Uuid,
        confirmation: &str,
    ) -> Result<CleanupActionResult, CleanupError> {
        validate_confirmation(
            CleanupAction::RevokeCredential,
            credential_id,
            Some(confirmation),
        )
        .map_err(|_| CleanupError::InvalidConfirmation)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| CleanupError::Database)?;
        let Some(account_id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM passkey_credentials WHERE id = $1 AND revoked_at IS NULL FOR UPDATE",
        )
        .bind(credential_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| CleanupError::Database)?
        else {
            return Err(CleanupError::NotFound);
        };
        let status =
            sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id = $1 FOR UPDATE")
                .bind(account_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| CleanupError::Database)?;
        let active_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM passkey_credentials WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| CleanupError::Database)?;
        if status == "active" && active_count <= 1 {
            return Err(CleanupError::LastWorkingPasskey);
        }
        let revoked = sqlx::query(
            "UPDATE passkey_credentials SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(credential_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CleanupError::Database)?;
        let audit_event_id = audit(
            &mut transaction,
            Some(account_id),
            None,
            "operator_passkey_revoked",
        )
        .await
        .map_err(|_| CleanupError::Database)?;
        transaction
            .commit()
            .await
            .map_err(|_| CleanupError::Database)?;
        Ok(CleanupActionResult {
            action: "credential_revoked".to_owned(),
            target_id: credential_id,
            account_id,
            status: "revoked".to_owned(),
            revoked: CleanupCounts {
                passkeys: affected_count(revoked),
                ..CleanupCounts::default()
            },
            audit_event_id,
        })
    }

    /// Revokes all account-owned devices, sessions, refresh tokens, and passkeys before tombstoning the user.
    pub async fn delete_account(&self, user_id: Uuid) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE users SET status = 'deleted', deleted_at = now(), updated_at = now() WHERE id = $1 AND status = 'active'")
            .bind(user_id).execute(&mut *transaction).await?;
        if updated.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        revoke_account_dependencies(&mut transaction, user_id).await?;
        audit(
            &mut transaction,
            Some(user_id),
            None,
            "account_deletion_completed",
        )
        .await?;
        transaction.commit().await?;
        Ok(true)
    }
}

#[async_trait::async_trait]
impl NativeSessionResolver for PostgresAccountStore {
    async fn resolve_active_native_session(
        &self,
        access_hash: &SecretHash,
    ) -> Result<Option<ActiveSession>, NativeSessionLookupError> {
        self.find_active_session_by_access_hash(access_hash)
            .await
            .map_err(|_| NativeSessionLookupError)
    }
}

#[derive(sqlx::FromRow)]
struct CleanupAccountRow {
    account_id: Uuid,
    status: String,
    created_at: String,
    deleted_at: Option<String>,
    protected: bool,
}

#[derive(sqlx::FromRow)]
struct CleanupDependencyRow {
    account_id: Uuid,
    kind: String,
    id: Uuid,
    status: String,
    created_at: String,
    last_activity_at: Option<String>,
    expires_at: Option<String>,
    related_id: Option<Uuid>,
    event_type: Option<String>,
}

#[derive(sqlx::FromRow)]
struct OperatorAccountRow {
    status: String,
    protected: bool,
}

/// Revokes all live account-owned access while retaining rows for audit and recovery review.
async fn revoke_account_dependencies(
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

fn affected_count(result: sqlx::postgres::PgQueryResult) -> usize {
    result.rows_affected().try_into().unwrap_or(usize::MAX)
}

fn cleanup_dependency_reason(kind: &str, status: &str) -> String {
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
struct DeviceRow {
    id: Uuid,
    user_id: Uuid,
    device_display_name: String,
    device_type: String,
    created_at: String,
    last_seen_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct BrowserDeviceRow {
    id: Uuid,
    device_display_name: String,
    device_type: String,
    created_at: String,
    last_seen_at: Option<String>,
    session_status: String,
}

#[derive(sqlx::FromRow)]
struct AdminDeviceReadModelRow {
    device_type: String,
    device_display_name: String,
    status: String,
    created_at: String,
    last_seen_at: Option<String>,
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
struct AccountProjectionRow {
    account_display_name: String,
    created_at: String,
    device_display_name: String,
    device_type: String,
    device_created_at: String,
    last_seen_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ActiveSessionRow {
    session_id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
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
struct PasskeyCredentialRow {
    credential_id: Vec<u8>,
    public_key: Vec<u8>,
    sign_count: i64,
    transports: Vec<String>,
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
struct PairingRow {
    approved_by_user_id: Option<Uuid>,
    device_display_name: String,
    device_type: String,
    app_version: Option<String>,
    approved: bool,
    consumed: bool,
    revoked: bool,
    expired: bool,
}

#[derive(sqlx::FromRow)]
struct PairingPreviewRow {
    id: Uuid,
    device_display_name: String,
    device_type: String,
    app_version: Option<String>,
    verification_phrase: String,
    expires_at: String,
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

async fn audit(
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

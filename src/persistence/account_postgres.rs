//! PostgreSQL persistence for the approved passkey-only account ownership model.

use passkey_auth::{CosePublicKey, CredentialId, PasskeyCredential};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::auth::{
    ActiveSession, NewBrowserSession, NewPairingRequest, NewPairingSession, NewSession,
    NewWebAuthnChallenge, OwnedDevice, PairingCompletion, PairingPreview, RefreshError,
    RefreshRotation, SecretHash, WebAuthnCeremony, is_safe_audit_event,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// PostgreSQL store that persists only token hashes and safe audit classifications.
#[derive(Clone, Debug)]
pub struct PostgresAccountStore {
    pool: PgPool,
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

    /// Creates a device only when its owner is still active.
    pub async fn create_device(&self, device: &OwnedDevice) -> Result<bool, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO devices (id, user_id, name, platform) \
             SELECT $1, id, $3, $4 FROM users WHERE id = $2 AND status = 'active'",
        )
        .bind(device.id)
        .bind(device.user_id)
        .bind(&device.name)
        .bind(&device.platform)
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
            "SELECT d.id, d.user_id, d.name, d.platform FROM devices d \
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
             AND u.status = 'active' AND d.revoked_at IS NULL",
        )
        .bind(access_hash.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(Into::into))
    }

    /// Lists only active devices owned by the requested account.
    pub async fn list_owned_devices(&self, user_id: Uuid) -> Result<Vec<OwnedDevice>, sqlx::Error> {
        sqlx::query_as::<_, DeviceRow>(
            "SELECT d.id, d.user_id, d.name, d.platform FROM devices d \
             JOIN users u ON u.id = d.user_id WHERE d.user_id = $1 AND d.revoked_at IS NULL \
             AND u.status = 'active' ORDER BY d.created_at, d.id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().map(Into::into).collect())
    }

    /// Revokes one device and all native sessions/refresh tokens owned by it.
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
        sqlx::query("UPDATE refresh_tokens SET revoked_at = now() WHERE session_id IN (SELECT id FROM sessions WHERE user_id = $1 AND device_id = $2) AND revoked_at IS NULL")
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

    /// Revokes the caller's native session and its refresh-token family.
    pub async fn revoke_session(
        &self,
        session_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
            .bind(session_id).bind(user_id).execute(&mut *transaction).await?;
        if updated.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        sqlx::query("UPDATE refresh_tokens SET revoked_at = now() WHERE session_id = $1 AND revoked_at IS NULL")
            .bind(session_id).execute(&mut *transaction).await?;
        audit(&mut transaction, Some(user_id), None, "logout").await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Persists a desktop/native session and its first hashed refresh token in one transaction.
    pub async fn create_session(&self, session: NewSession<'_>) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let family_id = Uuid::new_v4();
        let inserted = sqlx::query(
            "INSERT INTO sessions (id, user_id, device_id, access_token_hash, access_expires_at, refresh_family_id) \
             SELECT $1, $2, d.id, $4, $5::timestamptz, $6 FROM devices d JOIN users u ON u.id = d.user_id \
             WHERE d.id = $3 AND d.user_id = $2 AND d.revoked_at IS NULL AND u.status = 'active'",
        )
        .bind(session.session_id).bind(session.user_id).bind(session.device_id).bind(session.access_hash.as_bytes())
        .bind(session.access_expires_at_rfc3339).bind(family_id).execute(&mut *transaction).await?;
        if inserted.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        sqlx::query("INSERT INTO refresh_tokens (id, session_id, family_id, token_hash, expires_at) VALUES ($1, $2, $3, $4, $5::timestamptz)")
            .bind(session.refresh_id).bind(session.session_id).bind(family_id).bind(session.refresh_hash.as_bytes())
            .bind(session.refresh_expires_at_rfc3339).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(true)
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
            "INSERT INTO pairing_requests (id, desktop_token_hash, approval_secret_hash, short_code_hash, verification_phrase, device_name, platform, app_version, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::timestamptz)",
        )
        .bind(request.request_id)
        .bind(request.desktop_token_hash.as_bytes())
        .bind(request.approval_secret_hash.as_bytes())
        .bind(request.short_code_hash.as_bytes())
        .bind(request.verification_phrase)
        .bind(request.device_name)
        .bind(request.platform)
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
    ) -> Result<bool, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO pairing_requests (id, desktop_token_hash, approval_secret_hash, short_code_hash, verification_phrase, device_name, platform, app_version, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + ($9 * interval '1 minute'))",
        )
        .bind(request.request_id)
        .bind(request.desktop_token_hash.as_bytes())
        .bind(request.approval_secret_hash.as_bytes())
        .bind(request.short_code_hash.as_bytes())
        .bind(request.verification_phrase)
        .bind(request.device_name)
        .bind(request.platform)
        .bind(request.app_version)
        .bind(lifetime_minutes)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Looks up an unexpired request by short-code hash without returning any secret proof.
    pub async fn lookup_pairing_request(
        &self,
        short_code_hash: &SecretHash,
    ) -> Result<Option<PairingPreview>, sqlx::Error> {
        sqlx::query_as::<_, PairingPreviewRow>(
            "SELECT id, device_name, platform, app_version, verification_phrase FROM pairing_requests \
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
    ) -> Result<Option<PairingCompletion>, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let request = sqlx::query_as::<_, PairingRow>(
            "SELECT approved_by_user_id, device_name, platform, app_version FROM pairing_requests \
             WHERE id = $1 AND desktop_token_hash = $2 AND approved_at IS NOT NULL AND consumed_at IS NULL \
             AND revoked_at IS NULL AND expires_at > now() FOR UPDATE",
        )
        .bind(request_id)
        .bind(desktop_token_hash.as_bytes())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(request) = request else {
            transaction.rollback().await?;
            return Ok(None);
        };
        let Some(user_id) = request.approved_by_user_id else {
            transaction.rollback().await?;
            return Ok(None);
        };
        // Serialize completions for one account so concurrent requests cannot bypass the device cap.
        sqlx::query("SELECT id FROM users WHERE id = $1 AND status = 'active' FOR UPDATE")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        let inserted = sqlx::query(
            "INSERT INTO devices (id, user_id, name, platform, app_version) \
             SELECT $1, $2, $3, $4, $5 WHERE (SELECT COUNT(*) FROM devices WHERE user_id = $2 AND revoked_at IS NULL) < 10",
        )
        .bind(session.device_id)
        .bind(user_id)
        .bind(request.device_name)
        .bind(request.platform)
        .bind(request.app_version)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(None);
        }
        let family_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, device_id, access_token_hash, access_expires_at, refresh_family_id) VALUES ($1, $2, $3, $4, CASE WHEN $5 = 'db:15m' THEN now() + interval '15 minutes' ELSE $5::timestamptz END, $6)",
        )
        .bind(session.session_id)
        .bind(user_id)
        .bind(session.device_id)
        .bind(session.access_hash.as_bytes())
        .bind(session.access_expires_at_rfc3339)
        .bind(family_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO refresh_tokens (id, session_id, family_id, token_hash, expires_at) VALUES ($1, $2, $3, $4, CASE WHEN $5 = 'db:30d' THEN now() + interval '30 days' ELSE $5::timestamptz END)")
            .bind(session.refresh_id).bind(session.session_id).bind(family_id).bind(session.refresh_hash.as_bytes())
            .bind(session.refresh_expires_at_rfc3339).execute(&mut *transaction).await?;
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
        Ok(Some(PairingCompletion {
            user_id,
            device_id: session.device_id,
            session_id: session.session_id,
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

    /// Atomically consumes a refresh token; a replay revokes the entire family before returning a neutral error.
    pub async fn rotate_refresh(
        &self,
        presented_hash: &SecretHash,
        replacement_id: Uuid,
        replacement_hash: &SecretHash,
        replacement_expires_at_rfc3339: &str,
    ) -> Result<RefreshRotation, RefreshError> {
        self.rotate_refresh_internal(
            presented_hash,
            replacement_id,
            replacement_hash,
            replacement_expires_at_rfc3339,
            None,
        )
        .await
    }

    /// Atomically rotates refresh and replaces the session's short-lived access token.
    pub async fn rotate_refresh_with_access(
        &self,
        presented_hash: &SecretHash,
        replacement_id: Uuid,
        replacement_hash: &SecretHash,
        replacement_expires_at_rfc3339: &str,
        access_hash: &SecretHash,
        access_expires_at_rfc3339: &str,
    ) -> Result<RefreshRotation, RefreshError> {
        self.rotate_refresh_internal(
            presented_hash,
            replacement_id,
            replacement_hash,
            replacement_expires_at_rfc3339,
            Some((access_hash, access_expires_at_rfc3339)),
        )
        .await
    }

    async fn rotate_refresh_internal(
        &self,
        presented_hash: &SecretHash,
        replacement_id: Uuid,
        replacement_hash: &SecretHash,
        replacement_expires_at_rfc3339: &str,
        replacement_access: Option<(&SecretHash, &str)>,
    ) -> Result<RefreshRotation, RefreshError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RefreshError::Rejected)?;
        let token = sqlx::query_as::<_, RefreshRow>(
            "SELECT r.id, r.session_id, r.family_id, r.used_at IS NOT NULL OR r.revoked_at IS NOT NULL \
             OR r.expires_at <= now() OR s.revoked_at IS NOT NULL AS rejected \
             FROM refresh_tokens r JOIN sessions s ON s.id = r.session_id WHERE r.token_hash = $1 FOR UPDATE",
        ).bind(presented_hash.as_bytes()).fetch_optional(&mut *transaction).await.map_err(|_| RefreshError::Rejected)?;
        let Some(token) = token else {
            return Err(RefreshError::Rejected);
        };
        if token.rejected {
            revoke_family(&mut transaction, token.family_id, token.session_id).await?;
            transaction
                .commit()
                .await
                .map_err(|_| RefreshError::Rejected)?;
            return Err(RefreshError::Rejected);
        }
        let inserted = sqlx::query("INSERT INTO refresh_tokens (id, session_id, family_id, token_hash, expires_at) VALUES ($1, $2, $3, $4, $5::timestamptz)")
            .bind(replacement_id).bind(token.session_id).bind(token.family_id).bind(replacement_hash.as_bytes())
            .bind(replacement_expires_at_rfc3339).execute(&mut *transaction).await.map_err(|_| RefreshError::Rejected)?;
        if inserted.rows_affected() != 1 {
            return Err(RefreshError::Rejected);
        }
        if let Some((access_hash, access_expires_at_rfc3339)) = replacement_access {
            let updated = sqlx::query(
                "UPDATE sessions SET access_token_hash = $2, access_expires_at = CASE WHEN $3 = 'db:15m' THEN now() + interval '15 minutes' ELSE $3::timestamptz END, last_seen_at = now() WHERE id = $1 AND revoked_at IS NULL",
            )
            .bind(token.session_id)
            .bind(access_hash.as_bytes())
            .bind(access_expires_at_rfc3339)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RefreshError::Rejected)?;
            if updated.rows_affected() != 1 {
                return Err(RefreshError::Rejected);
            }
        }
        sqlx::query("UPDATE refresh_tokens SET used_at = now(), replaced_by_id = $2 WHERE id = $1")
            .bind(token.id)
            .bind(replacement_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RefreshError::Rejected)?;
        audit(&mut transaction, None, None, "refresh_rotated")
            .await
            .map_err(|_| RefreshError::Rejected)?;
        transaction
            .commit()
            .await
            .map_err(|_| RefreshError::Rejected)?;
        Ok(RefreshRotation {
            replacement_id,
            session_id: token.session_id,
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
        sqlx::query(
            "UPDATE devices SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE refresh_tokens SET revoked_at = now() WHERE session_id IN (SELECT id FROM sessions WHERE user_id = $1) AND revoked_at IS NULL").bind(user_id).execute(&mut *transaction).await?;
        sqlx::query("UPDATE passkey_credentials SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL").bind(user_id).execute(&mut *transaction).await?;
        sqlx::query("UPDATE browser_sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE pairing_requests SET revoked_at = now() WHERE approved_by_user_id = $1 AND revoked_at IS NULL")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE webauthn_challenges SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
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

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: Uuid,
    user_id: Uuid,
    name: String,
    platform: String,
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
            name: row.name,
            platform: row.platform,
        }
    }
}
#[derive(sqlx::FromRow)]
struct RefreshRow {
    id: Uuid,
    session_id: Uuid,
    family_id: Uuid,
    rejected: bool,
}

#[derive(sqlx::FromRow)]
struct PairingRow {
    approved_by_user_id: Option<Uuid>,
    device_name: String,
    platform: String,
    app_version: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PairingPreviewRow {
    id: Uuid,
    device_name: String,
    platform: String,
    app_version: Option<String>,
    verification_phrase: String,
}

impl From<PairingPreviewRow> for PairingPreview {
    fn from(row: PairingPreviewRow) -> Self {
        Self {
            request_id: row.id,
            device_name: row.device_name,
            platform: row.platform,
            app_version: row.app_version,
            verification_phrase: row.verification_phrase,
        }
    }
}

async fn revoke_family(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    family_id: Uuid,
    session_id: Uuid,
) -> Result<(), RefreshError> {
    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = now() WHERE family_id = $1 AND revoked_at IS NULL",
    )
    .bind(family_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| RefreshError::Rejected)?;
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(session_id)
        .execute(&mut **transaction)
        .await
        .map_err(|_| RefreshError::Rejected)?;
    audit(transaction, None, None, "refresh_reuse_detected")
        .await
        .map_err(|_| RefreshError::Rejected)
}

async fn audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Option<Uuid>,
    device_id: Option<Uuid>,
    event_type: &str,
) -> Result<(), sqlx::Error> {
    debug_assert!(is_safe_audit_event(event_type));
    sqlx::query("INSERT INTO account_audit_events (id, user_id, device_id, event_type) VALUES ($1, $2, $3, $4)")
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(device_id)
        .bind(event_type)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

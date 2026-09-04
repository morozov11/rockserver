//! Private PostgreSQL account-store operations grouped by one responsibility.

use super::rows::*;
use super::*;

impl PostgresAccountStore {
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

//! PostgreSQL implementation of the administrator-only persistence boundary.

use async_trait::async_trait;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::{
    admin::{
        AdminAuditEntry, AdminAuditFilter, AdminBootstrapOutcome, AdminLoginAttempt,
        AdminLoginCredential, AdminLoginOutcome, AdminPasswordCredential, AdminPrincipal,
        AdminRequestRecord, AdminSecurityEvent, AdminSession, AdminStore, AdminStoreError,
        AdminUsername, NewAdminBootstrap, NewAdminPasswordCredential, NewAdminSession,
    },
    auth::SecretHash,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// PostgreSQL store for administrator authentication state only.
#[derive(Clone, Debug)]
pub struct PostgresAdminStore {
    pool: PgPool,
}

impl PostgresAdminStore {
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

    /// Reuses a caller-managed migrated pool, primarily for integration tests and application wiring.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AdminStore for PostgresAdminStore {
    async fn bootstrap_admin(
        &self,
        bootstrap: NewAdminBootstrap,
    ) -> Result<AdminBootstrapOutcome, AdminStoreError> {
        // The singleton index makes concurrent missing-admin attempts resolve to one safe winner.
        let created = sqlx::query_scalar::<_, Uuid>(
            "WITH new_principal AS (INSERT INTO admin_principals (id, username, status) SELECT $1, $2, 'active' WHERE NOT EXISTS (SELECT 1 FROM admin_principals) ON CONFLICT DO NOTHING RETURNING id), new_credential AS (INSERT INTO admin_password_credentials (id, principal_id, password_hash) SELECT $3, id, $4 FROM new_principal RETURNING principal_id) INSERT INTO admin_security_events (id, principal_id, event_type) SELECT $5, principal_id, 'admin_created' FROM new_credential RETURNING principal_id",
        )
        .bind(bootstrap.principal_id)
        .bind(bootstrap.username.as_str())
        .bind(bootstrap.credential_id)
        .bind(bootstrap.password_hash.as_str())
        .bind(bootstrap.security_event_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_error)?;
        Ok(if created.is_some() {
            AdminBootstrapOutcome::Created
        } else {
            AdminBootstrapOutcome::AlreadyExists
        })
    }

    async fn create_principal(&self, principal: AdminPrincipal) -> Result<(), AdminStoreError> {
        sqlx::query("INSERT INTO admin_principals (id, status) VALUES ($1, $2)")
            .bind(principal.id)
            .bind(match principal.status {
                crate::admin::AdminPrincipalStatus::Active => "active",
                crate::admin::AdminPrincipalStatus::Disabled => "disabled",
            })
            .execute(&self.pool)
            .await
            .map_err(map_error)?;
        Ok(())
    }

    async fn create_password_credential(
        &self,
        credential: NewAdminPasswordCredential,
    ) -> Result<(), AdminStoreError> {
        let result = sqlx::query("INSERT INTO admin_password_credentials (id, principal_id, password_hash) SELECT $1, id, $3 FROM admin_principals WHERE id = $2 AND status = 'active'")
            .bind(credential.id).bind(credential.principal_id).bind(credential.password_hash.as_str())
            .execute(&self.pool).await.map_err(map_error)?;
        if result.rows_affected() == 0 {
            return Err(AdminStoreError::NotFound);
        }
        Ok(())
    }

    async fn active_password_credential(
        &self,
        principal_id: Uuid,
    ) -> Result<Option<AdminPasswordCredential>, AdminStoreError> {
        let row = sqlx::query_as::<_, AdminPasswordCredentialRow>("SELECT c.id, c.principal_id, c.password_hash FROM admin_password_credentials c JOIN admin_principals p ON p.id = c.principal_id WHERE c.principal_id = $1 AND c.revoked_at IS NULL AND p.status = 'active'")
            .bind(principal_id).fetch_optional(&self.pool).await.map_err(map_error)?;
        row.map(|row| {
            Ok(AdminPasswordCredential {
                id: row.id,
                principal_id: row.principal_id,
                password_hash: crate::admin::AdminPasswordHash::parse(row.password_hash)
                    .map_err(|_| AdminStoreError::Unavailable)?,
            })
        })
        .transpose()
    }

    async fn login_credential(
        &self,
        username: &AdminUsername,
    ) -> Result<Option<AdminLoginCredential>, AdminStoreError> {
        sqlx::query_as::<_, AdminPasswordCredentialRow>("SELECT c.id, c.principal_id, c.password_hash FROM admin_password_credentials c JOIN admin_principals p ON p.id = c.principal_id WHERE p.username = $1 AND c.revoked_at IS NULL AND p.status = 'active'")
            .bind(username.as_str()).fetch_optional(&self.pool).await.map_err(map_error)?
            .map(|row| Ok(AdminLoginCredential { credential: AdminPasswordCredential { id: row.id, principal_id: row.principal_id, password_hash: crate::admin::AdminPasswordHash::parse(row.password_hash).map_err(|_| AdminStoreError::Unavailable)? } })).transpose()
    }

    async fn create_session(&self, session: NewAdminSession) -> Result<(), AdminStoreError> {
        let result = sqlx::query("INSERT INTO admin_sessions (id, principal_id, token_hash, expires_at) SELECT $1, id, $3, now() + ($4 * interval '1 second') FROM admin_principals WHERE id = $2 AND status = 'active'")
            .bind(session.id).bind(session.principal_id).bind(session.token_hash.as_bytes()).bind(session.ttl_seconds)
            .execute(&self.pool).await.map_err(map_error)?;
        if result.rows_affected() == 0 {
            return Err(AdminStoreError::NotFound);
        }
        Ok(())
    }

    async fn find_active_session(
        &self,
        token_hash: &SecretHash,
    ) -> Result<Option<AdminSession>, AdminStoreError> {
        sqlx::query_as::<_, AdminSessionRow>("SELECT s.id, s.principal_id FROM admin_sessions s JOIN admin_principals p ON p.id = s.principal_id WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now() AND p.status = 'active'")
            .bind(token_hash.as_bytes()).fetch_optional(&self.pool).await.map_err(map_error).map(|row| row.map(Into::into))
    }

    async fn recent_failed_login_count(
        &self,
        account_key_hash: &SecretHash,
        source_ip_hash: &SecretHash,
    ) -> Result<u64, AdminStoreError> {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_login_attempts WHERE account_key_hash = $1 AND source_ip_hash = $2 AND outcome = 'failed' AND occurred_at > now() - interval '15 minutes'")
            .bind(account_key_hash.as_bytes()).bind(source_ip_hash.as_bytes()).fetch_one(&self.pool).await.map_err(map_error)?;
        Ok(count as u64)
    }

    async fn revoke_session(
        &self,
        session_id: Uuid,
        replacement_session_id: Option<Uuid>,
    ) -> Result<bool, AdminStoreError> {
        Ok(sqlx::query("UPDATE admin_sessions SET revoked_at = now(), replaced_by_id = $2 WHERE id = $1 AND revoked_at IS NULL")
            .bind(session_id).bind(replacement_session_id).execute(&self.pool).await.map_err(map_error)?.rows_affected() == 1)
    }

    async fn rotate_session(
        &self,
        session_id: Uuid,
        replacement: NewAdminSession,
    ) -> Result<bool, AdminStoreError> {
        // A single transaction prevents a refresh from ever leaving both bearer sessions usable.
        let mut transaction = self.pool.begin().await.map_err(map_error)?;
        let inserted = sqlx::query(
            "INSERT INTO admin_sessions (id, principal_id, token_hash, expires_at) SELECT $1, s.principal_id, $2, now() + ($3 * interval '1 second') FROM admin_sessions s JOIN admin_principals p ON p.id = s.principal_id WHERE s.id = $4 AND s.revoked_at IS NULL AND s.expires_at > now() AND p.status = 'active'",
        )
        .bind(replacement.id)
        .bind(replacement.token_hash.as_bytes())
        .bind(replacement.ttl_seconds)
        .bind(session_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        if inserted.rows_affected() != 1 {
            transaction.rollback().await.map_err(map_error)?;
            return Ok(false);
        }
        let revoked = sqlx::query(
            "UPDATE admin_sessions SET revoked_at = now(), replaced_by_id = $2 WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(replacement.id)
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        if revoked.rows_affected() != 1 {
            transaction.rollback().await.map_err(map_error)?;
            return Ok(false);
        }
        transaction.commit().await.map_err(map_error)?;
        Ok(true)
    }

    async fn record_login_attempt(
        &self,
        attempt: AdminLoginAttempt,
    ) -> Result<(), AdminStoreError> {
        sqlx::query("INSERT INTO admin_login_attempts (id, principal_id, account_key_hash, source_ip_hash, outcome) VALUES ($1, $2, $3, $4, $5)")
            .bind(attempt.id).bind(attempt.principal_id).bind(attempt.account_key_hash.as_bytes()).bind(attempt.source_ip_hash.as_bytes()).bind(match attempt.outcome { AdminLoginOutcome::Succeeded => "succeeded", AdminLoginOutcome::Failed => "failed", AdminLoginOutcome::Locked => "locked" })
            .execute(&self.pool).await.map_err(map_error)?;
        Ok(())
    }

    async fn record_security_event(
        &self,
        event: AdminSecurityEvent,
    ) -> Result<(), AdminStoreError> {
        sqlx::query("INSERT INTO admin_security_events (id, principal_id, session_id, source_ip_hash, event_type) VALUES ($1, $2, $3, $4, $5)")
            .bind(event.id).bind(event.principal_id).bind(event.session_id).bind(event.source_ip_hash.as_ref().map(SecretHash::as_bytes)).bind(event.event_type.as_str())
            .execute(&self.pool).await.map_err(map_error)?;
        Ok(())
    }

    async fn record_request(&self, request: AdminRequestRecord) -> Result<(), AdminStoreError> {
        // Retention is enforced on the write path so request metadata remains bounded without a worker.
        let mut transaction = self.pool.begin().await.map_err(map_error)?;
        sqlx::query("INSERT INTO admin_request_records (id, request_id, principal_id, session_id, endpoint, outcome, duration_ms) VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(request.id)
            .bind(request.request_id)
            .bind(request.principal_id)
            .bind(request.session_id)
            .bind(request.endpoint)
            .bind(request.outcome.as_str())
            .bind(i32::try_from(request.duration_ms).expect("u32 duration must fit PostgreSQL integer"))
            .execute(&mut *transaction)
            .await
            .map_err(map_error)?;
        sqlx::query(
            "DELETE FROM admin_request_records WHERE occurred_at < now() - interval '30 days'",
        )
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        transaction.commit().await.map_err(map_error)?;
        Ok(())
    }

    async fn list_audit(
        &self,
        filter: AdminAuditFilter,
    ) -> Result<Vec<AdminAuditEntry>, AdminStoreError> {
        let rows = sqlx::query_as::<_, AdminAuditEntryRow>(
            "SELECT to_char(occurred_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS occurred_at, action, outcome FROM ( \
             SELECT occurred_at, endpoint AS action, outcome FROM admin_request_records \
             UNION ALL \
             SELECT occurred_at, event_type, 'recorded' FROM admin_security_events \
             ) audit WHERE ($1::timestamptz IS NULL OR occurred_at >= $1::timestamptz) AND ($2::timestamptz IS NULL OR occurred_at < $2::timestamptz) \
             AND ($3::text IS NULL OR action = $3) AND ($4::text IS NULL OR outcome = $4) \
             ORDER BY occurred_at DESC, action ASC LIMIT $5 OFFSET $6",
        )
        .bind(filter.from)
        .bind(filter.until)
        .bind(filter.action)
        .bind(filter.outcome)
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(&self.pool)
        .await
        .map_err(map_error)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}

#[derive(sqlx::FromRow)]
struct AdminPasswordCredentialRow {
    id: Uuid,
    principal_id: Uuid,
    password_hash: String,
}

#[derive(sqlx::FromRow)]
struct AdminSessionRow {
    id: Uuid,
    principal_id: Uuid,
}

#[derive(sqlx::FromRow)]
struct AdminAuditEntryRow {
    occurred_at: String,
    action: String,
    outcome: String,
}

impl From<AdminAuditEntryRow> for AdminAuditEntry {
    fn from(row: AdminAuditEntryRow) -> Self {
        Self {
            occurred_at: row.occurred_at,
            action: row.action,
            outcome: row.outcome,
        }
    }
}

impl From<AdminSessionRow> for AdminSession {
    fn from(row: AdminSessionRow) -> Self {
        Self {
            id: row.id,
            principal_id: row.principal_id,
        }
    }
}

fn map_error(error: sqlx::Error) -> AdminStoreError {
    if matches!(&error, sqlx::Error::Database(database) if database.is_unique_violation()) {
        AdminStoreError::Conflict
    } else {
        AdminStoreError::Unavailable
    }
}

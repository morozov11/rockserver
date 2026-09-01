//! Administrator-only identity and persistence contracts.
//!
//! These types are deliberately independent from passkey users, native devices,
//! browser sessions, and RockCast machine-client credentials.

use std::{collections::BTreeMap, fmt, sync::Mutex};

use async_trait::async_trait;
use uuid::Uuid;

use crate::auth::SecretHash;

/// Administrator lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminPrincipalStatus {
    /// The principal may authenticate when it has a non-revoked credential.
    Active,
    /// The principal is retained but may not authenticate.
    Disabled,
}

/// A separate administrator identity with no relationship to a Rock account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdminPrincipal {
    /// Stable administrator identifier.
    pub id: Uuid,
    /// Current administrator lifecycle state.
    pub status: AdminPrincipalStatus,
}

/// A validated Argon2id PHC password hash that is safe to persist but redacted in diagnostics.
#[derive(Clone, Eq, PartialEq)]
pub struct AdminPasswordHash(String);

impl AdminPasswordHash {
    /// Accepts an Argon2id PHC hash from the password-hashing boundary.
    pub fn parse(value: String) -> Result<Self, AdminPasswordHashError> {
        if value.starts_with("$argon2id$") {
            Ok(Self(value))
        } else {
            Err(AdminPasswordHashError::NotArgon2id)
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AdminPasswordHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdminPasswordHash([REDACTED])")
    }
}

/// Safe failure while accepting a password-hash representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminPasswordHashError {
    /// The value is not an Argon2id PHC representation.
    NotArgon2id,
}

/// Password credential persisted for a principal without any raw password material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminPasswordCredential {
    /// Credential row identifier.
    pub id: Uuid,
    /// Administrator that owns this credential.
    pub principal_id: Uuid,
    /// Persisted Argon2id PHC hash only.
    pub password_hash: AdminPasswordHash,
}

/// Opaque administrator session resolved from a token hash only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdminSession {
    /// Session row identifier.
    pub id: Uuid,
    /// Administrator that owns the session.
    pub principal_id: Uuid,
}

/// Password credential resolved from a login identifier without exposing principal metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminLoginCredential {
    /// Active password credential for the resolved administrator.
    pub credential: AdminPasswordCredential,
}

/// Durable outcome class for one administrator login attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminLoginOutcome {
    /// Credential verification succeeded.
    Succeeded,
    /// Credential verification failed without exposing why.
    Failed,
    /// Durable throttling rejected the attempt.
    Locked,
}

/// Safe administrator security-event vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminSecurityEventType {
    /// An administrator principal was created through a protected operator path.
    AdminCreated,
    /// A password credential hash was created.
    PasswordCredentialCreated,
    /// Authentication succeeded.
    LoginSucceeded,
    /// Authentication failed.
    LoginFailed,
    /// Authentication was throttled or locked.
    LoginLocked,
    /// An opaque administrator session was created.
    SessionCreated,
    /// An administrator session was revoked.
    SessionRevoked,
    /// An administrator explicitly logged out.
    Logout,
    /// An administrator session was replaced atomically by a fresh bearer session.
    SessionRotated,
}

impl AdminSecurityEventType {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AdminCreated => "admin_created",
            Self::PasswordCredentialCreated => "password_credential_created",
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::LoginLocked => "login_locked",
            Self::SessionCreated => "session_created",
            Self::SessionRevoked => "session_revoked",
            Self::Logout => "logout",
            Self::SessionRotated => "session_rotated",
        }
    }
}

/// Safe outcome class for one authenticated administrator HTTP request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminRequestOutcome {
    /// The protected request completed successfully.
    Succeeded,
    /// The protected request was rejected after session authentication.
    Rejected,
    /// The protected request could not complete because a dependency was unavailable.
    Failed,
}

impl AdminRequestOutcome {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }
}

/// Bounded, secret-free operational record for one authenticated administrator request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminRequestRecord {
    /// Record row identifier.
    pub id: Uuid,
    /// Caller-provided or server-generated correlation identifier.
    pub request_id: String,
    /// Authenticated administrator principal.
    pub principal_id: Uuid,
    /// Authenticated opaque session identifier.
    pub session_id: Uuid,
    /// Static route name, never a URL containing request-controlled values.
    pub endpoint: &'static str,
    /// Safe terminal result class.
    pub outcome: AdminRequestOutcome,
    /// Bounded handler duration measured locally in milliseconds.
    pub duration_ms: u32,
}

/// Secret-free unified administrator audit entry for a read-only console timeline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminAuditEntry {
    /// Event time in RFC 3339 UTC form.
    pub occurred_at: String,
    /// Static event or route label, never a request URL.
    pub action: String,
    /// Safe terminal outcome class.
    pub outcome: String,
}

/// Bounded filters for the administrator audit timeline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminAuditFilter {
    /// Inclusive RFC 3339 UTC lower time bound.
    pub from: Option<String>,
    /// Exclusive RFC 3339 UTC upper time bound.
    pub until: Option<String>,
    /// Optional exact static endpoint or event label.
    pub action: Option<String>,
    /// Optional exact safe result class.
    pub outcome: Option<String>,
    /// Zero-based bounded page offset.
    pub offset: i64,
    /// Bounded page size.
    pub limit: i64,
}

/// Hashed material needed to persist one administrator password credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewAdminPasswordCredential {
    /// Credential row identifier.
    pub id: Uuid,
    /// Administrator that owns the credential.
    pub principal_id: Uuid,
    /// Argon2id PHC hash produced outside persistence.
    pub password_hash: AdminPasswordHash,
}

/// Validated administrator username supplied only by a protected operator boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminUsername(String);

impl AdminUsername {
    /// Validates the bounded identifier reserved for the single bootstrap administrator.
    pub fn parse(value: String) -> Result<Self, AdminUsernameError> {
        let valid = (3..=64).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if valid {
            Ok(Self(value))
        } else {
            Err(AdminUsernameError::Invalid)
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Safe failure while accepting an administrator username.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminUsernameError {
    /// The username is not 3–64 ASCII letters, digits, dots, underscores, or hyphens.
    Invalid,
}

/// Fully hashed data required to atomically create the first administrator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewAdminBootstrap {
    /// New administrator principal identifier.
    pub principal_id: Uuid,
    /// New administrator credential identifier.
    pub credential_id: Uuid,
    /// New security event identifier.
    pub security_event_id: Uuid,
    /// Administrator login identifier.
    pub username: AdminUsername,
    /// Argon2id PHC password hash produced outside persistence.
    pub password_hash: AdminPasswordHash,
}

/// Safe result of attempting the one-time administrator bootstrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminBootstrapOutcome {
    /// The missing administrator and its password credential were created together.
    Created,
    /// An administrator already existed, so no row or credential was changed.
    AlreadyExists,
}

/// Hashed material needed to persist one opaque administrator session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewAdminSession {
    /// Session row identifier.
    pub id: Uuid,
    /// Administrator that owns the session.
    pub principal_id: Uuid,
    /// Fixed-size hash of the random opaque session token.
    pub token_hash: SecretHash,
    /// Short server-side lifetime in seconds, calculated by PostgreSQL's clock.
    pub ttl_seconds: i64,
}

/// One login result with hashed account and source-IP correlation keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminLoginAttempt {
    /// Attempt row identifier.
    pub id: Uuid,
    /// Resolved administrator when one exists; unknown accounts remain anonymous.
    pub principal_id: Option<Uuid>,
    /// Fixed-size hash of the login account key.
    pub account_key_hash: SecretHash,
    /// Fixed-size hash of the source IP address.
    pub source_ip_hash: SecretHash,
    /// Safe result class.
    pub outcome: AdminLoginOutcome,
}

/// A security audit event with optional hashed source-IP correlation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminSecurityEvent {
    /// Event row identifier.
    pub id: Uuid,
    /// Related administrator when known.
    pub principal_id: Option<Uuid>,
    /// Related opaque session when applicable.
    pub session_id: Option<Uuid>,
    /// Fixed-size source-IP hash when known.
    pub source_ip_hash: Option<SecretHash>,
    /// Safe event type.
    pub event_type: AdminSecurityEventType,
}

/// Opaque persistence failures exposed to future administrator authentication flows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminStoreError {
    /// The requested durable record does not exist.
    NotFound,
    /// A uniqueness or lifecycle constraint rejected the requested state.
    Conflict,
    /// Persistence was unavailable without exposing database diagnostics.
    Unavailable,
}

/// Boundary for administrator credential and session verification state.
#[async_trait]
pub trait AdminStore: Send + Sync {
    /// Atomically creates the only administrator principal and its initial credential when absent.
    async fn bootstrap_admin(
        &self,
        bootstrap: NewAdminBootstrap,
    ) -> Result<AdminBootstrapOutcome, AdminStoreError>;
    /// Persists one active administrator principal.
    async fn create_principal(&self, principal: AdminPrincipal) -> Result<(), AdminStoreError>;
    /// Persists an Argon2id password hash for an existing administrator.
    async fn create_password_credential(
        &self,
        credential: NewAdminPasswordCredential,
    ) -> Result<(), AdminStoreError>;
    /// Reads the active password-hash record needed by a future verification boundary.
    async fn active_password_credential(
        &self,
        principal_id: Uuid,
    ) -> Result<Option<AdminPasswordCredential>, AdminStoreError>;
    /// Resolves the active password credential for a login identifier.
    async fn login_credential(
        &self,
        username: &AdminUsername,
    ) -> Result<Option<AdminLoginCredential>, AdminStoreError>;
    /// Persists an opaque administrator session using only its token hash.
    async fn create_session(&self, session: NewAdminSession) -> Result<(), AdminStoreError>;
    /// Resolves an active opaque session from its token hash without accepting raw tokens.
    async fn find_active_session(
        &self,
        token_hash: &SecretHash,
    ) -> Result<Option<AdminSession>, AdminStoreError>;
    /// Counts recent failed attempts for the durable account-and-source-IP throttle key.
    async fn recent_failed_login_count(
        &self,
        account_key_hash: &SecretHash,
        source_ip_hash: &SecretHash,
    ) -> Result<u64, AdminStoreError>;
    /// Revokes a live session, optionally recording its replacement.
    async fn revoke_session(
        &self,
        session_id: Uuid,
        replacement_session_id: Option<Uuid>,
    ) -> Result<bool, AdminStoreError>;
    /// Atomically replaces one active session, leaving no interval with two valid Bearers.
    async fn rotate_session(
        &self,
        session_id: Uuid,
        replacement: NewAdminSession,
    ) -> Result<bool, AdminStoreError>;
    /// Records a durable, non-secret login-attempt result.
    async fn record_login_attempt(&self, attempt: AdminLoginAttempt)
    -> Result<(), AdminStoreError>;
    /// Records a durable, non-secret administrator security event.
    async fn record_security_event(&self, event: AdminSecurityEvent)
    -> Result<(), AdminStoreError>;
    /// Persists a secret-free operational record and enforces its bounded retention window.
    async fn record_request(&self, request: AdminRequestRecord) -> Result<(), AdminStoreError>;
    /// Lists a bounded unified timeline without returning identifiers, hashes, or request content.
    async fn list_audit(
        &self,
        filter: AdminAuditFilter,
    ) -> Result<Vec<AdminAuditEntry>, AdminStoreError>;
}

/// Deterministic in-memory administrator store for unit tests.
#[derive(Default)]
pub struct FakeAdminStore {
    state: Mutex<FakeAdminState>,
}

#[derive(Default)]
struct FakeAdminState {
    principals: BTreeMap<Uuid, AdminPrincipal>,
    usernames: BTreeMap<String, Uuid>,
    credentials: BTreeMap<Uuid, AdminPasswordCredential>,
    sessions: BTreeMap<SecretHashKey, AdminSession>,
    login_attempts: Vec<AdminLoginAttempt>,
    security_events: Vec<AdminSecurityEvent>,
    request_records: Vec<AdminRequestRecord>,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct SecretHashKey([u8; 32]);

impl From<&SecretHash> for SecretHashKey {
    fn from(value: &SecretHash) -> Self {
        let mut bytes = [0; 32];
        bytes.copy_from_slice(value.as_bytes());
        Self(bytes)
    }
}

impl FakeAdminStore {
    /// Returns a deterministic snapshot of all recorded login attempts.
    pub fn login_attempts(&self) -> Vec<AdminLoginAttempt> {
        self.state
            .lock()
            .expect("fake admin store lock poisoned")
            .login_attempts
            .clone()
    }

    /// Returns a deterministic snapshot of all recorded security events.
    pub fn security_events(&self) -> Vec<AdminSecurityEvent> {
        self.state
            .lock()
            .expect("fake admin store lock poisoned")
            .security_events
            .clone()
    }

    /// Returns a deterministic snapshot of bounded administrator request records.
    pub fn request_records(&self) -> Vec<AdminRequestRecord> {
        self.state
            .lock()
            .expect("fake admin store lock poisoned")
            .request_records
            .clone()
    }
}

#[async_trait]
impl AdminStore for FakeAdminStore {
    async fn bootstrap_admin(
        &self,
        bootstrap: NewAdminBootstrap,
    ) -> Result<AdminBootstrapOutcome, AdminStoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?;
        if !state.principals.is_empty() {
            return Ok(AdminBootstrapOutcome::AlreadyExists);
        }
        state.principals.insert(
            bootstrap.principal_id,
            AdminPrincipal {
                id: bootstrap.principal_id,
                status: AdminPrincipalStatus::Active,
            },
        );
        state
            .usernames
            .insert(bootstrap.username.0, bootstrap.principal_id);
        state.credentials.insert(
            bootstrap.principal_id,
            AdminPasswordCredential {
                id: bootstrap.credential_id,
                principal_id: bootstrap.principal_id,
                password_hash: bootstrap.password_hash,
            },
        );
        state.security_events.push(AdminSecurityEvent {
            id: bootstrap.security_event_id,
            principal_id: Some(bootstrap.principal_id),
            session_id: None,
            source_ip_hash: None,
            event_type: AdminSecurityEventType::AdminCreated,
        });
        Ok(AdminBootstrapOutcome::Created)
    }

    async fn create_principal(&self, principal: AdminPrincipal) -> Result<(), AdminStoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?;
        if state.principals.insert(principal.id, principal).is_some() {
            return Err(AdminStoreError::Conflict);
        }
        Ok(())
    }

    async fn create_password_credential(
        &self,
        credential: NewAdminPasswordCredential,
    ) -> Result<(), AdminStoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?;
        if !state.principals.contains_key(&credential.principal_id) {
            return Err(AdminStoreError::NotFound);
        }
        if state.credentials.contains_key(&credential.principal_id) {
            return Err(AdminStoreError::Conflict);
        }
        state.credentials.insert(
            credential.principal_id,
            AdminPasswordCredential {
                id: credential.id,
                principal_id: credential.principal_id,
                password_hash: credential.password_hash,
            },
        );
        Ok(())
    }

    async fn active_password_credential(
        &self,
        principal_id: Uuid,
    ) -> Result<Option<AdminPasswordCredential>, AdminStoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?
            .credentials
            .get(&principal_id)
            .cloned())
    }

    async fn login_credential(
        &self,
        username: &AdminUsername,
    ) -> Result<Option<AdminLoginCredential>, AdminStoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?;
        Ok(state
            .usernames
            .get(&username.0)
            .and_then(|id| state.credentials.get(id))
            .cloned()
            .map(|credential| AdminLoginCredential { credential }))
    }

    async fn create_session(&self, session: NewAdminSession) -> Result<(), AdminStoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?;
        if !matches!(
            state.principals.get(&session.principal_id),
            Some(AdminPrincipal {
                status: AdminPrincipalStatus::Active,
                ..
            })
        ) {
            return Err(AdminStoreError::NotFound);
        }
        let key = SecretHashKey::from(&session.token_hash);
        if state.sessions.contains_key(&key) {
            return Err(AdminStoreError::Conflict);
        }
        state.sessions.insert(
            key,
            AdminSession {
                id: session.id,
                principal_id: session.principal_id,
            },
        );
        Ok(())
    }

    async fn find_active_session(
        &self,
        token_hash: &SecretHash,
    ) -> Result<Option<AdminSession>, AdminStoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?
            .sessions
            .get(&SecretHashKey::from(token_hash))
            .copied())
    }

    async fn recent_failed_login_count(
        &self,
        account_key_hash: &SecretHash,
        source_ip_hash: &SecretHash,
    ) -> Result<u64, AdminStoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?
            .login_attempts
            .iter()
            .filter(|attempt| {
                attempt.outcome == AdminLoginOutcome::Failed
                    && &attempt.account_key_hash == account_key_hash
                    && &attempt.source_ip_hash == source_ip_hash
            })
            .count() as u64)
    }

    async fn revoke_session(
        &self,
        session_id: Uuid,
        _replacement_session_id: Option<Uuid>,
    ) -> Result<bool, AdminStoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?;
        let key = state
            .sessions
            .iter()
            .find_map(|(key, session)| (session.id == session_id).then(|| key.clone()));
        Ok(key.is_some_and(|key| state.sessions.remove(&key).is_some()))
    }

    async fn rotate_session(
        &self,
        session_id: Uuid,
        replacement: NewAdminSession,
    ) -> Result<bool, AdminStoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?;
        let old_key = state
            .sessions
            .iter()
            .find_map(|(key, session)| (session.id == session_id).then(|| key.clone()));
        let replacement_key = SecretHashKey::from(&replacement.token_hash);
        let old_session = old_key
            .as_ref()
            .and_then(|key| state.sessions.get(key))
            .copied();
        if old_session.is_none()
            || old_session.is_some_and(|session| session.principal_id != replacement.principal_id)
            || state.sessions.contains_key(&replacement_key)
        {
            return Ok(false);
        }
        state.sessions.remove(&old_key.expect("checked above"));
        state.sessions.insert(
            replacement_key,
            AdminSession {
                id: replacement.id,
                principal_id: replacement.principal_id,
            },
        );
        Ok(true)
    }

    async fn record_login_attempt(
        &self,
        attempt: AdminLoginAttempt,
    ) -> Result<(), AdminStoreError> {
        self.state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?
            .login_attempts
            .push(attempt);
        Ok(())
    }

    async fn record_security_event(
        &self,
        event: AdminSecurityEvent,
    ) -> Result<(), AdminStoreError> {
        self.state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?
            .security_events
            .push(event);
        Ok(())
    }

    async fn record_request(&self, request: AdminRequestRecord) -> Result<(), AdminStoreError> {
        self.state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?
            .request_records
            .push(request);
        Ok(())
    }

    async fn list_audit(
        &self,
        filter: AdminAuditFilter,
    ) -> Result<Vec<AdminAuditEntry>, AdminStoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| AdminStoreError::Unavailable)?;
        let mut entries = state
            .request_records
            .iter()
            .map(|record| AdminAuditEntry {
                occurred_at: String::new(),
                action: record.endpoint.to_owned(),
                outcome: record.outcome.as_str().to_owned(),
            })
            .collect::<Vec<_>>();
        entries.retain(|entry| {
            filter
                .action
                .as_ref()
                .is_none_or(|action| action == &entry.action)
                && filter
                    .outcome
                    .as_ref()
                    .is_none_or(|outcome| outcome == &entry.outcome)
        });
        Ok(entries
            .into_iter()
            .skip(filter.offset as usize)
            .take(filter.limit as usize)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_requires_argon2id_and_redacts_debug() {
        assert_eq!(
            AdminPasswordHash::parse("password".to_owned()),
            Err(AdminPasswordHashError::NotArgon2id)
        );
        let hash =
            AdminPasswordHash::parse("$argon2id$v=19$m=1,t=1,p=1$salt$hash".to_owned()).unwrap();
        assert_eq!(format!("{hash:?}"), "AdminPasswordHash([REDACTED])");
    }

    #[tokio::test]
    async fn fake_store_requires_a_principal_and_never_accepts_raw_secret_material() {
        let store = FakeAdminStore::default();
        let principal_id = Uuid::new_v4();
        let credential = NewAdminPasswordCredential {
            id: Uuid::new_v4(),
            principal_id,
            password_hash: AdminPasswordHash::parse(
                "$argon2id$v=19$m=1,t=1,p=1$salt$hash".to_owned(),
            )
            .unwrap(),
        };
        assert_eq!(
            store.create_password_credential(credential.clone()).await,
            Err(AdminStoreError::NotFound)
        );
        store
            .create_principal(AdminPrincipal {
                id: principal_id,
                status: AdminPrincipalStatus::Active,
            })
            .await
            .unwrap();
        store.create_password_credential(credential).await.unwrap();
        let token_hash = SecretHash::new([9; 32]);
        store
            .create_session(NewAdminSession {
                id: Uuid::new_v4(),
                principal_id,
                token_hash: token_hash.clone(),
                ttl_seconds: 60,
            })
            .await
            .unwrap();
        assert!(
            store
                .find_active_session(&token_hash)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn fake_store_rejects_sessions_for_disabled_principals() {
        let store = FakeAdminStore::default();
        let principal_id = Uuid::new_v4();
        store
            .create_principal(AdminPrincipal {
                id: principal_id,
                status: AdminPrincipalStatus::Disabled,
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .create_session(NewAdminSession {
                    id: Uuid::new_v4(),
                    principal_id,
                    token_hash: SecretHash::new([5; 32]),
                    ttl_seconds: 60,
                })
                .await,
            Err(AdminStoreError::NotFound)
        );
    }

    #[tokio::test]
    async fn fake_store_rotation_replaces_the_only_active_session_and_records_safe_metadata() {
        let store = FakeAdminStore::default();
        let principal_id = Uuid::new_v4();
        store
            .create_principal(AdminPrincipal {
                id: principal_id,
                status: AdminPrincipalStatus::Active,
            })
            .await
            .unwrap();
        let old_hash = SecretHash::new([7; 32]);
        let old_session_id = Uuid::new_v4();
        store
            .create_session(NewAdminSession {
                id: old_session_id,
                principal_id,
                token_hash: old_hash.clone(),
                ttl_seconds: 60,
            })
            .await
            .unwrap();
        let new_hash = SecretHash::new([8; 32]);
        let replacement_id = Uuid::new_v4();
        assert!(
            store
                .rotate_session(
                    old_session_id,
                    NewAdminSession {
                        id: replacement_id,
                        principal_id,
                        token_hash: new_hash.clone(),
                        ttl_seconds: 60,
                    },
                )
                .await
                .unwrap()
        );
        assert!(
            store
                .find_active_session(&old_hash)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .find_active_session(&new_hash)
                .await
                .unwrap()
                .unwrap()
                .id,
            replacement_id
        );
        store
            .record_request(AdminRequestRecord {
                id: Uuid::new_v4(),
                request_id: "request_123".to_owned(),
                principal_id,
                session_id: replacement_id,
                endpoint: "/v1/admin/stations",
                outcome: AdminRequestOutcome::Succeeded,
                duration_ms: 12,
            })
            .await
            .unwrap();
        assert_eq!(store.request_records().len(), 1);
    }
}

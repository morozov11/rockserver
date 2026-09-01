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
        }
    }
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

/// Hashed material needed to persist one opaque administrator session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewAdminSession {
    /// Session row identifier.
    pub id: Uuid,
    /// Administrator that owns the session.
    pub principal_id: Uuid,
    /// Fixed-size hash of the random opaque session token.
    pub token_hash: SecretHash,
    /// Session expiry in PostgreSQL-validated RFC 3339 UTC form.
    pub expires_at_rfc3339: String,
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
    /// Persists an opaque administrator session using only its token hash.
    async fn create_session(&self, session: NewAdminSession) -> Result<(), AdminStoreError>;
    /// Resolves an active opaque session from its token hash without accepting raw tokens.
    async fn find_active_session(
        &self,
        token_hash: &SecretHash,
    ) -> Result<Option<AdminSession>, AdminStoreError>;
    /// Records a durable, non-secret login-attempt result.
    async fn record_login_attempt(&self, attempt: AdminLoginAttempt)
    -> Result<(), AdminStoreError>;
    /// Records a durable, non-secret administrator security event.
    async fn record_security_event(&self, event: AdminSecurityEvent)
    -> Result<(), AdminStoreError>;
}

/// Deterministic in-memory administrator store for unit tests.
#[derive(Default)]
pub struct FakeAdminStore {
    state: Mutex<FakeAdminState>,
}

#[derive(Default)]
struct FakeAdminState {
    principals: BTreeMap<Uuid, AdminPrincipal>,
    credentials: BTreeMap<Uuid, AdminPasswordCredential>,
    sessions: BTreeMap<SecretHashKey, AdminSession>,
    login_attempts: Vec<AdminLoginAttempt>,
    security_events: Vec<AdminSecurityEvent>,
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
}

#[async_trait]
impl AdminStore for FakeAdminStore {
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
                expires_at_rfc3339: "2035-01-01T00:00:00Z".to_owned(),
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
                    expires_at_rfc3339: "2035-01-01T00:00:00Z".to_owned(),
                })
                .await,
            Err(AdminStoreError::NotFound)
        );
    }
}

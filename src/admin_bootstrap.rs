//! Protected one-shot bootstrap of the first administrator principal.
//!
//! This module has no HTTP integration. Its environment variables are consumed only by the
//! `bootstrap_admin` binary, which operators run from a protected terminal or deployment runtime.

use std::{env, fmt};

use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use uuid::Uuid;

use crate::admin::{
    AdminBootstrapOutcome, AdminPasswordHash, AdminStore, AdminUsername, AdminUsernameError,
    NewAdminBootstrap,
};

/// Environment variable containing the initial administrator username.
pub const ADMIN_BOOTSTRAP_USERNAME_ENV: &str = "ROCKSERVER_ADMIN_BOOTSTRAP_USERNAME";
/// Environment variable containing the initial administrator password.
pub const ADMIN_BOOTSTRAP_PASSWORD_ENV: &str = "ROCKSERVER_ADMIN_BOOTSTRAP_PASSWORD";

/// Protected bootstrap input loaded from a terminal or deployment environment.
pub struct AdminBootstrapConfig {
    username: AdminUsername,
    initial_password: AdminInitialPassword,
}

impl AdminBootstrapConfig {
    /// Loads the protected bootstrap input without reading `.env` files or command-line arguments.
    pub fn from_env() -> Result<Self, AdminBootstrapConfigError> {
        Self::from_values(
            env::var(ADMIN_BOOTSTRAP_USERNAME_ENV).ok(),
            env::var(ADMIN_BOOTSTRAP_PASSWORD_ENV).ok(),
        )
    }

    /// Validates values already obtained from an equally protected explicit configuration boundary.
    pub fn from_values(
        username: Option<String>,
        initial_password: Option<String>,
    ) -> Result<Self, AdminBootstrapConfigError> {
        let username = username
            .filter(|value| !value.trim().is_empty())
            .ok_or(AdminBootstrapConfigError::MissingUsername)
            .and_then(|value| AdminUsername::parse(value).map_err(Into::into))?;
        let initial_password = AdminInitialPassword::parse(
            initial_password.ok_or(AdminBootstrapConfigError::MissingPassword)?,
        )?;
        Ok(Self {
            username,
            initial_password,
        })
    }
}

impl fmt::Debug for AdminBootstrapConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminBootstrapConfig")
            .field("username", &self.username)
            .field("initial_password", &"[REDACTED]")
            .finish()
    }
}

/// Initial password that is redacted from all diagnostics.
struct AdminInitialPassword(String);

impl AdminInitialPassword {
    /// Rejects missing, blank, and short bootstrap passwords before hashing.
    fn parse(value: String) -> Result<Self, AdminBootstrapConfigError> {
        if value.trim().is_empty() {
            return Err(AdminBootstrapConfigError::MissingPassword);
        }
        if value.chars().count() < 16 {
            return Err(AdminBootstrapConfigError::PasswordTooShort);
        }
        Ok(Self(value))
    }
}

impl fmt::Debug for AdminInitialPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdminInitialPassword([REDACTED])")
    }
}

/// Safe validation failure for protected administrator bootstrap configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminBootstrapConfigError {
    /// The protected username environment value is absent or blank.
    MissingUsername,
    /// The username is not a supported administrator identifier.
    InvalidUsername,
    /// The protected initial-password environment value is absent or blank.
    MissingPassword,
    /// The initial password has fewer than 16 Unicode scalar values.
    PasswordTooShort,
}

impl From<AdminUsernameError> for AdminBootstrapConfigError {
    fn from(_: AdminUsernameError) -> Self {
        Self::InvalidUsername
    }
}

impl fmt::Display for AdminBootstrapConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingUsername => {
                write!(formatter, "{ADMIN_BOOTSTRAP_USERNAME_ENV} is required")
            }
            Self::InvalidUsername => write!(formatter, "{ADMIN_BOOTSTRAP_USERNAME_ENV} is invalid"),
            Self::MissingPassword => {
                write!(formatter, "{ADMIN_BOOTSTRAP_PASSWORD_ENV} is required")
            }
            Self::PasswordTooShort => write!(
                formatter,
                "{ADMIN_BOOTSTRAP_PASSWORD_ENV} must contain at least 16 characters"
            ),
        }
    }
}

impl std::error::Error for AdminBootstrapConfigError {}

/// Safe bootstrap failure that never includes password or PHC-hash material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminBootstrapError {
    /// Argon2id could not produce a password hash.
    PasswordHashing,
    /// The database could not complete the protected bootstrap operation.
    StoreUnavailable,
}

impl fmt::Display for AdminBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PasswordHashing => formatter.write_str("administrator password hashing failed"),
            Self::StoreUnavailable => {
                formatter.write_str("administrator bootstrap storage is unavailable")
            }
        }
    }
}

impl std::error::Error for AdminBootstrapError {}

/// Hashes the initial password with Argon2id and atomically persists it only when no admin exists.
pub async fn bootstrap_admin(
    store: &dyn AdminStore,
    config: AdminBootstrapConfig,
) -> Result<AdminBootstrapOutcome, AdminBootstrapError> {
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = Argon2::default()
        .hash_password(config.initial_password.0.as_bytes(), &salt)
        .map_err(|_| AdminBootstrapError::PasswordHashing)?
        .to_string();
    let password_hash = AdminPasswordHash::parse(password_hash)
        .map_err(|_| AdminBootstrapError::PasswordHashing)?;
    store
        .bootstrap_admin(NewAdminBootstrap {
            principal_id: Uuid::new_v4(),
            credential_id: Uuid::new_v4(),
            security_event_id: Uuid::new_v4(),
            username: config.username,
            password_hash,
        })
        .await
        .map_err(|_| AdminBootstrapError::StoreUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::FakeAdminStore;

    #[test]
    fn configuration_rejects_missing_and_invalid_values_without_echoing_the_password() {
        assert!(matches!(
            AdminBootstrapConfig::from_values(None, Some("password-that-must-not-leak".to_owned())),
            Err(AdminBootstrapConfigError::MissingUsername)
        ));
        assert!(matches!(
            AdminBootstrapConfig::from_values(
                Some("bad name".to_owned()),
                Some("password-that-must-not-leak".to_owned())
            ),
            Err(AdminBootstrapConfigError::InvalidUsername)
        ));
        assert!(matches!(
            AdminBootstrapConfig::from_values(Some("admin".to_owned()), Some("short".to_owned())),
            Err(AdminBootstrapConfigError::PasswordTooShort)
        ));
        let error =
            AdminBootstrapConfig::from_values(None, Some("password-that-must-not-leak".to_owned()))
                .unwrap_err();
        assert!(!error.to_string().contains("password-that-must-not-leak"));
    }

    #[tokio::test]
    async fn bootstrap_is_one_time_and_idempotent() {
        let store = FakeAdminStore::default();
        let first = AdminBootstrapConfig::from_values(
            Some("admin".to_owned()),
            Some("a-unique-initial-password".to_owned()),
        )
        .unwrap();
        assert_eq!(
            bootstrap_admin(&store, first).await,
            Ok(AdminBootstrapOutcome::Created)
        );
        let replacement = AdminBootstrapConfig::from_values(
            Some("other-admin".to_owned()),
            Some("a-different-initial-password".to_owned()),
        )
        .unwrap();
        assert_eq!(
            bootstrap_admin(&store, replacement).await,
            Ok(AdminBootstrapOutcome::AlreadyExists)
        );
        assert_eq!(store.security_events().len(), 1);
    }

    #[test]
    fn configuration_debug_redacts_the_initial_password() {
        let config = AdminBootstrapConfig::from_values(
            Some("admin".to_owned()),
            Some("password-that-must-not-leak".to_owned()),
        )
        .unwrap();
        assert!(!format!("{config:?}").contains("password-that-must-not-leak"));
    }
}

//! Authentication foundation for future device-control ingress.

use crate::auth::{NativeSessionLookupError, NativeSessionResolver, SecretHash};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Server-derived account/device identity for one authenticated control connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceControlPrincipal {
    /// Account owning the authenticated paired device.
    pub user_id: Uuid,
    /// Existing paired device identifier owned by [`Self::user_id`].
    pub device_id: Uuid,
}

/// Safe outcome when authenticating a future device-control ingress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceControlAuthenticationError {
    /// The supplied credential is absent, malformed, expired, unknown, revoked, or not native.
    InvalidCredential,
    /// Native-session storage could not be consulted; callers may retry without re-pairing.
    Unavailable,
}

/// Resolves only a short-lived native `Bearer` access token to a server-derived principal.
///
/// This accepts neither browser cookies nor any caller-supplied user/device identity. Legacy and
/// administrator Bearers cannot resolve because the supplied resolver searches native sessions only.
pub async fn authenticate_device_control(
    authorization: Option<&str>,
    resolver: &(impl NativeSessionResolver + ?Sized),
) -> Result<DeviceControlPrincipal, DeviceControlAuthenticationError> {
    let Some(token) = authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty() && value.len() <= 512)
    else {
        return Err(DeviceControlAuthenticationError::InvalidCredential);
    };
    let session = resolver
        .resolve_active_native_session(&token_hash(token))
        .await
        .map_err(|NativeSessionLookupError| DeviceControlAuthenticationError::Unavailable)?
        .ok_or(DeviceControlAuthenticationError::InvalidCredential)?;
    Ok(DeviceControlPrincipal {
        user_id: session.user_id,
        device_id: session.device_id,
    })
}

/// Hashes an opaque bearer only at the persistence boundary; callers never retain or log it.
fn token_hash(token: &str) -> SecretHash {
    SecretHash::new(Sha256::digest(token.as_bytes()).into())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::auth::ActiveSession;

    #[derive(Default)]
    struct FakeResolver {
        sessions: HashMap<Vec<u8>, ActiveSession>,
        unavailable: bool,
    }

    #[async_trait::async_trait]
    impl NativeSessionResolver for FakeResolver {
        async fn resolve_active_native_session(
            &self,
            access_hash: &SecretHash,
        ) -> Result<Option<ActiveSession>, NativeSessionLookupError> {
            if self.unavailable {
                return Err(NativeSessionLookupError);
            }
            Ok(self.sessions.get(access_hash.as_bytes()).copied())
        }
    }

    fn resolver_for(token: &str, session: ActiveSession) -> FakeResolver {
        let mut resolver = FakeResolver::default();
        resolver
            .sessions
            .insert(token_hash(token).as_bytes().to_vec(), session);
        resolver
    }

    #[tokio::test]
    async fn native_token_derives_the_principal_without_client_identity_fields() {
        let session = ActiveSession {
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
        };
        let resolver = resolver_for("native-token", session);

        assert_eq!(
            authenticate_device_control(Some("Bearer native-token"), &resolver).await,
            Ok(DeviceControlPrincipal {
                user_id: session.user_id,
                device_id: session.device_id,
            })
        );
    }

    #[tokio::test]
    async fn malformed_unknown_and_non_native_credentials_are_rejected() {
        let resolver = FakeResolver::default();
        for authorization in [
            None,
            Some("RockserverBearer native-token"),
            Some("Bearer "),
            Some("Bearer expired-token"),
            Some("Bearer legacy-rockcast-token"),
            Some("Bearer admin-token"),
        ] {
            assert_eq!(
                authenticate_device_control(authorization, &resolver).await,
                Err(DeviceControlAuthenticationError::InvalidCredential)
            );
        }
    }

    #[tokio::test]
    async fn unavailable_session_store_is_retryable_and_not_an_auth_rejection() {
        let resolver = FakeResolver {
            unavailable: true,
            ..FakeResolver::default()
        };
        assert_eq!(
            authenticate_device_control(Some("Bearer native-token"), &resolver).await,
            Err(DeviceControlAuthenticationError::Unavailable)
        );
    }
}

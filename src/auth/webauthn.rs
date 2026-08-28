//! Pure-Rust WebAuthn ceremony adapter with the deployment's fixed RP policy.

use passkey_auth::{
    AuthSuccess, AuthenticationChallenge, AuthenticationResponse, AuthenticationState,
    CredentialId, PasskeyCredential, RegistrationChallenge, RegistrationResponse,
    RegistrationState, Webauthn,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The only relying-party identifier accepted by RockServer browser ceremonies.
pub const RP_ID: &str = "alex.vault57.ru";
/// The only first-party origin accepted by RockServer browser ceremonies.
pub const ORIGIN: &str = "https://alex.vault57.ru";

/// Authentication state persisted with the account owner to prevent cross-account assertions.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuthenticationStateContext {
    /// Legacy account binding, retained only so in-flight pre-discovery states deserialize.
    #[serde(default)]
    pub user_id: Option<Uuid>,
    /// Verifier state produced by `passkey-auth`.
    pub state: AuthenticationState,
}

/// Creates the strict passkey verifier used by registration and authentication.
pub fn verifier() -> Webauthn {
    Webauthn::new(RP_ID, "RockServer", ORIGIN)
        .strict_base64(true)
        .require_user_verification(true)
}

/// Starts a registration ceremony and returns JSON-safe challenge/state values.
pub fn start_registration(
    user_id: Uuid,
    account_display_name: &str,
) -> (RegistrationChallenge, RegistrationState) {
    verifier().start_registration(
        user_id.as_bytes(),
        account_display_name,
        account_display_name,
        &[],
    )
}

/// Verifies a browser registration response, including origin, RP ID and attestation signature.
pub fn finish_registration(
    state: &RegistrationState,
    response: &RegistrationResponse,
) -> Result<PasskeyCredential, String> {
    verifier()
        .finish_registration(state, response)
        .map_err(|error| error.to_string())
}

/// Starts a discoverable, username-less authentication ceremony.
pub fn start_authentication() -> (AuthenticationChallenge, AuthenticationStateContext) {
    let (challenge, state) = verifier().start_authentication(&[]);
    (
        challenge,
        AuthenticationStateContext {
            user_id: None,
            state,
        },
    )
}

/// Errors raised when a discoverable assertion does not carry a usable account handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserHandleError {
    /// The authenticator omitted the handle required to identify the account.
    Missing,
    /// The handle was not the exact 16-byte UUID encoded by RockServer.
    Invalid,
}

/// Resolves the account owner from the WebAuthn user handle, never from client account input.
pub fn user_id_from_handle(user_handle: Option<&str>) -> Result<Uuid, UserHandleError> {
    let user_handle = user_handle.ok_or(UserHandleError::Missing)?;
    let bytes = CredentialId::from_b64url(user_handle).map_err(|_| UserHandleError::Invalid)?;
    Uuid::from_slice(bytes.as_bytes()).map_err(|_| UserHandleError::Invalid)
}

/// Verifies a browser assertion, including origin, RP ID, challenge and signature.
pub fn finish_authentication(
    state: &AuthenticationStateContext,
    response: &AuthenticationResponse,
    credential: &PasskeyCredential,
) -> Result<AuthSuccess, String> {
    verifier()
        .finish_authentication(&state.state, response, credential)
        .map_err(|error| error.to_string())
}

/// Serializes server-only ceremony state for the PostgreSQL challenge context.
pub fn encode_state<T: Serialize>(state: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(state)
}

/// Restores server-only ceremony state and rejects malformed or client-substituted values.
pub fn decode_state<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discoverable_authentication_has_no_account_or_allow_list() {
        let (challenge, state) = start_authentication();

        assert!(challenge.allow_credentials.is_empty());
        assert!(state.user_id.is_none());
        assert!(state.state.allow_credentials.is_empty());
        assert_eq!(challenge.rp_id, RP_ID);
    }

    #[test]
    fn user_handle_resolves_only_a_uuid() {
        let user_id = Uuid::new_v4();
        let handle = CredentialId(user_id.as_bytes().to_vec()).to_b64url();

        assert_eq!(user_id_from_handle(Some(&handle)), Ok(user_id));
        assert_eq!(user_id_from_handle(None), Err(UserHandleError::Missing));
        assert_eq!(
            user_id_from_handle(Some("bm90LWEtdXVpZA")),
            Err(UserHandleError::Invalid)
        );
    }

    #[test]
    fn legacy_authentication_state_still_deserializes() {
        let state = start_authentication().1.state;
        let legacy = serde_json::json!({
            "user_id": Uuid::new_v4(),
            "state": state,
        });

        let decoded: AuthenticationStateContext =
            serde_json::from_value(legacy).expect("legacy state shape remains readable");
        assert!(decoded.user_id.is_some());
    }
}

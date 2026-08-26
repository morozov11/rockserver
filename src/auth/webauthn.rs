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
    /// Account whose credential allow-list was used to create the challenge.
    pub user_id: Uuid,
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
pub fn start_registration(user_id: Uuid) -> (RegistrationChallenge, RegistrationState) {
    verifier().start_registration(
        user_id.as_bytes(),
        &user_id.to_string(),
        "RockServer user",
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

/// Starts an authentication ceremony for registered credential IDs.
pub fn start_authentication(
    user_id: Uuid,
    ids: &[CredentialId],
) -> (AuthenticationChallenge, AuthenticationStateContext) {
    let (challenge, state) = verifier().start_authentication(ids);
    (challenge, AuthenticationStateContext { user_id, state })
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

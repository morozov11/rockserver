//! HTTP extractor shared by future device-control ingress handlers.

use axum::http::{HeaderMap, header};

use crate::{
    auth::NativeSessionResolver,
    device_control_auth::{
        DeviceControlAuthenticationError, DeviceControlPrincipal, authenticate_device_control,
    },
};

/// Authenticates a future control ingress without accepting client-claimed identity fields.
///
/// Transport handlers map `InvalidCredential` to a generic 401 and `Unavailable` to a retryable
/// 503 before a WebSocket upgrade. This does not implement that upgrade or connection lifecycle.
pub async fn authenticate_control_ingress(
    headers: &HeaderMap,
    resolver: &(impl NativeSessionResolver + ?Sized),
) -> Result<DeviceControlPrincipal, DeviceControlAuthenticationError> {
    authenticate_device_control(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        resolver,
    )
    .await
}

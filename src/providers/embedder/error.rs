//! Mapping of embedding-service gRPC failures to provider-safe summaries and
//! breaker outcomes.

use prost::Message;
use tonic::Status;

use crate::grpc::rpc::error_details::{ErrorInfo, Status as RpcStatus};

/// Type URL of `google.rpc.ErrorInfo` inside status details (contract §1).
const ERROR_INFO_TYPE_URL: &str = "type.googleapis.com/google.rpc.ErrorInfo";

/// Whether a failed call counts toward opening the service breaker.
///
/// Contract §6: only transport, timeouts, and internal-like statuses are
/// breaker failures; contract answers (validation, auth, model state) mean a
/// healthy service and the caller simply degrades to the fallback path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EmbedderFailure {
    /// Transport, deadline, or internal failure: breaker counts it.
    Transient,
    /// Service refused the call by contract: degrade without opening the breaker.
    ServiceRefused,
}

impl EmbedderFailure {
    /// True when this failure must advance the circuit breaker.
    pub(super) fn is_transient(self) -> bool {
        matches!(self, Self::Transient)
    }
}

/// Decodes the contract `reason` from `google.rpc.ErrorInfo` status details;
/// `None` when the status carries no recognizable ErrorInfo.
pub(super) fn error_reason(status: &Status) -> Option<String> {
    let rich = RpcStatus::decode(status.details()).ok()?;
    let any = rich.details.first()?;
    if any.type_url != ERROR_INFO_TYPE_URL {
        return None;
    }
    let info = ErrorInfo::decode(any.value.as_slice()).ok()?;
    Some(info.reason)
}

/// Classifies a failed call by contract reason (preferred) or gRPC code.
pub(super) fn classify(status: &Status) -> EmbedderFailure {
    match error_reason(status).as_deref() {
        Some("unauthorized")
        | Some("invalid_request")
        | Some("language_unsupported")
        | Some("limit_exceeded")
        | Some("model_not_loaded")
        | Some("providers_unavailable")
        | Some("invalid_model_output")
        | Some("unknown_taxonomy_version")
        | Some("taxonomy_version_conflict") => EmbedderFailure::ServiceRefused,
        Some("deadline_exceeded") | Some("internal") => EmbedderFailure::Transient,
        // No ErrorInfo: fall back to transport-oriented gRPC codes. Cancelled
        // covers the deadline case as tonic surfaces server-side grpc-timeout
        // enforcement (contract §1: expired deadline = refusal-grade failure).
        Some(_) | None => match status.code() {
            tonic::Code::Unavailable
            | tonic::Code::DeadlineExceeded
            | tonic::Code::Cancelled
            | tonic::Code::Internal
            | tonic::Code::Unknown
            | tonic::Code::Unimplemented => EmbedderFailure::Transient,
            _ => EmbedderFailure::ServiceRefused,
        },
    }
}

/// Builds a provider-safe log summary from a status; never includes the
/// service message text (it is not needed and must not reach users).
pub(super) fn safe_status_summary(context: &str, status: &Status) -> String {
    format!(
        "{context} failed (code={:?}, reason={:?})",
        status.code(),
        error_reason(status)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_with_reason(code: tonic::Code, reason: &str) -> Status {
        let info = ErrorInfo {
            reason: reason.to_owned(),
            domain: "rock-llm-service".to_owned(),
            metadata: Default::default(),
        };
        let any = prost_types::Any {
            type_url: ERROR_INFO_TYPE_URL.to_owned(),
            value: info.encode_to_vec(),
        };
        let rich = RpcStatus {
            code: code as i32,
            message: "no user data".to_owned(),
            details: vec![any],
        };
        Status::with_details(
            code,
            "no user data",
            bytes::Bytes::from(rich.encode_to_vec()),
        )
    }

    #[test]
    fn contract_reasons_map_to_breaker_outcomes() {
        assert_eq!(
            classify(&status_with_reason(
                tonic::Code::Unavailable,
                "model_not_loaded"
            )),
            EmbedderFailure::ServiceRefused
        );
        assert_eq!(
            classify(&status_with_reason(
                tonic::Code::Unauthenticated,
                "unauthorized"
            )),
            EmbedderFailure::ServiceRefused
        );
        assert_eq!(
            classify(&status_with_reason(
                tonic::Code::DeadlineExceeded,
                "deadline_exceeded"
            )),
            EmbedderFailure::Transient
        );
        assert_eq!(
            classify(&status_with_reason(tonic::Code::Internal, "internal")),
            EmbedderFailure::Transient
        );
        // Transport-shaped statuses without ErrorInfo are breaker failures.
        assert_eq!(
            classify(&Status::unavailable("transport")),
            EmbedderFailure::Transient
        );
        assert_eq!(
            classify(&Status::cancelled("deadline enforcement")),
            EmbedderFailure::Transient
        );
        assert_eq!(
            classify(&Status::invalid_argument("bad")),
            EmbedderFailure::ServiceRefused
        );
    }
}

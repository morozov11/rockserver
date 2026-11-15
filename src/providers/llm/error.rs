//! Error classification, ErrorInfo extraction, and circuit-breaker mapping for the LLM service.

use std::{error::Error, fmt};

use prost::Message;
use tonic::Status;

use crate::grpc::rpc::error_details::{ErrorInfo, Status as RpcStatus};

/// Type URL of `google.rpc.ErrorInfo` inside status details (contract §1).
const ERROR_INFO_TYPE_URL: &str = "type.googleapis.com/google.rpc.ErrorInfo";

/// Whether a failed call counts toward opening the service breaker.
///
/// Contract §6: only transport, timeouts, and internal-like statuses are
/// breaker failures; contract answers (validation, auth, model state,
/// provider exhaustion) mean a healthy service and the caller simply degrades
/// to the fallback path without opening the breaker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LlmFailure {
    /// Transport, deadline, or internal failure: breaker counts it.
    Transient,
    /// Service refused the call or reported upstream provider issues: degrade without opening breaker.
    ServiceRefused,
}

impl LlmFailure {
    /// True when this failure must advance the circuit breaker.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Transient)
    }
}

/// Decodes the contract `reason` from `google.rpc.ErrorInfo` status details;
/// `None` when the status carries no recognizable ErrorInfo.
pub fn error_reason(status: &Status) -> Option<String> {
    let rich = RpcStatus::decode(status.details()).ok()?;
    let any = rich.details.first()?;
    if any.type_url != ERROR_INFO_TYPE_URL {
        return None;
    }
    let info = ErrorInfo::decode(any.value.as_slice()).ok()?;
    Some(info.reason)
}

/// Classifies a failed call by contract reason (preferred) or gRPC code.
pub fn classify(status: &Status) -> LlmFailure {
    match error_reason(status).as_deref() {
        Some("unauthorized")
        | Some("invalid_request")
        | Some("language_unsupported")
        | Some("limit_exceeded")
        | Some("model_not_loaded")
        | Some("providers_unavailable")
        | Some("invalid_model_output")
        | Some("unknown_taxonomy_version")
        | Some("taxonomy_version_conflict") => LlmFailure::ServiceRefused,
        Some("deadline_exceeded") | Some("internal") => LlmFailure::Transient,
        // No ErrorInfo: fall back to transport-oriented gRPC codes.
        Some(_) | None => match status.code() {
            tonic::Code::Unavailable
            | tonic::Code::DeadlineExceeded
            | tonic::Code::Cancelled
            | tonic::Code::Internal
            | tonic::Code::Unknown
            | tonic::Code::Unimplemented => LlmFailure::Transient,
            _ => LlmFailure::ServiceRefused,
        },
    }
}

/// Builds a provider-safe log summary from a status; never includes the
/// service message text (it is not needed and must not reach users).
pub fn safe_status_summary(context: &str, status: &Status) -> String {
    format!(
        "{context} failed (code={:?}, reason={:?})",
        status.code(),
        error_reason(status)
    )
}

/// Client-side error produced by the LLM service client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlmClientError {
    message: String,
    reason: Option<String>,
    code: Option<tonic::Code>,
    transient: bool,
}

impl LlmClientError {
    /// Creates a safe error with no gRPC status.
    pub fn safe(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: None,
            code: None,
            transient: false,
        }
    }

    /// Creates a transient error that trips or is caused by the circuit breaker.
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: None,
            code: None,
            transient: true,
        }
    }

    /// Maps a gRPC status according to contract §1 / §6 rules.
    pub fn from_status(context: &str, status: &Status) -> Self {
        let reason = error_reason(status);
        let failure = classify(status);
        Self {
            message: safe_status_summary(context, status),
            reason,
            code: Some(status.code()),
            transient: failure.is_transient(),
        }
    }

    /// Returns the contract `reason` string, if present.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Returns the underlying gRPC code, if present.
    pub fn code(&self) -> Option<tonic::Code> {
        self.code
    }

    /// Returns whether this error was considered transient by the circuit breaker.
    pub fn is_transient(&self) -> bool {
        self.transient
    }
}

impl fmt::Display for LlmClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for LlmClientError {}

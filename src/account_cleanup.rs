//! Safe, preview-first account cleanup commands for staging operators.

use std::{fmt, str::FromStr};

use serde::Serialize;
use uuid::Uuid;

/// The narrowly scoped destructive action supported by the cleanup operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupAction {
    /// Deactivate one account and revoke every account-owned access record.
    DeactivateAccount,
    /// Revoke one device and its native sessions and refresh tokens.
    RevokeDevice,
    /// Revoke one passkey credential row without exposing its WebAuthn material.
    RevokeCredential,
}

impl CleanupAction {
    /// Returns the action word used in the human confirmation phrase.
    pub const fn confirmation_word(self) -> &'static str {
        match self {
            Self::DeactivateAccount => "DEACTIVATE ACCOUNT",
            Self::RevokeDevice => "REVOKE DEVICE",
            Self::RevokeCredential => "REVOKE CREDENTIAL",
        }
    }
}

/// Builds the exact confirmation phrase required for one cleanup target.
pub fn confirmation_for(action: CleanupAction, id: Uuid) -> String {
    format!("{} {id}", action.confirmation_word())
}

/// Safe failures while checking an operator confirmation phrase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationError {
    /// The operator did not provide a confirmation phrase.
    Missing,
    /// The phrase did not name the exact requested action and UUID.
    Mismatch,
}

impl fmt::Display for ConfirmationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("an exact confirmation phrase is required"),
            Self::Mismatch => formatter.write_str("confirmation does not match the exact target"),
        }
    }
}

/// Checks an operator confirmation without echoing the supplied value.
pub fn validate_confirmation(
    action: CleanupAction,
    id: Uuid,
    supplied: Option<&str>,
) -> Result<(), ConfirmationError> {
    let Some(supplied) = supplied else {
        return Err(ConfirmationError::Missing);
    };
    if supplied == confirmation_for(action, id) {
        Ok(())
    } else {
        Err(ConfirmationError::Mismatch)
    }
}

/// A dependency attached to an account in a cleanup preview.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupDependency {
    /// Stable server-side row identifier; token hashes and credential bytes are never returned.
    pub id: Uuid,
    /// Sanitized dependency kind, such as `device` or `session`.
    pub kind: String,
    /// Safe lifecycle status of the dependency.
    pub status: String,
    /// Safe explanation of whether the dependency is actionable or retained history.
    pub candidate_reason: String,
    /// Creation or issuance time in UTC RFC 3339 form.
    pub created_at: String,
    /// Last non-secret activity time, when the table has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    /// Expiry time, when the dependency has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Safe relationship to a device, session, or pairing request when relevant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_id: Option<Uuid>,
    /// Safe audit event class; arbitrary audit details are never returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
}

/// Counts of records affected or found for one account.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CleanupCounts {
    /// Number of passkey credential rows.
    pub passkeys: usize,
    /// Number of account identity rows.
    pub account_identities: usize,
    /// Number of device rows.
    pub devices: usize,
    /// Number of native session rows.
    pub sessions: usize,
    /// Number of browser-session rows.
    pub browser_sessions: usize,
    /// Number of pairing-request rows tied to the account.
    pub pairing_requests: usize,
    /// Number of WebAuthn challenge rows tied to the account.
    pub webauthn_challenges: usize,
    /// Number of retained audit-event rows.
    pub audit_events: usize,
}

impl CleanupCounts {
    /// Increments the count matching one sanitized dependency kind.
    pub fn count_dependency(&mut self, kind: &str) {
        match kind {
            "passkey" => self.passkeys += 1,
            "account_identity" => self.account_identities += 1,
            "device" => self.devices += 1,
            "session" => self.sessions += 1,
            "browser_session" => self.browser_sessions += 1,
            "pairing_request" => self.pairing_requests += 1,
            "webauthn_challenge" => self.webauthn_challenges += 1,
            "audit_event" => self.audit_events += 1,
            _ => {}
        }
    }
}

/// One account and its server-side cleanup dependencies.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupAccount {
    /// Stable account UUID required for any operator action.
    pub account_id: Uuid,
    /// Current account lifecycle status.
    pub status: String,
    /// Account creation time in UTC RFC 3339 form.
    pub created_at: String,
    /// Tombstone time when the account was already deactivated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    /// Whether the account needs manual review before any action.
    pub candidate_status: String,
    /// Safe explanation for the candidate status.
    pub candidate_reason: String,
    /// Whether an active admin identity blocks account deactivation.
    pub protected: bool,
    /// Dependency counts that help an operator compare the preview with expectations.
    pub dependency_counts: CleanupCounts,
    /// Non-secret dependency rows needed to select one exact target.
    pub dependencies: Vec<CleanupDependency>,
}

/// Complete read-only cleanup preview.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupPreview {
    /// All account rows; no automatic test-account inference or deletion is performed.
    pub accounts: Vec<CleanupAccount>,
}

/// Result of one confirmed account, device, or credential action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupActionResult {
    /// Action performed after exact confirmation.
    pub action: String,
    /// Exact target row supplied by the operator.
    pub target_id: Uuid,
    /// Account that owned the target at the time of the transaction.
    pub account_id: Uuid,
    /// Resulting safe lifecycle status.
    pub status: String,
    /// Records revoked by the transaction.
    pub revoked: CleanupCounts,
    /// Audit row created by the transaction.
    pub audit_event_id: Uuid,
}

/// Safe operator mutation failures; database diagnostics are intentionally redacted.
#[derive(Debug, Eq, PartialEq)]
pub enum CleanupError {
    /// The exact target does not exist or is not currently active.
    NotFound,
    /// The target is already inactive and cannot be acted on twice.
    NotActive,
    /// The target account carries an active protected admin identity.
    ProtectedAccount,
    /// The action was not accompanied by the exact target confirmation phrase.
    InvalidConfirmation,
    /// Revoking this row would remove the last working passkey from a live account.
    LastWorkingPasskey,
    /// The persistence operation failed without exposing database details.
    Database,
}

impl fmt::Display for CleanupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("exact target was not found"),
            Self::NotActive => formatter.write_str("exact target is already inactive"),
            Self::ProtectedAccount => {
                formatter.write_str("protected admin access blocks this action")
            }
            Self::InvalidConfirmation => {
                formatter.write_str("confirmation does not match the exact target")
            }
            Self::LastWorkingPasskey => {
                formatter.write_str("the last working passkey must be retained")
            }
            Self::Database => formatter.write_str("database operation failed"),
        }
    }
}

impl FromStr for CleanupAction {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "account" => Ok(Self::DeactivateAccount),
            "device" => Ok(Self::RevokeDevice),
            "credential" => Ok(Self::RevokeCredential),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CleanupAction, ConfirmationError, confirmation_for, validate_confirmation};
    use uuid::Uuid;

    #[test]
    fn exact_confirmation_is_required_for_one_target() {
        let id = Uuid::nil();
        let expected = confirmation_for(CleanupAction::DeactivateAccount, id);
        assert_eq!(
            validate_confirmation(CleanupAction::DeactivateAccount, id, Some(&expected)),
            Ok(())
        );
        assert_eq!(
            validate_confirmation(CleanupAction::DeactivateAccount, id, None),
            Err(ConfirmationError::Missing)
        );
        assert_eq!(
            validate_confirmation(CleanupAction::DeactivateAccount, id, Some("*")),
            Err(ConfirmationError::Mismatch)
        );
        assert_eq!(
            validate_confirmation(CleanupAction::RevokeDevice, id, Some(&expected)),
            Err(ConfirmationError::Mismatch)
        );
    }
}

//! Command lifecycle acknowledgements and terminal-result invariants.

use serde::{Deserialize, Serialize};

use super::{CommandId, Timestamp, ValidationError};

/// Receipt acknowledgement for a command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandReceived {
    pub command_id: CommandId,
    pub received_at: Timestamp,
    pub duplicate: bool,
}
/// Target acknowledgement that work started.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandAccepted {
    pub command_id: CommandId,
    pub accepted_at: Timestamp,
}
/// Terminal command result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub command_id: CommandId,
    pub status: CommandStatus,
    pub completed_at: Timestamp,
    pub error: Option<DomainError>,
}
/// Terminal command status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Succeeded,
    Failed,
}
/// Safe structured domain error without HTTP coupling.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DomainError {
    pub code: String,
    pub message: String,
}
impl CommandResult {
    /// Checks the one terminal outcome invariant.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if matches!(
            (&self.status, &self.error),
            (CommandStatus::Succeeded, None) | (CommandStatus::Failed, Some(_))
        ) {
            Ok(())
        } else {
            Err(ValidationError::InvalidPayload {
                field: "command result",
            })
        }
    }
}

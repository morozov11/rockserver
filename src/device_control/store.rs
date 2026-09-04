//! Owner-scoped durable device-control store boundary and safe persistence outcomes.

use async_trait::async_trait;
use std::fmt;
use uuid::Uuid;

use super::{
    CommandId, CommandResult, DeviceCapability, DeviceCommand, DeviceId, DeviceManifest,
    DeviceStateSnapshot, Entity, EntityState, Surface, Timestamp, ValidationError,
};

/// Safe persistence errors; database details are intentionally not exposed to protocol callers.
#[derive(Debug)]
pub enum StoreError {
    /// The input failed existing domain validation.
    Validation(ValidationError),
    /// The persistence backend failed without exposing its raw error string.
    Database,
}
impl StoreError {
    /// Converts a database-layer failure into the safe store error.
    pub fn database<E>(_error: E) -> Self {
        Self::Database
    }
    /// Converts a validation failure into the safe store error.
    pub fn validation(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Validation(error) => error.code(),
            Self::Database => "persistence_unavailable",
        })
    }
}
impl std::error::Error for StoreError {}

/// Result of an ownership-scoped persistence mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreOutcome {
    Accepted,
    Replay,
    Stale,
    Conflict,
    Resync,
    NotOwned,
}

/// Canonical command reservation payload; the caller supplies a SHA-256 request fingerprint.
#[derive(Clone, Debug)]
pub struct CommandReservation {
    /// Validated request which supplies the idempotency key and explicit target.
    pub command: DeviceCommand,
    /// Fixed-size canonical request fingerprint.
    pub fingerprint: [u8; 32],
    /// RFC3339 command deadline.
    pub deadline_at: Timestamp,
}

/// Persisted command projection with an optional terminal result.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandLifecycle {
    /// Original validated request.
    pub command: DeviceCommand,
    /// Terminal result, when the lifecycle has completed.
    pub result: Option<CommandResult>,
}

/// Owner-scoped durable projections used by future transport and directory layers.
#[async_trait]
pub trait DeviceControlStore: Send + Sync {
    /// Atomically accepts a full manifest or reports its revision outcome.
    async fn apply_manifest(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        manifest: DeviceManifest,
    ) -> Result<StoreOutcome, StoreError>;
    /// Loads the accepted full manifest for an owned active device.
    async fn load_manifest(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Option<DeviceManifest>, StoreError>;
    /// Lists current, non-tombstoned entity projections.
    async fn list_entities(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Vec<Entity>, StoreError>;
    /// Lists current, non-tombstoned surface projections.
    async fn list_surfaces(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Vec<Surface>, StoreError>;
    /// Lists current, non-tombstoned capability projections.
    async fn list_capabilities(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Vec<DeviceCapability>, StoreError>;
    /// Stores one latest complete device-state snapshot.
    async fn store_device_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        state: DeviceStateSnapshot,
    ) -> Result<StoreOutcome, StoreError>;
    /// Loads the latest complete device-state snapshot for an owned active device.
    async fn load_device_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Option<DeviceStateSnapshot>, StoreError>;
    /// Stores one latest entity-state snapshot.
    async fn store_entity_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        state: EntityState,
    ) -> Result<StoreOutcome, StoreError>;
    /// Loads the latest entity-state snapshot when the entity belongs to the owned active device.
    async fn load_entity_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        entity_id: &str,
    ) -> Result<Option<EntityState>, StoreError>;
    /// Atomically reserves a bounded idempotency command record.
    async fn reserve_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        request: CommandReservation,
    ) -> Result<StoreOutcome, StoreError>;
    /// Returns a bounded command lifecycle for its owner and target device.
    async fn load_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        command_id: CommandId,
    ) -> Result<Option<CommandLifecycle>, StoreError>;
    /// Writes a terminal command result once.
    async fn complete_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        result: CommandResult,
    ) -> Result<StoreOutcome, StoreError>;
    /// Deletes at most `limit` expired terminal command records.
    async fn prune_commands(&self, limit: u32) -> Result<u64, StoreError>;
}

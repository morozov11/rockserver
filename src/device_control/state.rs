//! Runtime state and entity observations, separate from static capability declarations.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Entity, Timestamp, ValidationError,
    validation::{bounded, valid_entity_id},
};

/// Observed device runtime state, distinct from declared capabilities and commands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct DeviceRuntimeState {
    pub playback: Option<PlaybackState>,
    pub volume: Option<VolumeState>,
    pub display: Option<DisplayState>,
}
/// Playback's current observed state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaybackState {
    pub status: String,
    pub station_id: Option<String>,
}
/// Volume's current observed state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolumeState {
    pub level: u8,
    pub muted: bool,
}
/// Display's current observed state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayState {
    pub surface_id: String,
    pub view: String,
}
/// Complete runtime-state observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceStateSnapshot {
    pub state_revision: u64,
    pub observed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<Timestamp>,
    pub state: DeviceRuntimeState,
}
/// Incremental runtime-state observation requiring a known base revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceStateDelta {
    pub base_revision: u64,
    pub state_revision: u64,
    pub observed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<Timestamp>,
    pub changes: DeviceRuntimeState,
}

/// Quality of an observed entity value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Ok,
    Degraded,
    Unavailable,
    Unknown,
}
/// Derived freshness state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
    Unknown,
}
/// Online/offline presence without a connection registry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub status: PresenceStatus,
    pub last_seen_at: Option<Timestamp>,
}
/// Presence status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Online,
    Offline,
}
/// Normalized latest entity value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    pub entity_id: String,
    pub entity_revision: u64,
    pub value: Value,
    pub unit: Option<String>,
    pub quality: Quality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Freshness>,
    pub observed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<Timestamp>,
    pub stale_after: Timestamp,
}
impl EntityState {
    /// Derives freshness without consulting a wall clock.
    pub fn freshness_at(&self, now: &Timestamp) -> Freshness {
        if self.received_at.is_none() {
            Freshness::Unknown
        } else if now.instant() > self.stale_after.instant() {
            Freshness::Stale
        } else {
            Freshness::Fresh
        }
    }
    /// Checks normalized unit/value compatibility against declared entity metadata.
    pub fn validate_for(&self, entity: &Entity) -> Result<(), ValidationError> {
        valid_entity_id(&self.entity_id)?;
        if self.entity_revision == 0 {
            return Err(ValidationError::InvalidPayload {
                field: "entity_revision",
            });
        }
        if let Some(n) = self.value.as_f64()
            && !n.is_finite()
        {
            return Err(ValidationError::InvalidPayload { field: "value" });
        }
        if let Some(s) = self.value.as_str() {
            bounded(s, 0, 256, "value")?;
        }
        let numeric = self.value.is_number();
        let unit = self.unit.as_deref();
        let expected = match entity.device_class.as_str() {
            "temperature" => Some("°C"),
            "humidity" => Some("%"),
            "co2" => Some("ppm"),
            _ => None,
        };
        if expected.is_some() && (!numeric || unit != expected) {
            return Err(ValidationError::InvalidPayload {
                field: "unit/value",
            });
        }
        if let Some(n) = self.value.as_f64() {
            if let Some(min) = entity.minimum
                && n < min
            {
                return Err(ValidationError::InvalidPayload { field: "minimum" });
            }
            if let Some(max) = entity.maximum
                && n > max
            {
                return Err(ValidationError::InvalidPayload { field: "maximum" });
            }
        }
        Ok(())
    }
}

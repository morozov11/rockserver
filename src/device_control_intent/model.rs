//! Public typed intent schema and transport-free resolution boundary.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::device_control::{
    ActuatorAction, CommandBody, CommandId, CommandTarget, Device, DeviceControlScope, DeviceId,
    DeviceManifest, DeviceRuntimeState, EntityState, Presentation, ValidationError, VolumeCommand,
};

const MAX_INTENT_TEXT: usize = 128;
const MAX_AREA_ID: usize = 64;

/// Schema-valid request vocabulary that a future parser or LLM may emit.
///
/// It intentionally contains no raw command body, provider method, SQL fragment, URL, or tool
/// instruction. Normal code resolves and authorizes it before any command becomes executable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UserIntent {
    /// Plays one stable RockServer catalog station on a selected player.
    PlayRadio {
        /// Bounded structured catalog reference; it is never a stream URI or provider query.
        station_id: String,
        /// Optional typed player selector.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentTarget>,
    },
    /// Builds a sensor-grid presentation for one selected display/device.
    ShowSensors {
        /// Optional typed display selector.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentTarget>,
    },
    /// Returns one readable sensor as a bounded text presentation.
    QuerySensor {
        /// Optional entity identifier; absence is allowed only when exactly one candidate exists.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity_id: Option<String>,
        /// Optional typed device selector that narrows the entity search.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentTarget>,
    },
    /// Builds one concrete media-navigation or volume command.
    Media {
        /// Bounded media operation from the existing command model.
        action: MediaIntentAction,
        /// Optional typed player selector.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentTarget>,
    },
    /// Builds a truthful `now_playing` view only from known observed playback state.
    ShowNowPlaying {
        /// Optional typed player/display selector.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentTarget>,
    },
    /// Requests a narrowly allowlisted entity action, always requiring confirmation.
    Actuator {
        /// Explicit actuator action from the v1 allowlist.
        action: ActuatorAction,
        /// Numeric set point only for `entity.set_value`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<f64>,
        /// Required explicit device/entity target.
        target: ActuatorTarget,
    },
}

/// Versioned schema envelope for storing or exchanging a typed intent without binding it to HTTP.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionedUserIntent {
    /// Intent-schema version; v1 accepts exactly `1`.
    pub version: u8,
    /// Bounded structured intent interpreted by this version.
    pub intent: UserIntent,
}

impl VersionedUserIntent {
    /// Validates the schema version and its contained intent.
    pub fn validate(&self) -> Result<(), IntentError> {
        if self.version != 1 {
            return Err(IntentError::invalid("version"));
        }
        self.intent.validate()
    }
}

impl UserIntent {
    /// Validates bounded values and action/value combinations before resolution.
    pub fn validate(&self) -> Result<(), IntentError> {
        match self {
            Self::PlayRadio { station_id, target } => {
                bounded_id(station_id, "station_id")?;
                validate_target(target.as_ref())
            }
            Self::ShowSensors { target } | Self::ShowNowPlaying { target } => {
                validate_target(target.as_ref())
            }
            Self::QuerySensor { entity_id, target } => {
                if let Some(entity_id) = entity_id {
                    bounded_id(entity_id, "entity_id")?;
                }
                validate_target(target.as_ref())
            }
            Self::Media { action, target } => {
                action.validate()?;
                validate_target(target.as_ref())
            }
            Self::Actuator {
                action,
                value,
                target,
            } => {
                bounded_id(&target.entity_id, "entity_id")?;
                match (action, value) {
                    (ActuatorAction::SetValue, Some(value)) if value.is_finite() => Ok(()),
                    (ActuatorAction::SetValue, _) | (_, Some(_)) => {
                        Err(IntentError::invalid("value"))
                    }
                    _ => Ok(()),
                }
            }
        }
    }
}

/// Explicit selectors available to a typed intent.
///
/// `area_id` is meaningful only if this single resolution call received a matching canonical
/// [`CanonicalArea`] context. Directory v1 itself has no home/area storage or name-based mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct IntentTarget {
    /// Stable device identity, preferred over all inferred target sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    /// Stable display surface identifier belonging to the selected device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    /// Canonical per-request area ID, never a display-name guess or wildcard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_id: Option<String>,
}

/// Explicit entity destination required by an actuator request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActuatorTarget {
    /// Stable target device identity.
    pub device_id: DeviceId,
    /// Stable declared entity identifier.
    pub entity_id: String,
}

/// Allowed media navigation and volume actions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaIntentAction {
    /// Starts playback.
    Play,
    /// Pauses playback.
    Pause,
    /// Stops playback.
    Stop,
    /// Moves to the next item.
    Next,
    /// Moves to the previous item.
    Previous,
    /// Sets a bounded absolute output level.
    SetVolume { level: u8 },
    /// Changes volume by a non-zero bounded delta.
    ChangeVolume { delta: i8 },
    /// Explicitly changes mute state.
    SetMute { muted: bool },
}

impl MediaIntentAction {
    /// Validates non-zero volume deltas.
    pub fn validate(&self) -> Result<(), IntentError> {
        match self {
            Self::ChangeVolume { delta } if *delta == 0 => Err(IntentError::invalid("delta")),
            _ => Ok(()),
        }
    }

    pub(super) fn command_body(&self) -> CommandBody {
        match self {
            Self::Play => CommandBody::Playback {
                action: "play".into(),
            },
            Self::Pause => CommandBody::Playback {
                action: "pause".into(),
            },
            Self::Stop => CommandBody::Playback {
                action: "stop".into(),
            },
            Self::Next => CommandBody::Playback {
                action: "next".into(),
            },
            Self::Previous => CommandBody::Playback {
                action: "previous".into(),
            },
            Self::SetVolume { level } => CommandBody::Volume {
                command: VolumeCommand::SetLevel { level: *level },
            },
            Self::ChangeVolume { delta } => CommandBody::Volume {
                command: VolumeCommand::Change { delta: *delta },
            },
            Self::SetMute { muted } => CommandBody::Volume {
                command: VolumeCommand::SetMute { muted: *muted },
            },
        }
    }
}

/// Server-derived actor information used for one resolution call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolutionActor {
    /// Account derived by the control authentication boundary.
    pub user_id: Uuid,
    /// Explicit server-issued grants; roles and capabilities do not substitute for these scopes.
    pub scopes: Vec<DeviceControlScope>,
}

/// Immutable input projection built only from the DC-010 account-owned directory state.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectoryProjection {
    /// Account that owns every candidate in this projection.
    pub owner_id: Uuid,
    /// Current owner-scoped devices, manifests, presence, and latest state.
    pub devices: Vec<DirectoryDevice>,
}

/// One account-owned directory candidate visible to the resolver.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectoryDevice {
    /// Public device identity and label.
    pub device: Device,
    /// Current static declaration.
    pub manifest: DeviceManifest,
    /// Whether the directory snapshot observed an active live generation.
    pub online: bool,
    /// Latest complete runtime state when available to the projection.
    pub runtime_state: Option<DeviceRuntimeState>,
    /// Latest entity states included only when the actor has `entity.state.read`.
    pub entity_states: Vec<EntityState>,
}

/// One ephemeral current-target input for the request that must never be persisted as a preference.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct CurrentTarget {
    /// Context device selected by the calling surface for this request only.
    pub device_id: Option<DeviceId>,
    /// Context display surface selected by the calling surface for this request only.
    pub surface_id: Option<String>,
}

/// Explicit canonical area mapping supplied for this call by a future owner-scoped source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalArea {
    /// Canonical area identifier, not a device label or inferred string.
    pub area_id: String,
    /// Exactly the devices assigned by that external canonical model.
    pub device_ids: Vec<DeviceId>,
}

/// Deterministic contextual inputs that are not taken from the untrusted intent.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ResolutionContext {
    /// Request-local target context; it is never written to storage.
    pub current_target: Option<CurrentTarget>,
    /// Optional explicit canonical mappings for this request only.
    pub canonical_areas: Vec<CanonicalArea>,
}

/// A complete non-executing resolution input.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolutionRequest {
    /// Server-authenticated actor.
    pub actor: ResolutionActor,
    /// Owner-scoped directory projection.
    pub directory: DirectoryProjection,
    /// Server-assigned command identity reused by every safe plan command in this request.
    pub command_id: CommandId,
    /// Server-observed receipt time used for command validation.
    pub received_at: crate::device_control::Timestamp,
    /// Request-local selector context.
    pub context: ResolutionContext,
}

/// Typed result of resolving one user intent without performing a side effect.
#[derive(Clone, Debug, PartialEq)]
pub enum ResolutionResult {
    /// A fully authorized command/presentation plan; the caller may submit commands to DC-009.
    Plan(IntentPlan),
    /// The request needs a deterministic user choice and did not create a command.
    Clarification(Clarification),
    /// A side-effecting actuator request is allowed only after explicit confirmation.
    Confirmation(Confirmation),
    /// The request is invalid, forbidden, unavailable, or unsupported and created no command.
    Error(IntentError),
}

/// Safe, executable-after-router-validation plan produced by the resolver.
#[derive(Clone, Debug, PartialEq)]
pub struct IntentPlan {
    /// Presentation generated from bounded canonical values.
    pub presentation: Option<Presentation>,
    /// Explicit commands compatible with DC-009 validation, never dispatched by this module.
    pub commands: Vec<crate::device_control::DeviceCommand>,
}

/// A safe selector question with no hidden broad fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Clarification {
    /// Stable reason code.
    pub code: ClarificationCode,
    /// Bounded candidate device IDs, not labels inferred from an area/name.
    pub device_ids: Vec<DeviceId>,
    /// Bounded candidate surface IDs when the device itself was unambiguous.
    pub surface_ids: Vec<String>,
}

/// Reasons why one explicit user selection is required.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClarificationCode {
    /// More than one compatible target exists.
    AmbiguousTarget,
    /// More than one compatible entity exists.
    AmbiguousEntity,
    /// More than one compatible display surface exists.
    AmbiguousSurface,
    /// A required target, entity, or surface was absent.
    MissingTarget,
}

/// Confirmation details for an allowed actuator proposal without an executable command.
#[derive(Clone, Debug, PartialEq)]
pub struct Confirmation {
    /// Exact target that must be shown to the user before confirmation.
    pub target: CommandTarget,
    /// Allowlisted action that would be submitted only after confirmation.
    pub action: ActuatorAction,
    /// Optional finite set point for `entity.set_value`.
    pub value: Option<f64>,
}

/// Safe deterministic resolver failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntentError {
    /// Stable non-enumerating or validation-safe error code.
    pub code: IntentErrorCode,
    /// Field name for malformed schema-valid input when safe to expose.
    pub field: Option<&'static str>,
}

impl IntentError {
    pub(super) fn invalid(field: &'static str) -> Self {
        Self {
            code: IntentErrorCode::InvalidIntent,
            field: Some(field),
        }
    }

    pub(super) fn code(code: IntentErrorCode) -> Self {
        Self { code, field: None }
    }
}

/// Stable resolver error codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntentErrorCode {
    /// Schema-valid intent violates an additional bounded invariant.
    InvalidIntent,
    /// Actor/directory ownership or required scope is not valid.
    Forbidden,
    /// The target is known but not live; no offline queue or fallback exists.
    TargetOffline,
    /// Required role, capability, entity, surface, or allowlist entry is absent.
    CapabilityNotSupported,
    /// State-dependent read has no permitted current entity state.
    StateUnavailable,
    /// The requested area has no supplied canonical mapping.
    UnsupportedSelector,
}

fn validate_target(target: Option<&IntentTarget>) -> Result<(), IntentError> {
    let Some(target) = target else {
        return Ok(());
    };
    if let Some(surface_id) = &target.surface_id {
        bounded_id(surface_id, "surface_id")?;
    }
    if let Some(area_id) = &target.area_id
        && (area_id.is_empty() || area_id.len() > MAX_AREA_ID)
    {
        return Err(IntentError::invalid("area_id"));
    }
    Ok(())
}

fn bounded_id(value: &str, field: &'static str) -> Result<(), IntentError> {
    if value.is_empty() || value.len() > MAX_INTENT_TEXT {
        Err(IntentError::invalid(field))
    } else {
        Ok(())
    }
}

impl From<ValidationError> for IntentError {
    fn from(_: ValidationError) -> Self {
        Self::code(IntentErrorCode::InvalidIntent)
    }
}

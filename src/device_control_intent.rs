//! Deterministic typed intent resolution and presentation planning for device control.
//!
//! This module accepts only schema-valid [`UserIntent`] values and a server-derived actor plus an
//! account-owned directory projection. It neither parses speech nor executes a command: callers
//! must pass a produced command through the DC-009 command router after confirmation where needed.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::device_control::{
    ActuatorAction, CommandBody, CommandId, CommandTarget, Device, DeviceCapability,
    DeviceControlScope, DeviceId, DeviceManifest, DeviceRole, DeviceRuntimeState, Entity,
    EntityDomain, EntityState, Freshness, PlaybackState, Presentation, Quality, SensorCard,
    Surface, SurfaceKind, Timestamp, ValidationError, ViewKind, VolumeCommand,
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

    fn command_body(&self) -> CommandBody {
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
    pub received_at: Timestamp,
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
    fn invalid(field: &'static str) -> Self {
        Self {
            code: IntentErrorCode::InvalidIntent,
            field: Some(field),
        }
    }

    fn code(code: IntentErrorCode) -> Self {
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

/// Resolves a schema-valid user intent into a typed plan, clarification, confirmation, or error.
///
/// Ordering is fixed: validate intent, verify owner and scopes, resolve explicit IDs, apply the
/// request-local current target, use a supplied canonical area, then accept exactly one compatible
/// candidate. It never broadcasts, guesses names, persists a preference, or calls a provider.
pub fn resolve(request: &ResolutionRequest, intent: &UserIntent) -> ResolutionResult {
    if let Err(error) = intent.validate() {
        return ResolutionResult::Error(error);
    }
    if request.actor.user_id != request.directory.owner_id {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::Forbidden));
    }
    match intent {
        UserIntent::PlayRadio { station_id, target } => {
            resolve_radio(request, station_id, target.as_ref())
        }
        UserIntent::ShowSensors { target } => resolve_sensors(request, target.as_ref()),
        UserIntent::QuerySensor { entity_id, target } => {
            resolve_sensor_query(request, entity_id.as_deref(), target.as_ref())
        }
        UserIntent::Media { action, target } => resolve_media(request, action, target.as_ref()),
        UserIntent::ShowNowPlaying { target } => resolve_now_playing(request, target.as_ref()),
        UserIntent::Actuator {
            action,
            value,
            target,
        } => resolve_actuator(request, action, *value, target),
    }
}

fn resolve_radio(
    request: &ResolutionRequest,
    station_id: &str,
    target: Option<&IntentTarget>,
) -> ResolutionResult {
    if let Err(error) = require_scope(request, DeviceControlScope::MediaControl) {
        return ResolutionResult::Error(error);
    }
    let devices = compatible_devices(request, target, |device| {
        device.online
            && has_role(&device.manifest, DeviceRole::Player)
            && has_capability(&device.manifest, |capability| {
                matches!(capability, DeviceCapability::Station { .. })
            })
    });
    let device = match select_device(request, target, devices) {
        Ok(device) => device,
        Err(result) => return result,
    };
    ResolutionResult::Plan(command_plan(
        request,
        device,
        None,
        None,
        CommandBody::PlayStation {
            station_id: station_id.into(),
        },
    ))
}

fn resolve_media(
    request: &ResolutionRequest,
    action: &MediaIntentAction,
    target: Option<&IntentTarget>,
) -> ResolutionResult {
    if let Err(error) = require_scope(request, DeviceControlScope::MediaControl) {
        return ResolutionResult::Error(error);
    }
    let body = action.command_body();
    let devices = compatible_devices(request, target, |device| {
        device.online && media_supported(&device.manifest, &body)
    });
    let device = match select_device(request, target, devices) {
        Ok(device) => device,
        Err(result) => return result,
    };
    ResolutionResult::Plan(command_plan(request, device, None, None, body))
}

fn resolve_sensors(request: &ResolutionRequest, target: Option<&IntentTarget>) -> ResolutionResult {
    if let Err(error) = require_scope(request, DeviceControlScope::EntityStateRead)
        .and_then(|_| require_scope(request, DeviceControlScope::DisplayControl))
    {
        return ResolutionResult::Error(error);
    }
    let devices = compatible_devices(request, target, |device| {
        device.online
            && readable_sensor_count(device) > 0
            && display_capable(&device.manifest, ViewKind::SensorGrid)
    });
    let device = match select_device(request, target, devices) {
        Ok(device) => device,
        Err(result) => return result,
    };
    let surface = match select_surface(request, device, target, ViewKind::SensorGrid) {
        Ok(surface) => surface,
        Err(result) => return result,
    };
    let cards = device
        .manifest
        .entities
        .iter()
        .filter(|entity| entity.domain == EntityDomain::Sensor && entity.readable)
        .map(|entity| {
            sensor_card(
                entity,
                state_for(device, &entity.entity_id),
                &request.received_at,
            )
        })
        .collect::<Vec<_>>();
    let presentation = Presentation::SensorGrid {
        title: "Sensors".into(),
        items: cards,
    };
    if presentation.validate().is_err() || !presentation_supported(&device.manifest, &presentation)
    {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::CapabilityNotSupported));
    }
    ResolutionResult::Plan(
        command_plan(
            request,
            device,
            None,
            Some(surface),
            CommandBody::Display {
                presentation: presentation.clone(),
            },
        )
        .with_presentation(presentation),
    )
}

fn resolve_sensor_query(
    request: &ResolutionRequest,
    entity_id: Option<&str>,
    target: Option<&IntentTarget>,
) -> ResolutionResult {
    if let Err(error) = require_scope(request, DeviceControlScope::EntityStateRead) {
        return ResolutionResult::Error(error);
    }
    let device_candidates = compatible_devices(request, target, |device| {
        device.online && readable_sensor_count(device) > 0
    });
    let mut candidates = Vec::new();
    for device in device_candidates {
        for entity in &device.manifest.entities {
            if entity.domain == EntityDomain::Sensor
                && entity.readable
                && entity_id.is_none_or(|id| entity.entity_id == id)
            {
                candidates.push((device, entity));
            }
        }
    }
    if candidates.is_empty() {
        return ResolutionResult::Clarification(Clarification {
            code: ClarificationCode::MissingTarget,
            device_ids: Vec::new(),
            surface_ids: Vec::new(),
        });
    }
    if candidates.len() != 1 {
        return ResolutionResult::Clarification(Clarification {
            code: ClarificationCode::AmbiguousEntity,
            device_ids: unique_device_ids(
                candidates.iter().map(|(device, _)| device.device.device_id),
            ),
            surface_ids: Vec::new(),
        });
    }
    let (device, entity) = candidates[0];
    let Some(state) = state_for(device, &entity.entity_id) else {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::StateUnavailable));
    };
    let (value, quality, freshness) = sensor_value(state, &request.received_at);
    let text = format_sensor_text(entity, &value, state.unit.as_deref(), &quality, &freshness);
    let presentation = Presentation::Text { text };
    if presentation.validate().is_err() {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::CapabilityNotSupported));
    }
    ResolutionResult::Plan(IntentPlan {
        presentation: Some(presentation),
        commands: Vec::new(),
    })
}

fn resolve_now_playing(
    request: &ResolutionRequest,
    target: Option<&IntentTarget>,
) -> ResolutionResult {
    if let Err(error) = require_scope(request, DeviceControlScope::DisplayControl) {
        return ResolutionResult::Error(error);
    }
    let devices = compatible_devices(request, target, |device| {
        device.online
            && display_capable(&device.manifest, ViewKind::NowPlaying)
            && has_role(&device.manifest, DeviceRole::Player)
    });
    let device = match select_device(request, target, devices) {
        Ok(device) => device,
        Err(result) => return result,
    };
    let surface = match select_surface(request, device, target, ViewKind::NowPlaying) {
        Ok(surface) => surface,
        Err(result) => return result,
    };
    let Some(PlaybackState {
        status,
        station_id: Some(station_id),
    }) = device
        .runtime_state
        .as_ref()
        .and_then(|state| state.playback.as_ref())
    else {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::StateUnavailable));
    };
    if status != "playing" {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::StateUnavailable));
    }
    // v1 state has no catalog title field; echoing the observed stable station ID is truthful.
    let presentation = Presentation::NowPlaying {
        station_id: station_id.clone(),
        title: station_id.clone(),
        subtitle: None,
    };
    if presentation.validate().is_err() || !presentation_supported(&device.manifest, &presentation)
    {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::CapabilityNotSupported));
    }
    ResolutionResult::Plan(
        command_plan(
            request,
            device,
            None,
            Some(surface),
            CommandBody::Display {
                presentation: presentation.clone(),
            },
        )
        .with_presentation(presentation),
    )
}

fn resolve_actuator(
    request: &ResolutionRequest,
    action: &ActuatorAction,
    value: Option<f64>,
    target: &ActuatorTarget,
) -> ResolutionResult {
    if let Err(error) = require_scope(request, DeviceControlScope::ActuatorControl) {
        return ResolutionResult::Error(error);
    }
    let device_id = target.device_id;
    let Some(device) = request
        .directory
        .devices
        .iter()
        .find(|device| device.device.device_id == device_id)
    else {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::Forbidden));
    };
    if !device.online {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::TargetOffline));
    }
    let Some(entity) = device
        .manifest
        .entities
        .iter()
        .find(|entity| entity.entity_id == target.entity_id)
    else {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::CapabilityNotSupported));
    };
    let allowed = entity.controllable
        && entity.allowed_commands.contains(action)
        && has_role(&device.manifest, DeviceRole::Actuator)
        && has_capability(
            &device.manifest,
            |capability| matches!(capability, DeviceCapability::Actuator { commands } if commands.contains(action)),
        );
    if !allowed || !safe_actuator_entity(entity) {
        return ResolutionResult::Error(IntentError::code(IntentErrorCode::CapabilityNotSupported));
    }
    if matches!(action, ActuatorAction::SetValue)
        && (entity
            .minimum
            .is_some_and(|minimum| value.unwrap_or_default() < minimum)
            || entity
                .maximum
                .is_some_and(|maximum| value.unwrap_or_default() > maximum))
    {
        return ResolutionResult::Error(IntentError::invalid("value"));
    }
    ResolutionResult::Confirmation(Confirmation {
        target: CommandTarget {
            device_id,
            entity_id: Some(target.entity_id.clone()),
            surface_id: None,
        },
        action: action.clone(),
        value,
    })
}

// v1 intentionally has no lock/climate actuator vocabulary; a permissive manifest cannot widen it.
fn safe_actuator_entity(entity: &Entity) -> bool {
    matches!(entity.domain, EntityDomain::Switch | EntityDomain::Light)
        && !matches!(entity.device_class.as_str(), "lock" | "climate")
}

fn compatible_devices<'a>(
    request: &'a ResolutionRequest,
    target: Option<&IntentTarget>,
    predicate: impl Fn(&DirectoryDevice) -> bool,
) -> Vec<&'a DirectoryDevice> {
    let area_ids = target
        .and_then(|target| target.area_id.as_deref())
        .and_then(|area_id| canonical_area_devices(request, area_id));
    request
        .directory
        .devices
        .iter()
        .filter(|device| predicate(device))
        .filter(|device| {
            area_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&device.device.device_id))
        })
        .collect()
}

fn select_device<'a>(
    request: &'a ResolutionRequest,
    target: Option<&IntentTarget>,
    candidates: Vec<&'a DirectoryDevice>,
) -> Result<&'a DirectoryDevice, ResolutionResult> {
    if target
        .and_then(|target| target.area_id.as_deref())
        .is_some()
        && target
            .and_then(|target| target.area_id.as_deref())
            .and_then(|area_id| canonical_area_devices(request, area_id))
            .is_none()
    {
        return Err(ResolutionResult::Error(IntentError::code(
            IntentErrorCode::UnsupportedSelector,
        )));
    }
    if let Some(device_id) = target.and_then(|target| target.device_id) {
        return choose_exact(request, candidates, device_id);
    }
    if let Some(surface_id) = target.and_then(|target| target.surface_id.as_deref()) {
        let surface_candidates = candidates
            .into_iter()
            .filter(|device| {
                device
                    .manifest
                    .surfaces
                    .iter()
                    .any(|surface| surface.surface_id == surface_id)
            })
            .collect::<Vec<_>>();
        return choose_one(surface_candidates, ClarificationCode::AmbiguousTarget);
    }
    if let Some(device_id) = request
        .context
        .current_target
        .as_ref()
        .and_then(|target| target.device_id)
    {
        return choose_exact(request, candidates, device_id);
    }
    choose_one(candidates, ClarificationCode::AmbiguousTarget)
}

fn choose_exact<'a>(
    request: &'a ResolutionRequest,
    candidates: Vec<&'a DirectoryDevice>,
    device_id: DeviceId,
) -> Result<&'a DirectoryDevice, ResolutionResult> {
    let found = candidates
        .into_iter()
        .filter(|device| device.device.device_id == device_id)
        .collect::<Vec<_>>();
    if found.is_empty() {
        let code = request
            .directory
            .devices
            .iter()
            .find(|device| device.device.device_id == device_id)
            .filter(|device| !device.online)
            .map(|_| IntentErrorCode::TargetOffline)
            .unwrap_or(IntentErrorCode::CapabilityNotSupported);
        Err(ResolutionResult::Error(IntentError::code(code)))
    } else {
        choose_one(found, ClarificationCode::AmbiguousTarget)
    }
}

fn choose_one(
    candidates: Vec<&DirectoryDevice>,
    code: ClarificationCode,
) -> Result<&DirectoryDevice, ResolutionResult> {
    match candidates.as_slice() {
        [device] => Ok(device),
        [] => Err(ResolutionResult::Clarification(Clarification {
            code: ClarificationCode::MissingTarget,
            device_ids: Vec::new(),
            surface_ids: Vec::new(),
        })),
        _ => Err(ResolutionResult::Clarification(Clarification {
            code,
            device_ids: unique_device_ids(candidates.iter().map(|device| device.device.device_id)),
            surface_ids: Vec::new(),
        })),
    }
}

fn select_surface<'a>(
    request: &ResolutionRequest,
    device: &'a DirectoryDevice,
    target: Option<&IntentTarget>,
    view: ViewKind,
) -> Result<&'a Surface, ResolutionResult> {
    let explicit = target.and_then(|target| target.surface_id.as_deref());
    let current = request
        .context
        .current_target
        .as_ref()
        .filter(|current| current.device_id == Some(device.device.device_id))
        .and_then(|current| current.surface_id.as_deref());
    let requested = explicit.or(current);
    let candidates = device
        .manifest
        .surfaces
        .iter()
        .filter(|surface| surface.kind == SurfaceKind::Display && surface.views.contains(&view))
        .filter(|surface| requested.is_none_or(|surface_id| surface.surface_id == surface_id))
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [surface] => Ok(surface),
        [] => Err(ResolutionResult::Error(IntentError::code(
            IntentErrorCode::CapabilityNotSupported,
        ))),
        _ => Err(ResolutionResult::Clarification(Clarification {
            code: ClarificationCode::AmbiguousSurface,
            device_ids: vec![device.device.device_id],
            surface_ids: candidates
                .iter()
                .map(|surface| surface.surface_id.clone())
                .collect(),
        })),
    }
}

fn command_plan(
    request: &ResolutionRequest,
    device: &DirectoryDevice,
    entity_id: Option<String>,
    surface: Option<&Surface>,
    body: CommandBody,
) -> IntentPlan {
    IntentPlan {
        presentation: None,
        commands: vec![crate::device_control::DeviceCommand {
            command_id: request.command_id,
            target: CommandTarget {
                device_id: device.device.device_id,
                entity_id,
                surface_id: surface.map(|surface| surface.surface_id.clone()),
            },
            deadline_at: None,
            body,
        }],
    }
}

trait WithPresentation {
    fn with_presentation(self, presentation: Presentation) -> Self;
}

impl WithPresentation for IntentPlan {
    fn with_presentation(mut self, presentation: Presentation) -> Self {
        self.presentation = Some(presentation);
        self
    }
}

fn sensor_card(entity: &Entity, state: Option<&EntityState>, now: &Timestamp) -> SensorCard {
    let Some(state) = state else {
        return SensorCard {
            entity_id: entity.entity_id.clone(),
            label: entity.label.clone(),
            value: Value::Null,
            unit: entity.unit.clone(),
            quality: Quality::Unavailable,
            freshness: Freshness::Unknown,
        };
    };
    let (value, quality, freshness) = sensor_value(state, now);
    SensorCard {
        entity_id: entity.entity_id.clone(),
        label: entity.label.clone(),
        value,
        unit: state.unit.clone().or_else(|| entity.unit.clone()),
        quality,
        freshness,
    }
}

fn sensor_value(state: &EntityState, now: &Timestamp) -> (Value, Quality, Freshness) {
    let freshness = state.freshness_at(now);
    if state.quality == Quality::Unavailable {
        // An unavailable observation has no current value even when its transport timestamp is new.
        (Value::Null, Quality::Unavailable, Freshness::Unknown)
    } else {
        (state.value.clone(), state.quality.clone(), freshness)
    }
}

fn format_sensor_text(
    entity: &Entity,
    value: &Value,
    unit: Option<&str>,
    quality: &Quality,
    freshness: &Freshness,
) -> String {
    match value {
        Value::Null => format!("{}: unavailable ({quality:?}, {freshness:?})", entity.label),
        Value::String(value) => format!(
            "{}: {value}{} ({quality:?}, {freshness:?})",
            entity.label,
            unit.map(|unit| format!(" {unit}")).unwrap_or_default()
        ),
        value => format!(
            "{}: {value}{} ({quality:?}, {freshness:?})",
            entity.label,
            unit.map(|unit| format!(" {unit}")).unwrap_or_default()
        ),
    }
}

fn state_for<'a>(device: &'a DirectoryDevice, entity_id: &str) -> Option<&'a EntityState> {
    device
        .entity_states
        .iter()
        .find(|state| state.entity_id == entity_id)
}

fn readable_sensor_count(device: &DirectoryDevice) -> usize {
    device
        .manifest
        .entities
        .iter()
        .filter(|entity| entity.domain == EntityDomain::Sensor && entity.readable)
        .count()
}

fn media_supported(manifest: &DeviceManifest, body: &CommandBody) -> bool {
    has_role(manifest, DeviceRole::Player)
        && match body {
            CommandBody::Playback { action } => has_capability(
                manifest,
                |capability| matches!(capability, DeviceCapability::Playback { actions } if actions.contains(action)),
            ),
            CommandBody::Volume { command } => has_capability(manifest, |capability| {
                matches!(capability, DeviceCapability::Volume { mute, .. } if match command {
                    VolumeCommand::SetMute { .. } => *mute,
                    VolumeCommand::SetLevel { .. } | VolumeCommand::Change { .. } => true,
                })
            }),
            _ => false,
        }
}

fn display_capable(manifest: &DeviceManifest, view: ViewKind) -> bool {
    has_role(manifest, DeviceRole::DisplaySurface)
        && has_capability(
            manifest,
            |capability| matches!(capability, DeviceCapability::Display { views, .. } if views.contains(&view)),
        )
}

/// Checks the target's declared presentation limits before a plan reaches the command router.
fn presentation_supported(manifest: &DeviceManifest, presentation: &Presentation) -> bool {
    has_capability(manifest, |capability| {
        matches!(capability, DeviceCapability::Display { views, max_items, max_text_length } if match presentation {
            Presentation::Text { text } => text.len() <= usize::from(*max_text_length),
            Presentation::NowPlaying { .. } => views.contains(&ViewKind::NowPlaying),
            Presentation::SensorGrid { items, .. } => views.contains(&ViewKind::SensorGrid) && items.len() <= usize::from(*max_items),
        })
    })
}

fn has_role(manifest: &DeviceManifest, role: DeviceRole) -> bool {
    manifest.roles.contains(&role)
}

fn has_capability(
    manifest: &DeviceManifest,
    predicate: impl Fn(&DeviceCapability) -> bool,
) -> bool {
    manifest.capabilities.items.iter().any(predicate)
}

fn require_scope(
    request: &ResolutionRequest,
    scope: DeviceControlScope,
) -> Result<(), IntentError> {
    request
        .actor
        .scopes
        .contains(&scope)
        .then_some(())
        .ok_or_else(|| IntentError::code(IntentErrorCode::Forbidden))
}

fn canonical_area_devices(request: &ResolutionRequest, area_id: &str) -> Option<Vec<DeviceId>> {
    request
        .context
        .canonical_areas
        .iter()
        .find(|area| area.area_id == area_id)
        .map(|area| area.device_ids.clone())
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

fn unique_device_ids(ids: impl Iterator<Item = DeviceId>) -> Vec<DeviceId> {
    let mut result = Vec::new();
    for id in ids {
        if !result.contains(&id) {
            result.push(id);
        }
    }
    result
}

impl From<ValidationError> for IntentError {
    fn from(_: ValidationError) -> Self {
        Self::code(IntentErrorCode::InvalidIntent)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::device_control::DeviceCapabilities;

    fn timestamp() -> Timestamp {
        Timestamp::parse("2026-09-03T12:00:00Z").unwrap()
    }

    fn actor(user_id: Uuid) -> ResolutionActor {
        ResolutionActor {
            user_id,
            scopes: vec![
                DeviceControlScope::DirectoryRead,
                DeviceControlScope::EntityStateRead,
                DeviceControlScope::MediaControl,
                DeviceControlScope::DisplayControl,
                DeviceControlScope::ActuatorControl,
            ],
        }
    }

    fn sensor(id: &str, class: &str, label: &str) -> Entity {
        Entity {
            entity_id: id.into(),
            domain: EntityDomain::Sensor,
            device_class: class.into(),
            label: label.into(),
            readable: true,
            controllable: false,
            unit: Some(if class == "temperature" {
                "°C".into()
            } else {
                "%".into()
            }),
            stale_after_seconds: 60,
            allowed_commands: Vec::new(),
            minimum: None,
            maximum: None,
            step: None,
        }
    }

    fn device(id: DeviceId, online: bool, sensor_count: usize) -> DirectoryDevice {
        let mut entities = Vec::new();
        if sensor_count > 0 {
            entities.push(sensor("sensor.temperature", "temperature", "Temperature"));
        }
        if sensor_count > 1 {
            entities.push(sensor("sensor.humidity", "humidity", "Humidity"));
        }
        let states = entities
            .iter()
            .enumerate()
            .map(|(index, entity)| EntityState {
                entity_id: entity.entity_id.clone(),
                entity_revision: 1,
                value: if index == 0 { json!(21.5) } else { json!(48) },
                unit: entity.unit.clone(),
                quality: Quality::Ok,
                freshness: Some(Freshness::Fresh),
                observed_at: timestamp(),
                received_at: Some(timestamp()),
                stale_after: Timestamp::parse("2026-09-03T12:01:00Z").unwrap(),
            })
            .collect();
        DirectoryDevice {
            device: Device {
                device_id: id,
                display_name: "Device".into(),
                device_type: "test".into(),
            },
            manifest: DeviceManifest {
                manifest_revision: 1,
                roles: vec![
                    DeviceRole::Player,
                    DeviceRole::DisplaySurface,
                    DeviceRole::SensorSource,
                    DeviceRole::Actuator,
                ],
                capabilities: DeviceCapabilities {
                    revision: 1,
                    items: vec![
                        DeviceCapability::Playback {
                            actions: vec![
                                "play".into(),
                                "pause".into(),
                                "stop".into(),
                                "next".into(),
                                "previous".into(),
                            ],
                        },
                        DeviceCapability::Station {
                            sources: vec!["rockserver_catalog".into()],
                        },
                        DeviceCapability::Volume {
                            step: 1,
                            mute: true,
                        },
                        DeviceCapability::Display {
                            views: vec![ViewKind::Text, ViewKind::NowPlaying, ViewKind::SensorGrid],
                            max_items: 8,
                            max_text_length: 128,
                        },
                        DeviceCapability::Actuator {
                            commands: vec![ActuatorAction::TurnOn],
                        },
                    ],
                },
                entities,
                surfaces: vec![Surface {
                    surface_id: "display.main".into(),
                    kind: SurfaceKind::Display,
                    label: "Main".into(),
                    views: vec![ViewKind::Text, ViewKind::NowPlaying, ViewKind::SensorGrid],
                }],
            },
            online,
            runtime_state: Some(DeviceRuntimeState {
                playback: Some(PlaybackState {
                    status: "playing".into(),
                    station_id: Some("station.rock".into()),
                }),
                volume: None,
                display: None,
            }),
            entity_states: states,
        }
    }

    fn request(devices: Vec<DirectoryDevice>) -> ResolutionRequest {
        let user = Uuid::new_v4();
        ResolutionRequest {
            actor: actor(user),
            directory: DirectoryProjection {
                owner_id: user,
                devices,
            },
            command_id: CommandId(Uuid::new_v4()),
            received_at: timestamp(),
            context: ResolutionContext::default(),
        }
    }

    #[test]
    fn show_sensors_builds_a_fresh_sensor_grid_for_explicit_target_and_surface() {
        let id = DeviceId(Uuid::new_v4());
        let result = resolve(
            &request(vec![device(id, true, 2)]),
            &UserIntent::ShowSensors {
                target: Some(IntentTarget {
                    device_id: Some(id),
                    surface_id: Some("display.main".into()),
                    area_id: None,
                }),
            },
        );
        let ResolutionResult::Plan(plan) = result else {
            panic!("expected plan");
        };
        let Some(Presentation::SensorGrid { items, .. }) = plan.presentation else {
            panic!("expected grid");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].value, json!(21.5));
        assert_eq!(items[0].unit.as_deref(), Some("°C"));
        assert_eq!(items[0].freshness, Freshness::Fresh);
        assert_eq!(
            plan.commands[0].target.surface_id.as_deref(),
            Some("display.main")
        );
    }

    #[test]
    fn target_ambiguity_never_broadcasts() {
        let result = resolve(
            &request(vec![
                device(DeviceId(Uuid::new_v4()), true, 1),
                device(DeviceId(Uuid::new_v4()), true, 1),
            ]),
            &UserIntent::ShowSensors { target: None },
        );
        assert!(matches!(
            result,
            ResolutionResult::Clarification(Clarification {
                code: ClarificationCode::AmbiguousTarget,
                ..
            })
        ));
    }

    #[test]
    fn explicit_target_and_request_local_current_target_win_deterministically() {
        let first = DeviceId(Uuid::new_v4());
        let second = DeviceId(Uuid::new_v4());
        let explicit = resolve(
            &request(vec![device(first, true, 1), device(second, true, 1)]),
            &UserIntent::ShowSensors {
                target: Some(IntentTarget {
                    device_id: Some(second),
                    surface_id: Some("display.main".into()),
                    area_id: None,
                }),
            },
        );
        let ResolutionResult::Plan(explicit) = explicit else {
            panic!("expected explicit plan");
        };
        assert_eq!(explicit.commands[0].target.device_id, second);

        let mut contextual = request(vec![device(first, true, 1), device(second, true, 1)]);
        contextual.context.current_target = Some(CurrentTarget {
            device_id: Some(first),
            surface_id: Some("display.main".into()),
        });
        let current = resolve(&contextual, &UserIntent::ShowSensors { target: None });
        let ResolutionResult::Plan(current) = current else {
            panic!("expected current target plan");
        };
        assert_eq!(current.commands[0].target.device_id, first);
    }

    #[test]
    fn stale_missing_and_unavailable_values_are_never_zero_or_current() {
        let id = DeviceId(Uuid::new_v4());
        let mut device = device(id, true, 2);
        device.entity_states[0].stale_after = Timestamp::parse("2026-09-03T11:00:00Z").unwrap();
        device.entity_states[1].quality = Quality::Unavailable;
        let result = resolve(
            &request(vec![device]),
            &UserIntent::ShowSensors {
                target: Some(IntentTarget {
                    device_id: Some(id),
                    ..IntentTarget::default()
                }),
            },
        );
        let ResolutionResult::Plan(plan) = result else {
            panic!("expected plan");
        };
        let Some(Presentation::SensorGrid { items, .. }) = plan.presentation else {
            panic!("expected grid");
        };
        assert_eq!(items[0].freshness, Freshness::Stale);
        assert_eq!(items[1].value, Value::Null);
        assert_eq!(items[1].quality, Quality::Unavailable);
    }

    #[test]
    fn query_sensor_requires_one_readable_entity_and_state_scope() {
        let id = DeviceId(Uuid::new_v4());
        let mut request = request(vec![device(id, true, 2)]);
        let ambiguous = resolve(
            &request,
            &UserIntent::QuerySensor {
                entity_id: None,
                target: Some(IntentTarget {
                    device_id: Some(id),
                    ..IntentTarget::default()
                }),
            },
        );
        assert!(matches!(
            ambiguous,
            ResolutionResult::Clarification(Clarification {
                code: ClarificationCode::AmbiguousEntity,
                ..
            })
        ));
        request
            .actor
            .scopes
            .retain(|scope| *scope != DeviceControlScope::EntityStateRead);
        let forbidden = resolve(
            &request,
            &UserIntent::QuerySensor {
                entity_id: Some("sensor.temperature".into()),
                target: Some(IntentTarget {
                    device_id: Some(id),
                    ..IntentTarget::default()
                }),
            },
        );
        assert!(matches!(
            forbidden,
            ResolutionResult::Error(IntentError {
                code: IntentErrorCode::Forbidden,
                ..
            })
        ));
    }

    #[test]
    fn media_and_now_playing_require_live_supported_targets() {
        let id = DeviceId(Uuid::new_v4());
        let mut offline = device(id, false, 1);
        let media = resolve(
            &request(vec![offline.clone()]),
            &UserIntent::Media {
                action: MediaIntentAction::Next,
                target: Some(IntentTarget {
                    device_id: Some(id),
                    ..IntentTarget::default()
                }),
            },
        );
        assert!(matches!(
            media,
            ResolutionResult::Clarification(_) | ResolutionResult::Error(_)
        ));
        offline.online = true;
        offline
            .manifest
            .capabilities
            .items
            .retain(|capability| !matches!(capability, DeviceCapability::Display { .. }));
        let now_playing = resolve(
            &request(vec![offline]),
            &UserIntent::ShowNowPlaying {
                target: Some(IntentTarget {
                    device_id: Some(id),
                    ..IntentTarget::default()
                }),
            },
        );
        assert!(matches!(
            now_playing,
            ResolutionResult::Clarification(_) | ResolutionResult::Error(_)
        ));
    }

    #[test]
    fn actuator_is_explicit_allowlisted_and_confirmation_only() {
        let id = DeviceId(Uuid::new_v4());
        let mut controlled = device(id, true, 1);
        controlled.manifest.entities.push(Entity {
            entity_id: "switch.safe".into(),
            domain: EntityDomain::Switch,
            device_class: "switch".into(),
            label: "Safe switch".into(),
            readable: true,
            controllable: true,
            unit: None,
            stale_after_seconds: 60,
            allowed_commands: vec![ActuatorAction::TurnOn],
            minimum: None,
            maximum: None,
            step: None,
        });
        let result = resolve(
            &request(vec![controlled]),
            &UserIntent::Actuator {
                action: ActuatorAction::TurnOn,
                value: None,
                target: ActuatorTarget {
                    device_id: id,
                    entity_id: "switch.safe".into(),
                },
            },
        );
        assert!(matches!(
            result,
            ResolutionResult::Confirmation(Confirmation { .. })
        ));
        let mut high_risk = device(id, true, 1);
        high_risk.manifest.entities.push(Entity {
            entity_id: "switch.lock_like".into(),
            domain: EntityDomain::Switch,
            device_class: "lock".into(),
            label: "Lock".into(),
            readable: true,
            controllable: true,
            unit: None,
            stale_after_seconds: 60,
            allowed_commands: vec![ActuatorAction::TurnOn],
            minimum: None,
            maximum: None,
            step: None,
        });
        let denied = resolve(
            &request(vec![high_risk]),
            &UserIntent::Actuator {
                action: ActuatorAction::TurnOn,
                value: None,
                target: ActuatorTarget {
                    device_id: id,
                    entity_id: "switch.lock_like".into(),
                },
            },
        );
        assert!(matches!(
            denied,
            ResolutionResult::Error(IntentError {
                code: IntentErrorCode::CapabilityNotSupported,
                ..
            })
        ));
        let malformed = serde_json::from_value::<UserIntent>(
            json!({"kind":"media","action":"play","unknown":true}),
        );
        assert!(malformed.is_err());
        let unknown = serde_json::from_value::<UserIntent>(json!({"kind":"provider_call"}));
        assert!(unknown.is_err());
    }

    #[test]
    fn cross_owner_area_and_current_target_follow_explicit_policy() {
        let id = DeviceId(Uuid::new_v4());
        let mut cross_owner_request = request(vec![device(id, true, 1)]);
        cross_owner_request.directory.owner_id = Uuid::new_v4();
        let cross_owner = resolve(
            &cross_owner_request,
            &UserIntent::ShowSensors {
                target: Some(IntentTarget {
                    device_id: Some(id),
                    ..IntentTarget::default()
                }),
            },
        );
        assert!(matches!(
            cross_owner,
            ResolutionResult::Error(IntentError {
                code: IntentErrorCode::Forbidden,
                ..
            })
        ));
        let request = request(vec![device(id, true, 1)]);
        let unsupported_area = resolve(
            &request,
            &UserIntent::ShowSensors {
                target: Some(IntentTarget {
                    area_id: Some("kitchen".into()),
                    ..IntentTarget::default()
                }),
            },
        );
        assert!(matches!(
            unsupported_area,
            ResolutionResult::Error(IntentError {
                code: IntentErrorCode::UnsupportedSelector,
                ..
            })
        ));
    }

    #[test]
    fn produced_command_passes_protocol_validation_and_payload_is_bounded() {
        let id = DeviceId(Uuid::new_v4());
        let media_request = request(vec![device(id, true, 1)]);
        let result = resolve(
            &media_request,
            &UserIntent::Media {
                action: MediaIntentAction::SetMute { muted: true },
                target: Some(IntentTarget {
                    device_id: Some(id),
                    ..IntentTarget::default()
                }),
            },
        );
        let ResolutionResult::Plan(plan) = result else {
            panic!("expected plan");
        };
        assert!(
            plan.commands[0]
                .validate_at(&media_request.received_at)
                .is_ok()
        );
        let oversized = UserIntent::PlayRadio {
            station_id: "x".repeat(129),
            target: None,
        };
        assert!(matches!(
            resolve(&media_request, &oversized),
            ResolutionResult::Error(IntentError {
                code: IntentErrorCode::InvalidIntent,
                ..
            })
        ));

        let mut too_small = device(id, true, 2);
        for capability in &mut too_small.manifest.capabilities.items {
            if let DeviceCapability::Display { max_items, .. } = capability {
                *max_items = 1;
            }
        }
        let rejected = resolve(
            &request(vec![too_small]),
            &UserIntent::ShowSensors {
                target: Some(IntentTarget {
                    device_id: Some(id),
                    ..IntentTarget::default()
                }),
            },
        );
        assert!(matches!(
            rejected,
            ResolutionResult::Error(IntentError {
                code: IntentErrorCode::CapabilityNotSupported,
                ..
            })
        ));
    }
}

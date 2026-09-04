//! Deterministic typed intent resolution and presentation planning for device control.
//!
//! This module accepts only schema-valid [`UserIntent`] values and a server-derived actor plus an
//! account-owned directory projection. It neither parses speech nor executes a command: callers
//! must pass a produced command through the DC-009 command router after confirmation where needed.

use crate::device_control::{
    ActuatorAction, CommandBody, CommandTarget, DeviceCapability, DeviceControlScope, DeviceId,
    DeviceManifest, DeviceRole, Entity, EntityDomain, EntityState, Freshness, PlaybackState,
    Presentation, Quality, SensorCard, Surface, SurfaceKind, Timestamp, ViewKind, VolumeCommand,
};
use serde_json::Value;

mod model;

pub use model::{
    ActuatorTarget, CanonicalArea, Clarification, ClarificationCode, Confirmation, CurrentTarget,
    DirectoryDevice, DirectoryProjection, IntentError, IntentErrorCode, IntentPlan, IntentTarget,
    MediaIntentAction, ResolutionActor, ResolutionContext, ResolutionRequest, ResolutionResult,
    UserIntent, VersionedUserIntent,
};

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

fn unique_device_ids(ids: impl Iterator<Item = DeviceId>) -> Vec<DeviceId> {
    let mut result = Vec::new();
    for id in ids {
        if !result.contains(&id) {
            result.push(id);
        }
    }
    result
}

#[cfg(test)]
#[path = "device_control_intent/tests.rs"]
mod tests;

//! Focused regression tests for this private domain facade.

use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::device_control::{CommandId, Device, DeviceCapabilities, DeviceRuntimeState};

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

//! Bounded, owner-scoped command admission and live routing for protocol v1.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    device_control::{
        ActuatorAction, CommandAccepted, CommandBody, CommandId, CommandReceived,
        CommandReservation, CommandResult, CommandStatus, DeviceCapability, DeviceCommand,
        DeviceControlScope, DeviceControlStore, DeviceId, DeviceManifest, DeviceRole, DomainError,
        StoreOutcome, Timestamp,
    },
    device_control_presence::{ConnectionRegistry, OutboundFrame},
};

const MAX_CONNECTION_IN_FLIGHT: usize = 16;
const MAX_TARGET_IN_FLIGHT: usize = 8;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TIMEOUT: Duration = Duration::from_secs(30);

/// Server-side command router shared by all authenticated control WebSocket sessions.
#[derive(Clone, Default)]
pub struct CommandRouter {
    state: Arc<Mutex<HashMap<CommandId, InFlight>>>,
}

#[derive(Clone)]
struct InFlight {
    owner_id: Uuid,
    controller_device_id: Uuid,
    controller_connection_id: Uuid,
    target_device_id: DeviceId,
    target_connection_id: Uuid,
}

/// Safe protocol outcome sent only to the originating active controller connection.
#[derive(Clone, Debug)]
pub struct CommandError {
    /// Stable non-enumerating error code.
    pub code: &'static str,
}

impl CommandRouter {
    /// Admits one command after authentication, owner, scope, manifest and capacity checks.
    pub async fn submit(
        &self,
        registry: &ConnectionRegistry,
        store: Option<&Arc<dyn DeviceControlStore>>,
        owner_id: Uuid,
        controller_device_id: Uuid,
        controller_connection_id: Uuid,
        mut command: DeviceCommand,
    ) -> Result<(), CommandError> {
        let now = OffsetDateTime::now_utc();
        let received_at = timestamp(now);
        let controller = registry
            .active_for(owner_id, controller_device_id)
            .filter(|active| active.connection_id == controller_connection_id)
            .ok_or(CommandError { code: "forbidden" })?;
        if !controller.manifest.roles.contains(&DeviceRole::Controller) {
            return Err(CommandError { code: "forbidden" });
        }
        command
            .validate_at(&received_at)
            .map_err(validation_error)?;
        command.executable().map_err(validation_error)?;
        let required_scope = required_scope(&command.body);
        if !controller.scopes.contains(&required_scope) {
            return Err(CommandError { code: "forbidden" });
        }
        let Some(store) = store else {
            return Err(CommandError {
                code: "persistence_unavailable",
            });
        };
        let target = match registry.active_for(owner_id, command.target.device_id.0) {
            Some(target) => target,
            None => match store
                .load_manifest(owner_id, command.target.device_id)
                .await
            {
                Ok(Some(_)) => {
                    return Err(CommandError {
                        code: "target_offline",
                    });
                }
                Ok(None) => return Err(CommandError { code: "forbidden" }),
                Err(_) => {
                    return Err(CommandError {
                        code: "persistence_unavailable",
                    });
                }
            },
        };
        validate_target(&command, &target.manifest)?;
        // The fingerprint represents the client-visible validated request. The server-selected
        // default deadline is intentionally excluded so a retry with the same command ID replays.
        let fingerprint = fingerprint(&command).map_err(|_| CommandError {
            code: "invalid_payload",
        })?;
        let deadline = command
            .deadline_at
            .as_ref()
            .map(Timestamp::instant)
            .unwrap_or(now + time::Duration::seconds(DEFAULT_TIMEOUT.as_secs() as i64));
        if deadline <= now || deadline > now + time::Duration::seconds(MAX_TIMEOUT.as_secs() as i64)
        {
            return Err(CommandError {
                code: "command_timeout",
            });
        }
        command.deadline_at = Some(timestamp(deadline));
        let reservation = CommandReservation {
            command: command.clone(),
            fingerprint,
            deadline_at: command
                .deadline_at
                .clone()
                .expect("command deadline assigned"),
        };
        let reserve = store
            .reserve_command(owner_id, command.target.device_id, reservation)
            .await
            .map_err(|_| CommandError {
                code: "persistence_unavailable",
            })?;
        if reserve == StoreOutcome::Conflict {
            return Err(CommandError {
                code: "duplicate_command",
            });
        }
        if reserve != StoreOutcome::Accepted && reserve != StoreOutcome::Replay {
            return Err(CommandError { code: "forbidden" });
        }
        if reserve == StoreOutcome::Replay {
            replay(
                store.as_ref(),
                registry,
                owner_id,
                controller_device_id,
                controller_connection_id,
                command.target.device_id,
                command.command_id,
            )
            .await?;
            return Ok(());
        }
        let over_capacity = {
            let state = self
                .state
                .lock()
                .expect("command router mutex is not poisoned");
            state
                .values()
                .filter(|item| item.controller_connection_id == controller_connection_id)
                .count()
                >= MAX_CONNECTION_IN_FLIGHT
                || state
                    .values()
                    .filter(|item| item.target_device_id == command.target.device_id)
                    .count()
                    >= MAX_TARGET_IN_FLIGHT
        };
        if over_capacity {
            let _ = complete(
                store.as_ref(),
                owner_id,
                command.target.device_id,
                terminal(&command.command_id, "too_many_in_flight"),
            )
            .await;
            return Err(CommandError {
                code: "too_many_in_flight",
            });
        }
        let in_flight = InFlight {
            owner_id,
            controller_device_id,
            controller_connection_id,
            target_device_id: command.target.device_id,
            target_connection_id: target.connection_id,
        };
        self.state
            .lock()
            .expect("command router mutex is not poisoned")
            .insert(command.command_id, in_flight);
        send(
            registry,
            owner_id,
            controller_device_id,
            controller_connection_id,
            "command.received",
            &CommandReceived {
                command_id: command.command_id,
                received_at,
                duplicate: false,
            },
        )?;
        if send(
            registry,
            owner_id,
            command.target.device_id.0,
            target.connection_id,
            "device.command",
            &command,
        )
        .is_err()
        {
            self.finish(
                registry,
                store.as_ref(),
                command.command_id,
                terminal(&command.command_id, "target_offline"),
            )
            .await;
            return Ok(());
        }
        let router = self.clone();
        let registry = registry.clone();
        let store = Arc::clone(store);
        tokio::spawn(async move {
            let remaining = std::time::Duration::try_from(deadline - OffsetDateTime::now_utc())
                .unwrap_or_default();
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                std::time::Instant::now() + remaining,
            ))
            .await;
            router
                .finish(
                    &registry,
                    store.as_ref(),
                    command.command_id,
                    terminal(&command.command_id, "command_timeout"),
                )
                .await;
        });
        Ok(())
    }

    /// Records a target acknowledgement only for the exact authenticated delivery generation.
    pub fn accepted(
        &self,
        registry: &ConnectionRegistry,
        owner_id: Uuid,
        device_id: Uuid,
        connection_id: Uuid,
        mut accepted: CommandAccepted,
    ) -> Result<(), CommandError> {
        let state = self
            .state
            .lock()
            .expect("command router mutex is not poisoned");
        let item = state.get(&accepted.command_id).ok_or(CommandError {
            code: "duplicate_command",
        })?;
        if item.owner_id != owner_id
            || item.target_device_id.0 != device_id
            || item.target_connection_id != connection_id
        {
            return Err(CommandError { code: "forbidden" });
        }
        accepted.accepted_at = timestamp(OffsetDateTime::now_utc());
        send(
            registry,
            owner_id,
            item.controller_device_id,
            item.controller_connection_id,
            "command.accepted",
            &accepted,
        )
    }

    /// Persists and forwards exactly one valid target terminal result.
    pub async fn result(
        &self,
        registry: &ConnectionRegistry,
        store: Option<&Arc<dyn DeviceControlStore>>,
        owner_id: Uuid,
        device_id: Uuid,
        connection_id: Uuid,
        mut result: CommandResult,
    ) -> Result<(), CommandError> {
        result.validate().map_err(validation_error)?;
        let item = self
            .state
            .lock()
            .expect("command router mutex is not poisoned")
            .get(&result.command_id)
            .cloned()
            .ok_or(CommandError {
                code: "duplicate_command",
            })?;
        if item.owner_id != owner_id
            || item.target_device_id.0 != device_id
            || item.target_connection_id != connection_id
        {
            return Err(CommandError { code: "forbidden" });
        }
        result.completed_at = timestamp(OffsetDateTime::now_utc());
        let Some(store) = store else {
            return Err(CommandError {
                code: "persistence_unavailable",
            });
        };
        self.finish(registry, store.as_ref(), result.command_id, result)
            .await;
        Ok(())
    }

    /// Fails only commands delivered to a disconnected exact generation; controller loss does not reroute work.
    pub async fn disconnected(
        &self,
        registry: &ConnectionRegistry,
        store: Option<&Arc<dyn DeviceControlStore>>,
        owner_id: Uuid,
        device_id: Uuid,
        connection_id: Uuid,
    ) {
        let Some(store) = store else {
            return;
        };
        let command_ids: Vec<_> = self
            .state
            .lock()
            .expect("command router mutex is not poisoned")
            .iter()
            .filter_map(|(command_id, item)| {
                (item.owner_id == owner_id
                    && item.target_device_id.0 == device_id
                    && item.target_connection_id == connection_id)
                    .then_some(*command_id)
            })
            .collect();
        for command_id in command_ids {
            self.finish(
                registry,
                store.as_ref(),
                command_id,
                terminal(&command_id, "target_offline"),
            )
            .await;
        }
    }

    /// Terminates an in-flight command exactly once and never lets a stale timeout overwrite a result.
    async fn finish(
        &self,
        registry: &ConnectionRegistry,
        store: &dyn DeviceControlStore,
        command_id: CommandId,
        result: CommandResult,
    ) {
        let item = self
            .state
            .lock()
            .expect("command router mutex is not poisoned")
            .remove(&command_id);
        let Some(item) = item else {
            return;
        };
        if complete(store, item.owner_id, item.target_device_id, result.clone()).await
            == StoreOutcome::Accepted
        {
            let _ = send(
                registry,
                item.owner_id,
                item.controller_device_id,
                item.controller_connection_id,
                "command.result",
                &result,
            );
        }
    }
}

fn required_scope(body: &CommandBody) -> DeviceControlScope {
    match body {
        CommandBody::Display { .. } => DeviceControlScope::DisplayControl,
        CommandBody::Actuator { .. } => DeviceControlScope::ActuatorControl,
        _ => DeviceControlScope::MediaControl,
    }
}

fn validate_target(command: &DeviceCommand, manifest: &DeviceManifest) -> Result<(), CommandError> {
    match &command.body {
        CommandBody::PlayStation { .. } => {
            require_role_capability(manifest, DeviceRole::Player, |capability| {
                matches!(capability, DeviceCapability::Station { .. })
            })
        }
        CommandBody::Playback { action } => require_role_capability(
            manifest,
            DeviceRole::Player,
            |capability| matches!(capability, DeviceCapability::Playback { actions } if actions.contains(action)),
        ),
        CommandBody::Volume { command } => {
            require_role_capability(manifest, DeviceRole::Player, |capability| {
                matches!(capability, DeviceCapability::Volume { mute, .. } if match command {
                    crate::device_control::VolumeCommand::SetMute { .. } => *mute,
                    crate::device_control::VolumeCommand::SetLevel { .. }
                    | crate::device_control::VolumeCommand::Change { .. } => true,
                })
            })
        }
        CommandBody::Display { presentation } => {
            let Some(surface_id) = &command.target.surface_id else {
                return Err(CommandError {
                    code: "invalid_payload",
                });
            };
            let surface = manifest
                .surfaces
                .iter()
                .find(|surface| &surface.surface_id == surface_id)
                .ok_or(CommandError {
                    code: "capability_not_supported",
                })?;
            if surface.kind != crate::device_control::SurfaceKind::Display {
                return Err(CommandError {
                    code: "capability_not_supported",
                });
            }
            let supported = manifest.capabilities.items.iter().any(|capability| matches!(capability, DeviceCapability::Display { views, max_items, max_text_length } if display_supported(presentation, views, *max_items, *max_text_length)));
            (manifest.roles.contains(&DeviceRole::DisplaySurface) && supported)
                .then_some(())
                .ok_or(CommandError {
                    code: "capability_not_supported",
                })
        }
        CommandBody::Actuator { action, value } => {
            let Some(entity_id) = &command.target.entity_id else {
                return Err(CommandError {
                    code: "invalid_payload",
                });
            };
            let entity = manifest
                .entities
                .iter()
                .find(|entity| &entity.entity_id == entity_id)
                .ok_or(CommandError {
                    code: "capability_not_supported",
                })?;
            let action_supported = manifest.capabilities.items.iter().any(|capability| matches!(capability, DeviceCapability::Actuator { commands } if commands.contains(action)));
            if !manifest.roles.contains(&DeviceRole::Actuator)
                || !entity.controllable
                || !action_supported
            {
                return Err(CommandError {
                    code: "capability_not_supported",
                });
            }
            if let (ActuatorAction::SetValue, Some(value)) = (action, value)
                && (entity.minimum.is_some_and(|minimum| *value < minimum)
                    || entity.maximum.is_some_and(|maximum| *value > maximum))
            {
                return Err(CommandError {
                    code: "invalid_payload",
                });
            }
            Ok(())
        }
        CommandBody::Unknown { .. } => Err(CommandError {
            code: "unsupported_command",
        }),
    }
}

fn require_role_capability(
    manifest: &DeviceManifest,
    role: DeviceRole,
    capability: impl Fn(&DeviceCapability) -> bool,
) -> Result<(), CommandError> {
    (manifest.roles.contains(&role) && manifest.capabilities.items.iter().any(capability))
        .then_some(())
        .ok_or(CommandError {
            code: "capability_not_supported",
        })
}

fn display_supported(
    presentation: &crate::device_control::Presentation,
    views: &[crate::device_control::ViewKind],
    max_items: u8,
    max_text_length: u16,
) -> bool {
    match presentation {
        crate::device_control::Presentation::Text { text } => {
            text.len() <= usize::from(max_text_length)
        }
        crate::device_control::Presentation::NowPlaying { .. } => {
            views.contains(&crate::device_control::ViewKind::NowPlaying)
        }
        crate::device_control::Presentation::SensorGrid { items, .. } => {
            views.contains(&crate::device_control::ViewKind::SensorGrid)
                && items.len() <= usize::from(max_items)
        }
    }
}

async fn replay(
    store: &dyn DeviceControlStore,
    registry: &ConnectionRegistry,
    owner_id: Uuid,
    controller_device_id: Uuid,
    controller_connection_id: Uuid,
    target_device_id: DeviceId,
    command_id: CommandId,
) -> Result<(), CommandError> {
    let lifecycle = store
        .load_command(owner_id, target_device_id, command_id)
        .await
        .map_err(|_| CommandError {
            code: "persistence_unavailable",
        })?;
    send(
        registry,
        owner_id,
        controller_device_id,
        controller_connection_id,
        "command.received",
        &CommandReceived {
            command_id,
            received_at: timestamp(OffsetDateTime::now_utc()),
            duplicate: true,
        },
    )?;
    if let Some(result) = lifecycle.and_then(|lifecycle| lifecycle.result) {
        send(
            registry,
            owner_id,
            controller_device_id,
            controller_connection_id,
            "command.result",
            &result,
        )?;
    }
    Ok(())
}

async fn complete(
    store: &dyn DeviceControlStore,
    owner_id: Uuid,
    target_device_id: DeviceId,
    result: CommandResult,
) -> StoreOutcome {
    store
        .complete_command(owner_id, target_device_id, result)
        .await
        .unwrap_or(StoreOutcome::Conflict)
}

fn send<T: serde::Serialize>(
    registry: &ConnectionRegistry,
    owner_id: Uuid,
    device_id: Uuid,
    connection_id: Uuid,
    kind: &'static str,
    payload: &T,
) -> Result<(), CommandError> {
    registry
        .send_to(
            owner_id,
            device_id,
            connection_id,
            OutboundFrame {
                kind,
                payload: serde_json::to_value(payload).map_err(|_| CommandError {
                    code: "invalid_payload",
                })?,
            },
        )
        .then_some(())
        .ok_or(CommandError {
            code: "target_offline",
        })
}

fn fingerprint(command: &DeviceCommand) -> Result<[u8; 32], serde_json::Error> {
    Ok(Sha256::digest(serde_json::to_vec(command)?).into())
}

fn terminal(command_id: &CommandId, code: &'static str) -> CommandResult {
    CommandResult {
        command_id: *command_id,
        status: CommandStatus::Failed,
        completed_at: timestamp(OffsetDateTime::now_utc()),
        error: Some(DomainError {
            code: code.into(),
            message: "Device command did not complete.".into(),
        }),
    }
}

fn timestamp(value: OffsetDateTime) -> Timestamp {
    Timestamp::parse(value.format(&Rfc3339).expect("RFC3339 format is valid"))
        .expect("server timestamp is valid")
}

fn validation_error(error: crate::device_control::ValidationError) -> CommandError {
    CommandError { code: error.code() }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use super::*;
    use crate::device_control::{
        DeviceCapabilities, DeviceManifest, DeviceStateSnapshot, Entity, EntityState, StoreError,
        Surface,
    };

    type StoredCommand = (Uuid, DeviceId, CommandReservation, Option<CommandResult>);

    #[derive(Default)]
    struct MemoryStore {
        commands: Mutex<HashMap<CommandId, StoredCommand>>,
        manifests: Mutex<HashMap<(Uuid, DeviceId), DeviceManifest>>,
    }

    #[async_trait]
    impl DeviceControlStore for MemoryStore {
        async fn apply_manifest(
            &self,
            user: Uuid,
            device: DeviceId,
            manifest: DeviceManifest,
        ) -> Result<StoreOutcome, StoreError> {
            self.manifests
                .lock()
                .expect("test mutex")
                .insert((user, device), manifest);
            Ok(StoreOutcome::Accepted)
        }
        async fn load_manifest(
            &self,
            user: Uuid,
            device: DeviceId,
        ) -> Result<Option<DeviceManifest>, StoreError> {
            Ok(self
                .manifests
                .lock()
                .expect("test mutex")
                .get(&(user, device))
                .cloned())
        }
        async fn list_entities(&self, _: Uuid, _: DeviceId) -> Result<Vec<Entity>, StoreError> {
            Ok(Vec::new())
        }
        async fn list_surfaces(&self, _: Uuid, _: DeviceId) -> Result<Vec<Surface>, StoreError> {
            Ok(Vec::new())
        }
        async fn list_capabilities(
            &self,
            _: Uuid,
            _: DeviceId,
        ) -> Result<Vec<DeviceCapability>, StoreError> {
            Ok(Vec::new())
        }
        async fn store_device_state(
            &self,
            _: Uuid,
            _: DeviceId,
            _: DeviceStateSnapshot,
        ) -> Result<StoreOutcome, StoreError> {
            Ok(StoreOutcome::Accepted)
        }
        async fn load_device_state(
            &self,
            _: Uuid,
            _: DeviceId,
        ) -> Result<Option<DeviceStateSnapshot>, StoreError> {
            Ok(None)
        }
        async fn store_entity_state(
            &self,
            _: Uuid,
            _: DeviceId,
            _: EntityState,
        ) -> Result<StoreOutcome, StoreError> {
            Ok(StoreOutcome::Accepted)
        }
        async fn load_entity_state(
            &self,
            _: Uuid,
            _: DeviceId,
            _: &str,
        ) -> Result<Option<EntityState>, StoreError> {
            Ok(None)
        }
        async fn reserve_command(
            &self,
            user: Uuid,
            device: DeviceId,
            request: CommandReservation,
        ) -> Result<StoreOutcome, StoreError> {
            let mut commands = self.commands.lock().expect("test mutex");
            match commands.get(&request.command.command_id) {
                Some((stored_user, stored_device, stored, _))
                    if *stored_user == user
                        && *stored_device == device
                        && stored.fingerprint == request.fingerprint =>
                {
                    Ok(StoreOutcome::Replay)
                }
                Some(_) => Ok(StoreOutcome::Conflict),
                None => {
                    commands.insert(request.command.command_id, (user, device, request, None));
                    Ok(StoreOutcome::Accepted)
                }
            }
        }
        async fn load_command(
            &self,
            user: Uuid,
            device: DeviceId,
            command_id: CommandId,
        ) -> Result<Option<crate::device_control::CommandLifecycle>, StoreError> {
            Ok(self
                .commands
                .lock()
                .expect("test mutex")
                .get(&command_id)
                .filter(|(stored_user, stored_device, _, _)| {
                    *stored_user == user && *stored_device == device
                })
                .map(
                    |(_, _, request, result)| crate::device_control::CommandLifecycle {
                        command: request.command.clone(),
                        result: result.clone(),
                    },
                ))
        }
        async fn complete_command(
            &self,
            user: Uuid,
            device: DeviceId,
            result: CommandResult,
        ) -> Result<StoreOutcome, StoreError> {
            let mut commands = self.commands.lock().expect("test mutex");
            let Some((stored_user, stored_device, _, prior)) = commands.get_mut(&result.command_id)
            else {
                return Ok(StoreOutcome::NotOwned);
            };
            if *stored_user != user || *stored_device != device {
                return Ok(StoreOutcome::NotOwned);
            }
            if let Some(prior) = prior {
                return Ok(if *prior == result {
                    StoreOutcome::Replay
                } else {
                    StoreOutcome::Conflict
                });
            }
            *prior = Some(result);
            Ok(StoreOutcome::Accepted)
        }
        async fn prune_commands(&self, _: u32) -> Result<u64, StoreError> {
            Ok(0)
        }
    }

    fn manifest(roles: Vec<DeviceRole>, capabilities: Vec<DeviceCapability>) -> DeviceManifest {
        DeviceManifest {
            manifest_revision: 1,
            roles,
            capabilities: DeviceCapabilities {
                revision: 1,
                items: capabilities,
            },
            entities: Vec::new(),
            surfaces: Vec::new(),
        }
    }

    fn register(
        registry: &ConnectionRegistry,
        owner: Uuid,
        device: Uuid,
        manifest: DeviceManifest,
        scopes: Vec<DeviceControlScope>,
    ) -> (Uuid, mpsc::Receiver<OutboundFrame>) {
        let connection = Uuid::new_v4();
        let (replacement, _) = registry.replacement_channel();
        let (outbound, receiver) = registry.outbound_channel();
        registry.register(
            crate::device_control_presence::ConnectionRegistration {
                user_id: owner,
                device_id: device,
                connection_id: connection,
                replacement,
                outbound,
                manifest,
                scopes,
            },
            std::time::Instant::now(),
        );
        (connection, receiver)
    }

    fn command(target: Uuid) -> DeviceCommand {
        DeviceCommand {
            command_id: CommandId(Uuid::new_v4()),
            target: crate::device_control::CommandTarget {
                device_id: DeviceId(target),
                entity_id: None,
                surface_id: None,
            },
            deadline_at: None,
            body: CommandBody::PlayStation {
                station_id: "calm-jazz".into(),
            },
        }
    }

    #[tokio::test]
    async fn command_is_delivered_once_and_only_terminal_target_result_completes_it() {
        let registry = ConnectionRegistry::default();
        let router = CommandRouter::default();
        let store: Arc<dyn DeviceControlStore> = Arc::new(MemoryStore::default());
        let owner = Uuid::new_v4();
        let controller = Uuid::new_v4();
        let target = Uuid::new_v4();
        let (controller_connection, mut controller_messages) = register(
            &registry,
            owner,
            controller,
            manifest(vec![DeviceRole::Controller], Vec::new()),
            vec![DeviceControlScope::MediaControl],
        );
        let (target_connection, mut target_messages) = register(
            &registry,
            owner,
            target,
            manifest(
                vec![DeviceRole::Player],
                vec![DeviceCapability::Station {
                    sources: vec!["catalog".into()],
                }],
            ),
            Vec::new(),
        );
        store
            .apply_manifest(
                owner,
                DeviceId(target),
                registry.active_for(owner, target).unwrap().manifest,
            )
            .await
            .unwrap();
        let command = command(target);
        router
            .submit(
                &registry,
                Some(&store),
                owner,
                controller,
                controller_connection,
                command.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            controller_messages.recv().await.unwrap().kind,
            "command.received"
        );
        assert_eq!(target_messages.recv().await.unwrap().kind, "device.command");
        router
            .accepted(
                &registry,
                owner,
                target,
                target_connection,
                CommandAccepted {
                    command_id: command.command_id,
                    accepted_at: timestamp(OffsetDateTime::now_utc()),
                },
            )
            .unwrap();
        assert_eq!(
            controller_messages.recv().await.unwrap().kind,
            "command.accepted"
        );
        router
            .result(
                &registry,
                Some(&store),
                owner,
                target,
                target_connection,
                CommandResult {
                    command_id: command.command_id,
                    status: CommandStatus::Succeeded,
                    completed_at: timestamp(OffsetDateTime::now_utc()),
                    error: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            controller_messages.recv().await.unwrap().kind,
            "command.result"
        );
        router
            .result(
                &registry,
                Some(&store),
                owner,
                target,
                target_connection,
                CommandResult {
                    command_id: command.command_id,
                    status: CommandStatus::Succeeded,
                    completed_at: timestamp(OffsetDateTime::now_utc()),
                    error: None,
                },
            )
            .await
            .unwrap_err();
        router
            .submit(
                &registry,
                Some(&store),
                owner,
                controller,
                controller_connection,
                command,
            )
            .await
            .unwrap();
        assert_eq!(
            controller_messages.recv().await.unwrap().kind,
            "command.received"
        );
        assert_eq!(
            controller_messages.recv().await.unwrap().kind,
            "command.result"
        );
        assert!(target_messages.try_recv().is_err());
    }

    #[tokio::test]
    async fn spoofed_or_offline_targets_are_never_delivered() {
        let registry = ConnectionRegistry::default();
        let router = CommandRouter::default();
        let store: Arc<dyn DeviceControlStore> = Arc::new(MemoryStore::default());
        let owner = Uuid::new_v4();
        let controller = Uuid::new_v4();
        let target = Uuid::new_v4();
        let (controller_connection, mut controller_messages) = register(
            &registry,
            owner,
            controller,
            manifest(vec![DeviceRole::Controller], Vec::new()),
            vec![DeviceControlScope::MediaControl],
        );
        let (target_connection, mut target_messages) = register(
            &registry,
            owner,
            target,
            manifest(
                vec![DeviceRole::Player],
                vec![DeviceCapability::Station {
                    sources: vec!["catalog".into()],
                }],
            ),
            Vec::new(),
        );
        store
            .apply_manifest(
                owner,
                DeviceId(target),
                registry.active_for(owner, target).unwrap().manifest,
            )
            .await
            .unwrap();
        let sent = command(target);
        router
            .submit(
                &registry,
                Some(&store),
                owner,
                controller,
                controller_connection,
                sent.clone(),
            )
            .await
            .unwrap();
        let _ = controller_messages.recv().await;
        let _ = target_messages.recv().await;
        assert_eq!(
            router
                .accepted(
                    &registry,
                    owner,
                    Uuid::new_v4(),
                    target_connection,
                    CommandAccepted {
                        command_id: sent.command_id,
                        accepted_at: timestamp(OffsetDateTime::now_utc())
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        registry.disconnect(
            target,
            target_connection,
            crate::device_control_presence::DisconnectReason::TransportLost,
            std::time::Instant::now(),
        );
        router
            .disconnected(&registry, Some(&store), owner, target, target_connection)
            .await;
        assert_eq!(
            router
                .submit(
                    &registry,
                    Some(&store),
                    owner,
                    controller,
                    controller_connection,
                    command(target)
                )
                .await
                .unwrap_err()
                .code,
            "target_offline"
        );
    }
}

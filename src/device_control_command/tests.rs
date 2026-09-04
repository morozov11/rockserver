//! Focused regression tests for this private domain facade.

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

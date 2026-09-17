//! Bounded, owner-scoped live state fan-out for device-control consumers.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::sync::broadcast;
use uuid::Uuid;

use crate::device_control::{DeviceId, DeviceStateSnapshot, EntityState};

const SUBSCRIBER_CAPACITY: usize = 64;
type EntityStates = HashMap<(Uuid, DeviceId, String), EntityState>;

/// A live state or telemetry notification scoped to exactly one account.
#[derive(Clone, Debug, PartialEq)]
pub enum StateEvent {
    /// The latest device declaration was accepted.
    Manifest { device_id: DeviceId },
    /// The registry accepted or removed an owned live connection.
    Presence { device_id: DeviceId, online: bool },
    /// The latest complete device state was accepted.
    DeviceState {
        device_id: DeviceId,
        state: DeviceStateSnapshot,
    },
    /// The latest entity state was accepted.
    EntityState {
        device_id: DeviceId,
        state: EntityState,
    },
}

/// Process-local latest-state cache and bounded subscriber hub.
///
/// Every account receives a separate lossy broadcast channel. A lagging subscriber receives a
/// `Lagged` result from Tokio instead of making a provider connection wait or accumulating data.
#[derive(Clone, Default)]
pub struct StateHub {
    state: Arc<Mutex<HashMap<(Uuid, DeviceId), DeviceStateSnapshot>>>,
    entities: Arc<Mutex<EntityStates>>,
    subscribers: Arc<Mutex<HashMap<Uuid, broadcast::Sender<StateEvent>>>>,
    cursors: Arc<Mutex<HashMap<Uuid, u64>>>,
}

impl StateHub {
    /// Returns the current account-local event watermark for directory snapshots.
    pub fn cursor(&self, user_id: Uuid) -> u64 {
        self.cursors
            .lock()
            .expect("state hub mutex")
            .get(&user_id)
            .copied()
            .unwrap_or(0)
    }
    /// Returns a bounded subscription that can observe only the supplied account's events.
    pub fn subscribe(&self, user_id: Uuid) -> broadcast::Receiver<StateEvent> {
        self.sender(user_id).subscribe()
    }

    /// Returns the current accepted device state for the same owner and device.
    pub fn device_state(&self, user_id: Uuid, device_id: DeviceId) -> Option<DeviceStateSnapshot> {
        self.state
            .lock()
            .expect("state hub mutex")
            .get(&(user_id, device_id))
            .cloned()
    }

    /// Returns the current accepted entity state for the same owner and device.
    pub fn entity_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        entity_id: &str,
    ) -> Option<EntityState> {
        self.entities
            .lock()
            .expect("state hub mutex")
            .get(&(user_id, device_id, entity_id.to_owned()))
            .cloned()
    }

    /// Publishes an online or offline transition after registry generation checks.
    pub fn publish_presence(&self, user_id: Uuid, device_id: DeviceId, online: bool) {
        self.advance_cursor(user_id);
        let _ = self
            .sender(user_id)
            .send(StateEvent::Presence { device_id, online });
    }

    /// Publishes a durable manifest replacement without duplicating its payload in the hub.
    pub fn publish_manifest(&self, user_id: Uuid, device_id: DeviceId) {
        self.advance_cursor(user_id);
        let _ = self
            .sender(user_id)
            .send(StateEvent::Manifest { device_id });
    }

    /// Replaces and broadcasts one accepted device snapshot.
    pub fn publish_device_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        state: DeviceStateSnapshot,
    ) {
        self.advance_cursor(user_id);
        self.state
            .lock()
            .expect("state hub mutex")
            .insert((user_id, device_id), state.clone());
        let _ = self
            .sender(user_id)
            .send(StateEvent::DeviceState { device_id, state });
    }

    /// Replaces and broadcasts one accepted entity observation.
    pub fn publish_entity_state(&self, user_id: Uuid, device_id: DeviceId, state: EntityState) {
        self.advance_cursor(user_id);
        self.entities
            .lock()
            .expect("state hub mutex")
            .insert((user_id, device_id, state.entity_id.clone()), state.clone());
        let _ = self
            .sender(user_id)
            .send(StateEvent::EntityState { device_id, state });
    }

    fn sender(&self, user_id: Uuid) -> broadcast::Sender<StateEvent> {
        let mut subscribers = self.subscribers.lock().expect("state hub mutex");
        subscribers
            .entry(user_id)
            .or_insert_with(|| broadcast::channel(SUBSCRIBER_CAPACITY).0)
            .clone()
    }

    fn advance_cursor(&self, user_id: Uuid) {
        let mut cursors = self.cursors.lock().expect("state hub mutex");
        *cursors.entry(user_id).or_insert(0) += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriber_is_owner_scoped() {
        let hub = StateHub::default();
        let owner = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut receiver = hub.subscribe(other);
        let _owner_receiver = hub.subscribe(owner);
        assert!(
            hub.sender(owner)
                .send(StateEvent::DeviceState {
                    device_id: DeviceId(Uuid::new_v4()),
                    state: DeviceStateSnapshot {
                        state_revision: 1,
                        observed_at: crate::device_control::Timestamp::parse(
                            "2026-09-03T00:00:00Z"
                        )
                        .unwrap(),
                        received_at: None,
                        state: Default::default()
                    }
                })
                .is_ok()
        );
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn slow_subscriber_is_bounded_and_does_not_block_publisher() {
        let hub = StateHub::default();
        let user = Uuid::new_v4();
        let device = DeviceId(Uuid::new_v4());
        let mut receiver = hub.subscribe(user);
        for revision in 1..=65 {
            hub.publish_device_state(
                user,
                device,
                DeviceStateSnapshot {
                    state_revision: revision,
                    observed_at: crate::device_control::Timestamp::parse("2026-09-03T00:00:00Z")
                        .unwrap(),
                    received_at: None,
                    state: Default::default(),
                },
            );
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
    }

    #[test]
    fn device_state_keeps_only_the_last_accepted_snapshot_per_device() {
        let hub = StateHub::default();
        let user = Uuid::new_v4();
        let player = DeviceId(Uuid::new_v4());
        let legacy = DeviceId(Uuid::new_v4());
        let snapshot = |revision: u64, status: &str, level: u8| DeviceStateSnapshot {
            state_revision: revision,
            observed_at: crate::device_control::Timestamp::parse("2026-09-03T00:00:00Z").unwrap(),
            received_at: None,
            state: crate::device_control::DeviceRuntimeState {
                playback: Some(crate::device_control::PlaybackState {
                    status: status.to_owned(),
                    station_id: Some("station-rock-001".to_owned()),
                }),
                volume: Some(crate::device_control::VolumeState {
                    level,
                    muted: false,
                }),
                display: None,
            },
        };
        hub.publish_device_state(user, player, snapshot(7, "buffering", 62));
        hub.publish_device_state(user, player, snapshot(8, "playing", 62));
        // Another device's projection is independent, so one device catching up never
        // overwrites a neighbour's state.
        hub.publish_device_state(user, legacy, snapshot(2, "idle", 10));
        let latest = hub
            .device_state(user, player)
            .expect("the last accepted snapshot is the directory runtime_state source");
        assert_eq!(latest.state_revision, 8);
        assert_eq!(
            latest.state.playback.expect("playback state").status,
            "playing"
        );
        assert_eq!(latest.state.volume.expect("volume state").level, 62);
        assert_eq!(
            hub.device_state(user, legacy)
                .expect("legacy device keeps its own projection")
                .state_revision,
            2
        );
        assert!(
            hub.device_state(user, DeviceId(Uuid::new_v4())).is_none(),
            "a device that never published state has no projection to fabricate"
        );
    }
}

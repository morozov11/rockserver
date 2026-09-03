//! Process-local device-control connection and presence registry.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::device_control::{DeviceControlScope, DeviceManifest};

const USER_EVENT_CAPACITY: usize = 128;
static CONTROL_SHUTDOWN: OnceLock<broadcast::Sender<()>> = OnceLock::new();

fn control_shutdown() -> &'static broadcast::Sender<()> {
    CONTROL_SHUTDOWN.get_or_init(|| broadcast::channel(1).0)
}

/// Subscribes a live control connection to process shutdown.
pub(crate) fn control_shutdown_subscriber() -> broadcast::Receiver<()> {
    control_shutdown().subscribe()
}

/// Tells all live device-control sessions to close before server shutdown completes.
pub(crate) fn signal_control_shutdown() {
    let _ = control_shutdown().send(());
}

/// Why an active device-control connection stopped being present.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisconnectReason {
    /// The peer closed the WebSocket normally.
    GracefulDisconnect,
    /// The server stopped receiving heartbeats within its TTL.
    HeartbeatExpired,
    /// A newer successful registration replaced this connection.
    Replaced,
    /// The device credential was revoked.
    Revoked,
    /// The WebSocket transport ended unexpectedly.
    TransportLost,
}

/// Owner-scoped presence transition retained for internal consumers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresenceEvent {
    /// Owning user; callers must scope reads to this value.
    pub user_id: Uuid,
    /// Stable paired device identifier.
    pub device_id: Uuid,
    /// Per-WebSocket server-issued connection identifier.
    pub connection_id: Uuid,
    /// Whether this transition made the device online.
    pub online: bool,
    /// Server observation time.
    pub last_seen: Instant,
    /// Offline cause, when `online` is false.
    pub reason: Option<DisconnectReason>,
}

struct ActiveConnection {
    user_id: Uuid,
    connection_id: Uuid,
    last_seen: Instant,
    replacement: mpsc::Sender<DisconnectReason>,
    outbound: mpsc::Sender<OutboundFrame>,
    manifest: DeviceManifest,
    scopes: Vec<DeviceControlScope>,
}

/// One bounded server-to-device protocol message awaiting WebSocket serialization.
#[derive(Clone, Debug)]
pub struct OutboundFrame {
    /// Protocol message type.
    pub kind: &'static str,
    /// Already typed payload encoded only at the transport edge.
    pub payload: serde_json::Value,
}

/// Server-derived metadata for the active generation of an owned device.
#[derive(Clone, Debug)]
pub struct ActiveDevice {
    /// Active connection generation.
    pub connection_id: Uuid,
    /// Registered truthful declaration for this generation.
    pub manifest: DeviceManifest,
    /// Explicit scopes granted by server policy.
    pub scopes: Vec<DeviceControlScope>,
}

/// Complete server-side registration data for one active WebSocket generation.
pub struct ConnectionRegistration {
    /// Authenticated owner namespace.
    pub user_id: Uuid,
    /// Stable paired device identity.
    pub device_id: Uuid,
    /// Server-generated connection generation.
    pub connection_id: Uuid,
    /// Replacement notification sender owned by this session.
    pub replacement: mpsc::Sender<DisconnectReason>,
    /// Bounded protocol delivery sender owned by this session.
    pub outbound: mpsc::Sender<OutboundFrame>,
    /// The current registration manifest.
    pub manifest: DeviceManifest,
    /// The server-derived granted scopes.
    pub scopes: Vec<DeviceControlScope>,
}

#[derive(Default)]
struct RegistryState {
    active: HashMap<Uuid, ActiveConnection>,
    events: HashMap<Uuid, VecDeque<PresenceEvent>>,
}

/// A bounded, process-local registry of live authenticated device connections.
#[derive(Clone, Default)]
pub struct ConnectionRegistry {
    state: Arc<Mutex<RegistryState>>,
}

impl ConnectionRegistry {
    /// Creates a bounded replacement notification channel for one WebSocket session.
    pub fn replacement_channel(
        &self,
    ) -> (
        mpsc::Sender<DisconnectReason>,
        mpsc::Receiver<DisconnectReason>,
    ) {
        mpsc::channel(1)
    }

    /// Creates the bounded command/event delivery channel for one WebSocket session.
    pub fn outbound_channel(&self) -> (mpsc::Sender<OutboundFrame>, mpsc::Receiver<OutboundFrame>) {
        mpsc::channel(16)
    }

    /// Atomically makes this connection active, replacing any earlier connection for its device.
    pub fn register(
        &self,
        registration: ConnectionRegistration,
        now: Instant,
    ) -> Option<(Uuid, Uuid)> {
        let mut state = self
            .state
            .lock()
            .expect("connection registry mutex is not poisoned");
        let previous = state.active.insert(
            registration.device_id,
            ActiveConnection {
                user_id: registration.user_id,
                connection_id: registration.connection_id,
                last_seen: now,
                replacement: registration.replacement,
                outbound: registration.outbound,
                manifest: registration.manifest,
                scopes: registration.scopes,
            },
        );
        if let Some(previous) = &previous {
            let _ = previous.replacement.try_send(DisconnectReason::Replaced);
        }
        push_event(
            &mut state,
            PresenceEvent {
                user_id: registration.user_id,
                device_id: registration.device_id,
                connection_id: registration.connection_id,
                online: true,
                last_seen: now,
                reason: None,
            },
        );
        previous.map(|previous| (previous.user_id, previous.connection_id))
    }

    /// Returns an active target only when it belongs to the supplied owner.
    pub fn active_for(&self, user_id: Uuid, device_id: Uuid) -> Option<ActiveDevice> {
        self.state
            .lock()
            .expect("connection registry mutex is not poisoned")
            .active
            .get(&device_id)
            .filter(|active| active.user_id == user_id)
            .map(|active| ActiveDevice {
                connection_id: active.connection_id,
                manifest: active.manifest.clone(),
                scopes: active.scopes.clone(),
            })
    }

    /// Replaces the declaration for exactly the active connection generation.
    pub fn update_manifest(
        &self,
        device_id: Uuid,
        connection_id: Uuid,
        manifest: DeviceManifest,
    ) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("connection registry mutex is not poisoned");
        let Some(active) = state
            .active
            .get_mut(&device_id)
            .filter(|active| active.connection_id == connection_id)
        else {
            return false;
        };
        active.manifest = manifest;
        true
    }

    /// Attempts non-blocking delivery to one exact active connection generation.
    pub fn send_to(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        connection_id: Uuid,
        frame: OutboundFrame,
    ) -> bool {
        self.state
            .lock()
            .expect("connection registry mutex is not poisoned")
            .active
            .get(&device_id)
            .filter(|active| active.user_id == user_id && active.connection_id == connection_id)
            .map(|active| active.outbound.clone())
            .is_some_and(|sender| sender.try_send(frame).is_ok())
    }

    /// Refreshes server-observed activity only if this connection is still the active generation.
    pub fn heartbeat(&self, device_id: Uuid, connection_id: Uuid, now: Instant) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("connection registry mutex is not poisoned");
        let Some(active) = state
            .active
            .get_mut(&device_id)
            .filter(|active| active.connection_id == connection_id)
        else {
            return false;
        };
        active.last_seen = now;
        true
    }

    /// Removes this connection only if it still owns the active generation, emitting one offline event.
    pub fn disconnect(
        &self,
        device_id: Uuid,
        connection_id: Uuid,
        reason: DisconnectReason,
        _now: Instant,
    ) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("connection registry mutex is not poisoned");
        if state
            .active
            .get(&device_id)
            .is_none_or(|active| active.connection_id != connection_id)
        {
            return false;
        }
        let active = state
            .active
            .remove(&device_id)
            .expect("active connection was checked");
        push_event(
            &mut state,
            PresenceEvent {
                user_id: active.user_id,
                device_id,
                connection_id,
                online: false,
                last_seen: active.last_seen,
                reason: Some(reason),
            },
        );
        true
    }

    /// Removes an owner's active device after revocation and tells its socket to close.
    ///
    /// The user ID check prevents an owner-scoped revoke from affecting another account's device.
    pub fn revoke(&self, user_id: Uuid, device_id: Uuid, _now: Instant) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("connection registry mutex is not poisoned");
        if state
            .active
            .get(&device_id)
            .is_none_or(|active| active.user_id != user_id)
        {
            return false;
        }
        let active = state
            .active
            .remove(&device_id)
            .expect("active connection was checked");
        let _ = active.replacement.try_send(DisconnectReason::Revoked);
        push_event(
            &mut state,
            PresenceEvent {
                user_id,
                device_id,
                connection_id: active.connection_id,
                online: false,
                last_seen: active.last_seen,
                reason: Some(DisconnectReason::Revoked),
            },
        );
        true
    }

    /// Returns only the caller's retained presence history; other users are never enumerated.
    pub fn events_for(&self, user_id: Uuid) -> Vec<PresenceEvent> {
        self.state
            .lock()
            .expect("connection registry mutex is not poisoned")
            .events
            .get(&user_id)
            .map(|events| events.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Returns the caller's currently online devices as `(device_id, connection_id, last_seen)`.
    pub fn snapshot_for(&self, user_id: Uuid) -> Vec<(Uuid, Uuid, Instant)> {
        self.state
            .lock()
            .expect("connection registry mutex is not poisoned")
            .active
            .iter()
            .filter_map(|(device_id, active)| {
                (active.user_id == user_id).then_some((
                    *device_id,
                    active.connection_id,
                    active.last_seen,
                ))
            })
            .collect()
    }
}

fn push_event(state: &mut RegistryState, event: PresenceEvent) {
    let events = state.events.entry(event.user_id).or_default();
    if events.len() == USER_EVENT_CAPACITY {
        events.pop_front();
    }
    events.push_back(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration() -> (
        mpsc::Sender<OutboundFrame>,
        DeviceManifest,
        Vec<DeviceControlScope>,
    ) {
        let (outbound, _) = mpsc::channel(1);
        (
            outbound,
            DeviceManifest {
                manifest_revision: 1,
                roles: Vec::new(),
                capabilities: crate::device_control::DeviceCapabilities {
                    revision: 1,
                    items: Vec::new(),
                },
                entities: Vec::new(),
                surfaces: Vec::new(),
            },
            Vec::new(),
        )
    }

    #[test]
    fn replacement_and_stale_cleanup_do_not_take_new_connection_offline() {
        let registry = ConnectionRegistry::default();
        let user = Uuid::new_v4();
        let device = Uuid::new_v4();
        let old = Uuid::new_v4();
        let new = Uuid::new_v4();
        let now = Instant::now();
        let (old_tx, mut old_rx) = registry.replacement_channel();
        let (outbound, manifest, scopes) = registration();
        registry.register(
            ConnectionRegistration {
                user_id: user,
                device_id: device,
                connection_id: old,
                replacement: old_tx,
                outbound,
                manifest,
                scopes,
            },
            now,
        );
        let (new_tx, _) = registry.replacement_channel();
        let (outbound, manifest, scopes) = registration();
        registry.register(
            ConnectionRegistration {
                user_id: user,
                device_id: device,
                connection_id: new,
                replacement: new_tx,
                outbound,
                manifest,
                scopes,
            },
            now,
        );
        assert_eq!(old_rx.try_recv(), Ok(DisconnectReason::Replaced));
        assert!(!registry.disconnect(device, old, DisconnectReason::Replaced, now));
        assert_eq!(registry.snapshot_for(user), vec![(device, new, now)]);
        assert_eq!(
            registry
                .events_for(user)
                .iter()
                .filter(|event| !event.online)
                .count(),
            0
        );
    }

    #[test]
    fn history_and_snapshot_are_owner_scoped() {
        let registry = ConnectionRegistry::default();
        let one = Uuid::new_v4();
        let two = Uuid::new_v4();
        let device = Uuid::new_v4();
        let connection = Uuid::new_v4();
        let (tx, _) = registry.replacement_channel();
        let (outbound, manifest, scopes) = registration();
        registry.register(
            ConnectionRegistration {
                user_id: one,
                device_id: device,
                connection_id: connection,
                replacement: tx,
                outbound,
                manifest,
                scopes,
            },
            Instant::now(),
        );
        assert_eq!(registry.snapshot_for(two), Vec::new());
        assert_eq!(registry.events_for(two), Vec::new());
    }

    #[test]
    fn revocation_is_owner_scoped_and_emits_one_offline_event() {
        let registry = ConnectionRegistry::default();
        let owner = Uuid::new_v4();
        let other = Uuid::new_v4();
        let device = Uuid::new_v4();
        let connection = Uuid::new_v4();
        let (tx, mut rx) = registry.replacement_channel();
        let (outbound, manifest, scopes) = registration();
        registry.register(
            ConnectionRegistration {
                user_id: owner,
                device_id: device,
                connection_id: connection,
                replacement: tx,
                outbound,
                manifest,
                scopes,
            },
            Instant::now(),
        );
        assert!(!registry.revoke(other, device, Instant::now()));
        assert!(registry.revoke(owner, device, Instant::now()));
        assert_eq!(rx.try_recv(), Ok(DisconnectReason::Revoked));
        assert_eq!(
            registry.events_for(owner).last().unwrap().reason,
            Some(DisconnectReason::Revoked)
        );
    }
}

//! Private state-admission helpers for the device-control WebSocket lifecycle.

use uuid::Uuid;

use crate::{
    device_control::{
        DeviceId, DeviceRuntimeState, DeviceStateDelta, DeviceStateSnapshot, EntityState,
        RevisionOrder, revision_order,
    },
    device_control_state::StateHub,
};

/// Rejects impossible or deliberately empty complete runtime observations.
pub(super) fn valid_snapshot(snapshot: &DeviceStateSnapshot) -> bool {
    snapshot.state_revision > 0
        && (snapshot.state.playback.is_some()
            || snapshot.state.volume.is_some()
            || snapshot.state.display.is_some())
}

/// Rejects a delta that cannot represent the next monotonic device observation.
pub(super) fn valid_delta(delta: &DeviceStateDelta) -> bool {
    delta.base_revision > 0
        && delta.state_revision > delta.base_revision
        && (delta.changes.playback.is_some()
            || delta.changes.volume.is_some()
            || delta.changes.display.is_some())
}

/// Applies only fields explicitly included by a typed state delta.
pub(super) fn merge_state(
    current: DeviceRuntimeState,
    changes: DeviceRuntimeState,
) -> DeviceRuntimeState {
    DeviceRuntimeState {
        playback: changes.playback.or(current.playback),
        volume: changes.volume.or(current.volume),
        display: changes.display.or(current.display),
    }
}

/// Publishes a full snapshot only when its revision is a legal successor.
pub(super) fn accept_snapshot(
    hub: &StateHub,
    user_id: Uuid,
    device_id: Uuid,
    snapshot: DeviceStateSnapshot,
) -> RevisionOrder {
    let device_id = DeviceId(device_id);
    let result = match hub.device_state(user_id, device_id) {
        Some(current) => revision_order(
            current.state_revision,
            &current.state,
            snapshot.state_revision,
            &snapshot.state,
            None,
        ),
        None => RevisionOrder::Next,
    };
    if result == RevisionOrder::Next {
        hub.publish_device_state(user_id, device_id, snapshot);
    }
    result
}

/// Classifies a typed entity observation against the account-scoped latest observation.
pub(super) fn entity_revision(
    hub: &StateHub,
    user_id: Uuid,
    device_id: DeviceId,
    incoming: &EntityState,
) -> RevisionOrder {
    match hub.entity_state(user_id, device_id, &incoming.entity_id) {
        Some(accepted) => revision_order(
            accepted.entity_revision,
            &accepted,
            incoming.entity_revision,
            incoming,
            None,
        ),
        None => RevisionOrder::Next,
    }
}

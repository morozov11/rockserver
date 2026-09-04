//! Transport-independent domain model for device-control protocol v1.
//!
//! The public facade preserves the established domain API while private modules isolate protocol
//! identities, static manifests, runtime state, command payloads, lifecycle values, persistence,
//! and shared validation.

#[path = "device_control/command.rs"]
mod command;
#[path = "device_control/foundation.rs"]
mod foundation;
#[path = "device_control/lifecycle.rs"]
mod lifecycle;
#[path = "device_control/manifest.rs"]
mod manifest;
#[path = "device_control/revision.rs"]
mod revision;
#[path = "device_control/state.rs"]
mod state;
#[path = "device_control/store.rs"]
mod store;
#[path = "device_control/validation.rs"]
mod validation;

pub use command::*;
pub use foundation::*;
pub use lifecycle::*;
pub use manifest::*;
pub use revision::*;
pub use state::*;
pub use store::*;

#[cfg(test)]
#[path = "device_control/tests.rs"]
mod tests;

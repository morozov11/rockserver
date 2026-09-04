//! Focused protocol-domain regression tests.

use super::*;
use serde_json::Value;
use uuid::Uuid;
fn fixture(name: &str) -> Value {
    serde_json::from_str(match name {
        "rockcast" => {
            include_str!("../../tests/fixtures/device-control/v1/rockcast-register-client.json")
        }
        "esp32" => {
            include_str!("../../tests/fixtures/device-control/v1/esp32-manifest-client.json")
        }
        "grid" => include_str!(
            "../../tests/fixtures/device-control/v1/display-sensor-grid-command-server.json"
        ),
        "unknown" => {
            include_str!("../../tests/fixtures/device-control/v1/unknown-command-client.json")
        }
        "invalid" => include_str!(
            "../../tests/fixtures/device-control/v1/invalid-sensor-unit-value-client.json"
        ),
        _ => unreachable!(),
    })
    .unwrap()
}
#[test]
fn fixtures_round_trip_domain() {
    for name in ["rockcast", "esp32"] {
        let v = fixture(name);
        let manifest: DeviceManifest =
            serde_json::from_value(v["payload"]["manifest"].clone()).unwrap();
        manifest.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&manifest).unwrap(),
            v["payload"]["manifest"]
        );
    }
}
#[test]
fn extension_and_command_are_safe() {
    let cap: DeviceCapability = serde_json::from_str(include_str!(
        "../../tests/fixtures/device-control/v1/unknown-capability.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(&cap).unwrap()["metric"], "pm25");
    let command: DeviceCommand =
        serde_json::from_value(fixture("unknown")["payload"].clone()).unwrap();
    assert_eq!(
        command.executable(),
        Err(ValidationError::UnsupportedCommand)
    );
    assert!(serde_json::from_str::<DeviceCapability>(r#"{"name":"bad","version":1}"#).is_err());
}
#[test]
fn state_freshness_units_and_revision_are_deterministic() {
    let manifest: DeviceManifest =
        serde_json::from_value(fixture("esp32")["payload"]["manifest"].clone()).unwrap();
    let entity = &manifest.entities[0];
    let state: EntityState =
        serde_json::from_value(fixture("invalid")["payload"]["state"].clone()).unwrap();
    assert_eq!(
        state.validate_for(entity).unwrap_err().code(),
        "invalid_payload"
    );
    let valid: EntityState = serde_json::from_str(include_str!(
        "../../tests/fixtures/device-control/v1/ha-normalized-entity-state.json"
    ))
    .unwrap();
    assert_eq!(
        valid.freshness_at(&Timestamp::parse("2026-09-02T12:05:00Z").unwrap()),
        Freshness::Fresh
    );
    assert_eq!(
        valid.freshness_at(&Timestamp::parse("2026-09-02T12:07:00Z").unwrap()),
        Freshness::Stale
    );
    let x = 1;
    assert_eq!(revision_order(2, &x, 2, &x, None), RevisionOrder::Replay);
    assert_eq!(revision_order(2, &x, 4, &x, Some(2)), RevisionOrder::Gap);
}
#[test]
fn presentation_command_and_terminal_invariants() {
    let command: DeviceCommand =
        serde_json::from_value(fixture("grid")["payload"].clone()).unwrap();
    command
        .validate_at(&Timestamp::parse("2026-09-02T12:02:00Z").unwrap())
        .unwrap();
    let id = CommandId(Uuid::nil());
    let t = Timestamp::parse("2026-09-02T12:02:00Z").unwrap();
    assert!(
        CommandResult {
            command_id: id,
            status: CommandStatus::Succeeded,
            completed_at: t,
            error: Some(DomainError {
                code: "x".into(),
                message: "x".into()
            })
        }
        .validate()
        .is_err()
    );
}

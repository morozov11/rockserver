//! Golden HA bridge frames remain isolated from paired-device credentials and commands.

use super::common::{OPENAPI, assert_component_invalid, assert_component_valid};
use serde_json::Value;
use std::{fs, path::Path};

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ha-bridge/v1")
        .join(name);
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn separate_ha_contract_accepts_readings_and_rejects_identity_or_commands() {
    let yaml: serde_yaml::Value = serde_yaml::from_str(OPENAPI).unwrap();
    let document = serde_json::to_value(yaml).unwrap();
    let route = &document["paths"]["/api/v1/integrations/home-assistant/connect"]["get"];
    assert_eq!(
        route["security"][0]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap(),
        "HomeAssistantBridgeBearer"
    );
    assert_component_valid(
        &document,
        "HaBridgeSensorSnapshot",
        &fixture("snapshot.json"),
    );
    assert_component_valid(&document, "HaBridgeWeather", &fixture("weather.json"));
    assert_component_invalid(
        &document,
        "HaBridgeSensorSnapshot",
        &fixture("invalid-foreign-identity.json"),
        "client identity forbidden",
    );
    assert_component_invalid(
        &document,
        "HaBridgeSensorState",
        &fixture("invalid-actuator.json"),
        "actuator forbidden",
    );
    let overview = &document["paths"]["/api/v1/browser/home-assistant"]["get"];
    assert_eq!(overview["x-rockserver-status"], "implemented");
    let selection = &document["paths"]["/api/v1/browser/home-assistant/selection"]["put"];
    assert_eq!(selection["x-rockserver-status"], "implemented");
    assert!(selection["responses"]["204"]["description"].is_string());
    assert_component_valid(&document, "HaBridgeOverview", &fixture("overview.json"));
    assert_component_invalid(
        &document,
        "HaBridgeOverview",
        &fixture("invalid-native-overview.json"),
        "native HA identifiers are private",
    );
    assert_component_valid(
        &document,
        "HaBridgeProvisionRequest",
        &serde_json::json!({"sensors": ["sensor.room_temperature"], "weather_entity": "weather.home"}),
    );
    assert_component_invalid(
        &document,
        "HaBridgeProvisionRequest",
        &serde_json::json!({"sensors": ["switch.kitchen"], "weather_entity": "weather.home"}),
        "only sensor readings may be published",
    );
}

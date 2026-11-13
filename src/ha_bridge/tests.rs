use super::*;
use std::collections::HashSet;

fn principal() -> BridgePrincipal {
    BridgePrincipal {
        user_id: Uuid::new_v4(),
        integration_id: Uuid::new_v4(),
        credential_hash: credential_hash("ha1.test"),
        sensors: HashSet::from(["sensor.room_temperature".to_owned()]),
        weather_entity: "weather.home".to_owned(),
    }
}

fn sensor(value: Option<f64>, quality: ReadingQuality) -> SensorReading {
    SensorReading {
        entity_id: "sensor.room_temperature".to_owned(),
        display_name: "Room temperature".to_owned(),
        value,
        unit: Some("°C".to_owned()),
        device_class: Some("temperature".to_owned()),
        quality,
        observed_at: "2026-11-12T12:00:00Z".to_owned(),
    }
}

#[test]
fn sensors_reject_foreign_duplicate_and_misleading_values() {
    let owner = principal();
    assert!(validate_sensors(
        &owner,
        &[sensor(Some(22.5), ReadingQuality::Ok)]
    ));
    assert!(validate_sensors(
        &owner,
        &[sensor(None, ReadingQuality::Unavailable)]
    ));
    assert!(!validate_sensors(
        &owner,
        &[sensor(Some(0.0), ReadingQuality::Unavailable)]
    ));
    assert!(!validate_sensors(
        &owner,
        &[sensor(None, ReadingQuality::Ok)]
    ));
    let repeated = sensor(Some(1.0), ReadingQuality::Ok);
    assert!(!validate_sensors(&owner, &[repeated.clone(), repeated]));
    let mut foreign = sensor(Some(22.5), ReadingQuality::Ok);
    foreign.entity_id = "sensor.neighbour_temperature".to_owned();
    assert!(!validate_sensors(&owner, &[foreign]));
    let mut unsupported = sensor(Some(22.5), ReadingQuality::Ok);
    unsupported.unit = Some("psi".to_owned());
    assert!(!validate_sensors(&owner, &[unsupported]));
}

#[test]
fn weather_forecast_and_current_age_are_independent() {
    let now = OffsetDateTime::parse("2026-11-12T13:00:00Z", &Rfc3339).unwrap();
    let mut reading = WeatherReading {
        entity_id: "weather.home".to_owned(),
        condition: Some("cloudy".to_owned()),
        temperature: Some(8.0),
        temperature_unit: Some("°C".to_owned()),
        humidity: None,
        wind_speed: None,
        wind_speed_unit: None,
        quality: ReadingQuality::Ok,
        observed_at: "2026-11-12T12:55:00Z".to_owned(),
        forecast_observed_at: Some("2026-11-12T12:10:00Z".to_owned()),
        forecast: vec![ForecastDay {
            datetime: "2026-11-13T00:00:00Z".to_owned(),
            condition: None,
            temperature: Some(9.0),
            templow: None,
            precipitation: None,
        }],
    };
    assert!(validate_weather(&principal(), &reading));
    assert_eq!(
        weather_status(&reading, "2026-11-12T12:56:00Z", now),
        "fresh"
    );
    assert_eq!(forecast_status(&reading, now), "stale");
    assert_eq!(connected_status("fresh", false), "stale");
    assert_eq!(connected_status("unavailable", false), "unavailable");
    reading.quality = ReadingQuality::Unavailable;
    reading.temperature = None;
    reading.forecast.clear();
    assert_eq!(
        weather_status(&reading, "2026-11-12T12:56:00Z", now),
        "unavailable"
    );
}

#[test]
fn sensor_age_quality_and_connection_determine_freshness() {
    let now = OffsetDateTime::parse("2026-11-12T13:00:00Z", &Rfc3339).unwrap();
    let mut reading = sensor(Some(21.5), ReadingQuality::Ok);
    reading.observed_at = "2026-11-12T12:55:00Z".to_owned();
    assert_eq!(
        sensor_status(&reading, "2026-11-12T12:56:00Z", now, true),
        "fresh"
    );
    assert_eq!(
        sensor_status(&reading, "2026-11-12T12:56:00Z", now, false),
        "stale"
    );
    reading.observed_at = "2026-11-12T12:00:00Z".to_owned();
    assert_eq!(
        sensor_status(&reading, "2026-11-12T12:56:00Z", now, true),
        "stale"
    );
    reading.quality = ReadingQuality::Unavailable;
    assert_eq!(
        sensor_status(&reading, "2026-11-12T12:56:00Z", now, true),
        "unavailable"
    );
}

#[test]
fn separate_credentials_have_distinct_hashes() {
    assert_ne!(
        credential_hash("ha1.bridge-secret"),
        credential_hash("native-device-secret")
    );
}

#[test]
fn owner_selection_accepts_only_unique_bounded_sensor_ids_and_one_weather_id() {
    let sensors = vec!["sensor.room_temperature".to_owned()];
    assert!(valid_selection(&sensors, "weather.home"));
    assert!(!valid_selection(
        &[sensors[0].clone(), sensors[0].clone()],
        "weather.home"
    ));
    assert!(!valid_selection(
        &["switch.lamp".to_owned()],
        "weather.home"
    ));
    assert!(!valid_selection(&sensors, "sensor.outside"));
    assert!(!valid_selection(
        &["sensor.room.extra".to_owned()],
        "weather.home"
    ));
    assert!(!valid_selection(
        &["sensor.room\nother".to_owned()],
        "weather.home"
    ));
}

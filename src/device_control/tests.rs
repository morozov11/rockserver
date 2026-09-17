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
                message: "x".into(),
                request_id: "test".into(),
                details: Default::default(),
            })
        }
        .validate()
        .is_err()
    );
}

#[test]
fn station_play_stream_variants_round_trip_and_enforce_their_shapes() {
    let resolved: DeviceCommand = serde_json::from_str(
        r#"{"command_id":"00000000-0000-4000-8000-000000000001","target":{"device_id":"00000000-0000-4000-8000-000000000002"},"deadline_at":"2026-09-02T12:02:10Z","body":{"name":"station.play_stream","source":"rockserver_catalog","station_id":"station.jazz_fixture","station":{"name":"Fixture Jazz","icon_url":null},"stream_uri":"https://streams.example.com/quiet-jazz.mp3"}}"#,
    )
    .unwrap();
    assert_eq!(
        resolved.body,
        CommandBody::PlayStream {
            source: StreamSource::RockserverCatalog,
            station_id: Some("station.jazz_fixture".into()),
            station: Some(StationPresentation {
                name: "Fixture Jazz".into(),
                icon_url: None,
            }),
            stream_uri: "https://streams.example.com/quiet-jazz.mp3".into(),
        }
    );
    assert_eq!(
        serde_json::to_value(&resolved).unwrap()["body"],
        serde_json::json!({"name":"station.play_stream","source":"rockserver_catalog","station_id":"station.jazz_fixture","station":{"name":"Fixture Jazz","icon_url":null},"stream_uri":"https://streams.example.com/quiet-jazz.mp3"})
    );
    resolved
        .validate_at(&Timestamp::parse("2026-09-02T12:02:00Z").unwrap())
        .unwrap();
    assert!(resolved.executable().is_ok());

    let direct: DeviceCommand = serde_json::from_str(
        r#"{"command_id":"00000000-0000-4000-8000-000000000001","target":{"device_id":"00000000-0000-4000-8000-000000000002"},"body":{"name":"station.play_stream","source":"direct_stream","stream_uri":"https://streams.example.com/highway-rock.aac"}}"#,
    )
    .unwrap();
    assert_eq!(
        direct.body,
        CommandBody::PlayStream {
            source: StreamSource::DirectStream,
            station_id: None,
            station: None,
            stream_uri: "https://streams.example.com/highway-rock.aac".into(),
        }
    );

    // Catalog must echo an ID; direct streams cannot claim catalog presentation.
    for broken in [
        r#"{"name":"station.play_stream","source":"rockserver_catalog","stream_uri":"https://streams.example.com/x.mp3"}"#,
        r#"{"name":"station.play_stream","source":"direct_stream","station_id":"station.jazz_fixture","stream_uri":"https://streams.example.com/x.mp3"}"#,
        r#"{"name":"station.play_stream","source":"direct_stream","station":{"name":"Fixture Jazz","icon_url":null},"stream_uri":"https://streams.example.com/x.mp3"}"#,
        r#"{"name":"station.play_stream","stream_uri":"https://streams.example.com/x.mp3"}"#,
        r#"{"name":"station.play_stream","source":"torrent","stream_uri":"https://streams.example.com/x.mp3"}"#,
    ] {
        assert!(
            serde_json::from_str::<CommandBody>(broken).is_err(),
            "shape must be rejected: {broken}"
        );
    }

    assert!(
        StationPresentation {
            name: "".into(),
            icon_url: None,
        }
        .validate()
        .is_err()
    );
    assert!(
        StationPresentation {
            name: "Fixture Jazz".into(),
            icon_url: Some("file:///not-an-icon.png".into()),
        }
        .validate()
        .is_err()
    );
    StationPresentation {
        name: "Fixture Jazz".into(),
        icon_url: Some("https://icons.example.com/jazz.png".into()),
    }
    .validate()
    .unwrap();
}

#[test]
fn stream_uri_literal_validation_follows_the_station_contract() {
    for allowed in [
        "https://streams.example.com/quiet-jazz.mp3",
        "http://93.184.216.34:8080/live.mp3",
        "https://[2606:2800:220:1:248:1893:25c8:1946]/stream",
        "https://example.com",
        "http://example.com:65535/path?bitrate=128",
    ] {
        validate_stream_uri(allowed).unwrap_or_else(|rejection| {
            panic!("public destination {allowed} must pass: {rejection:?}")
        });
    }
    let forbidden_destination = [
        "https://127.0.0.1/x",
        "https://10.0.0.1/x",
        "https://172.16.0.1/x",
        "https://172.31.255.255/x",
        "https://192.168.1.1/x",
        "https://169.254.169.254/x",
        "https://100.64.0.1/x",
        "https://0.0.0.0/x",
        "https://192.0.2.1/x",
        "https://198.51.100.7/x",
        "https://203.0.113.9/x",
        "https://198.18.0.1/x",
        "https://224.0.0.1/x",
        "https://240.0.0.1/x",
        "https://[::1]/x",
        "https://[::]/x",
        "https://[fe80::1]/x",
        "https://[fc00::1]/x",
        "https://[ff02::1]/x",
        "https://[::ffff:127.0.0.1]/x",
        "https://[::192.0.2.1]/x",
        "https://localhost/x",
        "https://LOCALHOST:8443/x",
        "https://2130706433/x",
        "https://streams.example.com%2Fevil/x",
    ];
    let oversized = format!("https://streams.example.com/{}.mp3", "a".repeat(2048));
    let other_rejections = [
        (
            "ftp://streams.example.com/x",
            StreamUriRejection::NotAbsoluteHttp,
        ),
        (
            "HTTPS://streams.example.com/x",
            StreamUriRejection::NotAbsoluteHttp,
        ),
        ("streams.example.com/x", StreamUriRejection::NotAbsoluteHttp),
        ("https://", StreamUriRejection::MissingHost),
        ("https:///path", StreamUriRejection::MissingHost),
        (
            "https://user:pw@example.com/x",
            StreamUriRejection::Userinfo,
        ),
        (
            "https://example.com/x#fragment",
            StreamUriRejection::Fragment,
        ),
        ("https://example.com:0/x", StreamUriRejection::InvalidPort),
        (
            "https://example.com:65536/x",
            StreamUriRejection::InvalidPort,
        ),
        (
            "https://example.com:port/x",
            StreamUriRejection::InvalidPort,
        ),
        ("https://example.com:/x", StreamUriRejection::InvalidPort),
        (oversized.as_str(), StreamUriRejection::TooLong),
    ];
    for uri in forbidden_destination {
        assert_eq!(
            validate_stream_uri(uri),
            Err(StreamUriRejection::ForbiddenDestination),
            "non-public destination must be rejected: {uri}"
        );
    }
    for (uri, expected) in other_rejections {
        assert_eq!(validate_stream_uri(uri), Err(expected), "in {uri}");
    }
}

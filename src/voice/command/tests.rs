use serde_json::json;

use super::*;

#[test]
fn serde_round_trip_and_schema_are_strict() {
    let command: VoiceCommand = serde_json::from_str(r#"{"intent":"next_station"}"#).unwrap();
    assert_eq!(command.intent, Intent::NextStation);
    assert!(serde_json::from_str::<VoiceCommand>(r#"{"intent":"stop","extra":true}"#).is_err());
    let schema = voice_command_request("Включи джаз", "ru-RU");
    assert_eq!(schema.response_schema()["additionalProperties"], false);
    assert!(
        schema.response_schema()["properties"]["intent"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("volume_change"))
    );
}

#[test]
fn resume_and_add_favorite_round_trip_through_schema_and_serde() {
    for (payload, expected) in [
        (r#"{"intent":"resume"}"#, Intent::Resume),
        (r#"{"intent":"add_favorite"}"#, Intent::AddFavorite),
    ] {
        let command: VoiceCommand = serde_json::from_str(payload).unwrap();
        assert_eq!(command.intent, expected);
        assert!(command.query.is_none() && command.volume_delta.is_none());
    }
    let schema = voice_command_request("play that again", "en-US");
    let intents = schema.response_schema()["properties"]["intent"]["enum"]
        .as_array()
        .unwrap();
    assert!(intents.contains(&json!("resume")));
    assert!(intents.contains(&json!("add_favorite")));
}

#[test]
fn resume_and_add_favorite_reject_payload_fields() {
    assert!(
        VoiceCommand {
            intent: Intent::Resume,
            query: Some(RadioQuery {
                genres: vec!["rock".into()],
                ..RadioQuery::default()
            }),
            volume_delta: None,
            volume_level: None,
        }
        .validate()
        .is_err()
    );
    assert!(
        VoiceCommand {
            intent: Intent::AddFavorite,
            query: None,
            volume_delta: Some(5),
            volume_level: None,
        }
        .validate()
        .is_err()
    );
    assert!(control_command(Intent::Resume).validate().is_ok());
    assert!(control_command(Intent::AddFavorite).validate().is_ok());
}

#[test]
fn semantic_validation_normalizes_play_and_rejects_invalid_combinations() {
    let command = VoiceCommand {
        intent: Intent::PlayRadio,
        query: Some(RadioQuery {
            genres: vec![" Jazz ".into(), "jazz".into()],
            moods: vec![" Calm ".into()],
            language: Some("RU".into()),
            ..RadioQuery::default()
        }),
        volume_delta: None,
        volume_level: None,
    }
    .validate()
    .unwrap();
    assert_eq!(command.query.unwrap().genres, ["jazz"]);
    assert!(
        VoiceCommand {
            intent: Intent::NextStation,
            query: Some(RadioQuery::default()),
            volume_delta: None,
            volume_level: None,
        }
        .validate()
        .is_err()
    );
    assert!(
        VoiceCommand {
            intent: Intent::VolumeChange,
            query: None,
            volume_delta: Some(0),
            volume_level: None,
        }
        .validate()
        .is_err()
    );
}

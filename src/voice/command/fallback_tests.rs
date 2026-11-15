use std::sync::Arc;

use async_trait::async_trait;

use super::*;
use crate::providers::llm::{LlmClientError, LlmService};
use crate::search::QueryIntent;
use crate::search::taxonomy::GenreTaxonomy;

struct FailingService;

#[async_trait]
impl LlmService for FailingService {
    async fn parse_intent(
        &self,
        _query: &str,
        _locale: &str,
        _taxonomy: &GenreTaxonomy,
    ) -> Result<QueryIntent, LlmClientError> {
        Err(LlmClientError::safe("provider unavailable"))
    }

    async fn parse_command(
        &self,
        _transcript: &str,
        _locale: &str,
    ) -> Result<VoiceCommand, LlmClientError> {
        Err(LlmClientError::safe("provider unavailable"))
    }

    async fn languages(&self) -> Result<Vec<String>, LlmClientError> {
        Err(LlmClientError::safe("provider unavailable"))
    }
}

struct MalformedService;

#[async_trait]
impl LlmService for MalformedService {
    async fn parse_intent(
        &self,
        _query: &str,
        _locale: &str,
        _taxonomy: &GenreTaxonomy,
    ) -> Result<QueryIntent, LlmClientError> {
        Err(LlmClientError::safe(
            "invalid model output: schema mismatch",
        ))
    }

    async fn parse_command(
        &self,
        _transcript: &str,
        _locale: &str,
    ) -> Result<VoiceCommand, LlmClientError> {
        Err(LlmClientError::safe(
            "invalid model output: schema mismatch",
        ))
    }

    async fn languages(&self) -> Result<Vec<String>, LlmClientError> {
        Ok(vec![])
    }
}

struct UnknownService;

#[async_trait]
impl LlmService for UnknownService {
    async fn parse_intent(
        &self,
        _query: &str,
        _locale: &str,
        _taxonomy: &GenreTaxonomy,
    ) -> Result<QueryIntent, LlmClientError> {
        Ok(QueryIntent {
            action: crate::search::SearchAction::Play,
            terms: vec![],
            tags: vec![],
            language: None,
            country_code: None,
            core_term_count: 0,
            raw_query: String::new(),
        })
    }

    async fn parse_command(
        &self,
        _transcript: &str,
        _locale: &str,
    ) -> Result<VoiceCommand, LlmClientError> {
        Ok(VoiceCommand {
            intent: Intent::Unknown,
            query: None,
            volume_delta: None,
            volume_level: None,
        })
    }

    async fn languages(&self) -> Result<Vec<String>, LlmClientError> {
        Ok(vec!["en".into(), "ru".into()])
    }
}

#[tokio::test]
async fn provider_failure_preserves_basic_rock_command() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingService));
    let command = interpreter.interpret("включи рок", "ru").await.unwrap();
    assert_eq!(command.intent, Intent::PlayRadio);
    assert_eq!(command.query.unwrap().genres, ["rock"]);
}

#[tokio::test]
async fn provider_failure_preserves_black_metal_command() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingService));
    for transcript in ["Включи блэк метал", "Включи блэк металл"] {
        let command = interpreter.interpret(transcript, "ru").await.unwrap();
        assert_eq!(command.intent, Intent::PlayRadio);
        assert_eq!(command.query.unwrap().genres, ["metal"]);
    }
}

#[tokio::test]
async fn malformed_provider_response_preserves_stop_command() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(MalformedService));
    let command = interpreter.interpret("останови", "ru").await.unwrap();
    assert_eq!(command.intent, Intent::Stop);
}

#[tokio::test]
async fn provider_failure_falls_back_for_volume_phrases_end_to_end() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingService));

    // Volume up: «громче» -> VolumeChange(5)
    let louder = interpreter.interpret("громче", "ru").await.unwrap();
    assert_eq!(louder.intent, Intent::VolumeChange);
    assert_eq!(louder.volume_delta, Some(5));

    // Volume down: «тише» -> VolumeChange(-5)
    let quieter = interpreter.interpret("тише", "ru").await.unwrap();
    assert_eq!(quieter.intent, Intent::VolumeChange);
    assert_eq!(quieter.volume_delta, Some(-5));

    // Absolute volume: "set volume to 50" -> SetVolume(50)
    let set_vol = interpreter
        .interpret("set volume to 50", "en")
        .await
        .unwrap();
    assert_eq!(set_vol.intent, Intent::SetVolume);
    assert_eq!(set_vol.volume_level, Some(50));
}

#[tokio::test]
async fn provider_failure_preserves_unsupported_commands_as_unknown() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingService));
    let command = interpreter
        .interpret("какая температура", "ru")
        .await
        .unwrap();
    assert_eq!(command.intent, Intent::Unknown);
}

#[tokio::test]
async fn provider_failure_preserves_core_device_commands() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingService));
    for (transcript, expected) in [
        ("роксервер стоп", Intent::Stop),
        ("роксервер старт", Intent::Resume),
        ("роксервер лайк", Intent::AddFavorite),
    ] {
        let command = interpreter.interpret(transcript, "ru").await.unwrap();
        assert_eq!(command.intent, expected, "transcript: {transcript}");
    }
}

#[tokio::test]
async fn unknown_verdict_is_rescued_for_basic_commands() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(UnknownService));
    for (transcript, expected) in [
        ("роксервер стоп", Intent::Stop),
        ("роксервер старт", Intent::Resume),
        ("роксервер лайк", Intent::AddFavorite),
    ] {
        let command = interpreter.interpret(transcript, "ru").await.unwrap();
        assert_eq!(command.intent, expected, "transcript: {transcript}");
    }
    let rock = interpreter.interpret("роксервер рок", "ru").await.unwrap();
    assert_eq!(rock.intent, Intent::PlayRadio);
    assert_eq!(rock.query.unwrap().genres, ["rock"]);
}

#[tokio::test]
async fn unknown_verdict_stands_for_unrelated_speech() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(UnknownService));
    let command = interpreter.interpret("какая погода", "ru").await.unwrap();
    assert_eq!(command.intent, Intent::Unknown);
}

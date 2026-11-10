use super::*;
use crate::search::LlmProviderError;

struct FailingProvider;

#[async_trait]
impl LlmProvider for FailingProvider {
    async fn generate_json(&self, _request: &LlmRequest) -> Result<String, LlmProviderError> {
        Err(LlmProviderError::safe("provider unavailable"))
    }
}

struct MalformedProvider;

#[async_trait]
impl LlmProvider for MalformedProvider {
    async fn generate_json(&self, _request: &LlmRequest) -> Result<String, LlmProviderError> {
        Ok("{}".to_owned())
    }
}

struct UnknownProvider;

#[async_trait]
impl LlmProvider for UnknownProvider {
    async fn generate_json(&self, _request: &LlmRequest) -> Result<String, LlmProviderError> {
        Ok(
            r#"{"intent":"unknown","query":null,"volume_delta":null,"volume_level":null}"#
                .to_owned(),
        )
    }
}

#[tokio::test]
async fn provider_failure_preserves_basic_rock_command() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingProvider));
    let command = interpreter.interpret("включи рок", "ru").await.unwrap();
    assert_eq!(command.intent, Intent::PlayRadio);
    assert_eq!(command.query.unwrap().genres, ["rock"]);
}

#[tokio::test]
async fn provider_failure_preserves_black_metal_command() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingProvider));
    for transcript in ["Включи блэк метал", "Включи блэк металл"] {
        let command = interpreter.interpret(transcript, "ru").await.unwrap();
        assert_eq!(command.intent, Intent::PlayRadio);
        assert_eq!(command.query.unwrap().genres, ["metal"]);
    }
}

#[tokio::test]
async fn malformed_provider_response_preserves_stop_command() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(MalformedProvider));
    let command = interpreter.interpret("останови", "ru").await.unwrap();
    assert_eq!(command.intent, Intent::Stop);
}

#[tokio::test]
async fn provider_failure_does_not_turn_unrelated_speech_into_radio() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingProvider));
    assert!(interpreter.interpret("какая погода", "ru").await.is_err());
}

#[tokio::test]
async fn provider_failure_preserves_core_device_commands() {
    let interpreter = LlmCommandInterpreter::new(Arc::new(FailingProvider));
    for (transcript, expected) in [
        ("роксервер стоп", Intent::Stop),
        ("роксервер старт", Intent::Resume),
        ("роксервер лайк", Intent::AddFavorite),
    ] {
        let command = interpreter.interpret(transcript, "ru").await.unwrap();
        assert_eq!(command.intent, expected, "transcript: {transcript}");
    }
    let louder = interpreter
        .interpret("роксервер громче", "ru")
        .await
        .unwrap();
    assert_eq!(louder.intent, Intent::VolumeChange);
    assert_eq!(louder.volume_delta, Some(5));
    let quieter = interpreter.interpret("роксервер тише", "ru").await.unwrap();
    assert_eq!(quieter.intent, Intent::VolumeChange);
    assert_eq!(quieter.volume_delta, Some(-5));
}

#[tokio::test]
async fn unknown_verdict_is_rescued_for_basic_commands() {
    // Measured on device: the provider answers unknown for valid wake-word
    // prefixed commands often enough that every device session failed with
    // unsupported_intent. A recognized basic command must rescue the session.
    let interpreter = LlmCommandInterpreter::new(Arc::new(UnknownProvider));
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
    let interpreter = LlmCommandInterpreter::new(Arc::new(UnknownProvider));
    let command = interpreter.interpret("какая погода", "ru").await.unwrap();
    assert_eq!(command.intent, Intent::Unknown);
}

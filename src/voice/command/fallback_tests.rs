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

//! Opt-in live integration coverage for Yandex structured voice commands.

use std::sync::{Arc, Once};

use rockserver::{
    providers::yandex_llm::YandexLlmProvider,
    voice_command::{CommandInterpreter, Intent, LlmCommandInterpreter, voice_command_request},
};

static LOGGING: Once = Once::new();

#[tokio::test]
#[ignore = "requires real Yandex AI Studio credentials and performs a billable network request"]
async fn interprets_real_voice_command_with_safe_logs() {
    assert_command("Включи спокойный джаз", Intent::PlayRadio).await;
}

#[tokio::test]
#[ignore = "requires real Yandex AI Studio credentials and performs a billable network request"]
async fn interprets_real_russian_rock_command_with_safe_logs() {
    assert_command("Включи русский рок", Intent::PlayRadio).await;
}

#[tokio::test]
#[ignore = "requires real Yandex AI Studio credentials and performs a billable network request"]
async fn interprets_real_next_station_command_with_safe_logs() {
    assert_command("Следующая станция", Intent::NextStation).await;
}

#[tokio::test]
#[ignore = "requires real Yandex AI Studio credentials and performs a billable network request"]
async fn interprets_real_volume_command_with_safe_logs() {
    assert_command("Сделай громче", Intent::VolumeChange).await;
}

async fn assert_command(phrase: &str, expected: Intent) {
    init_logging();
    dotenvy::dotenv().ok();
    let provider = YandexLlmProvider::optional_from_env()
        .expect("Yandex LLM environment configuration must be valid")
        .expect("YANDEX_AI_API_KEY and YANDEX_FOLDER_ID must be set for this live test");
    let request = voice_command_request(phrase, "ru-RU");
    eprintln!(
        "Yandex LLM live request: method=POST endpoint={} authorization=Api-Key [REDACTED] request_body={}",
        provider.endpoint(),
        provider.safe_request_body(&request)
    );
    let interpreter = LlmCommandInterpreter::new(Arc::new(provider));
    let command = interpreter
        .interpret(phrase, "ru-RU")
        .await
        .expect("Yandex must return a valid structured voice command");
    eprintln!("Yandex LLM deserialized VoiceCommand: {command:?}");
    assert_eq!(command.intent, expected);
}

fn init_logging() {
    LOGGING.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("rockserver::providers::yandex_llm=debug")
            .try_init();
    });
}

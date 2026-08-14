//! Opt-in live integration coverage for pre-recorded SpeechKit recognition.
//!
//! The test never runs as part of the ordinary suite and emits only safe operational metadata.

use std::{
    env, fs,
    sync::{Once, OnceLock},
    time::Instant,
};

use reqwest::{StatusCode, header};
use serde::Deserialize;

const SPEECHKIT_RECOGNIZE_URL: &str = "https://stt.api.cloud.yandex.net/speech/v1/stt:recognize";
const MAX_AUDIO_BYTES: usize = 1_024 * 1_024;
const MAX_RESPONSE_BYTES: usize = 16 * 1_024;
const TEST_AUDIO_PATH_ENV: &str = "TEST_YANDEX_STT_AUDIO_PATH";
const TEST_EXPECTED_TRANSCRIPT_ENV: &str = "TEST_YANDEX_STT_EXPECTED_TRANSCRIPT";
const TEST_LOCALE_ENV: &str = "TEST_YANDEX_STT_LOCALE";

static LOGGING: Once = Once::new();
static TEST_ID: OnceLock<String> = OnceLock::new();

/// Sends a real, pre-recorded Ogg/Opus voice command to Yandex SpeechKit.
///
/// Run explicitly with the required environment variables; this test performs a billable network
/// request and therefore stays ignored in the default suite. It never logs credentials, audio,
/// the recognized transcript, or the expected transcript.
#[tokio::test]
#[ignore = "requires real SpeechKit credentials and a local Ogg/Opus voice recording"]
async fn recognizes_real_ogg_opus_voice_command_with_safe_logs() {
    init_test_logging();
    dotenvy::dotenv().ok();

    let api_key = required_env("YANDEX_AI_API_KEY");
    let folder_id = required_env("YANDEX_FOLDER_ID");
    let audio_path = required_env(TEST_AUDIO_PATH_ENV);
    let expected_transcript = required_env(TEST_EXPECTED_TRANSCRIPT_ENV);
    let locale = env::var(TEST_LOCALE_ENV).unwrap_or_else(|_| "ru-RU".to_owned());
    let audio = fs::read(&audio_path)
        .unwrap_or_else(|_| panic!("{TEST_AUDIO_PATH_ENV} must name a readable file"));

    assert!(
        audio_path.to_ascii_lowercase().ends_with(".ogg"),
        "{TEST_AUDIO_PATH_ENV} must point to a mono Ogg/Opus recording"
    );
    assert!(
        !audio.is_empty() && audio.len() <= MAX_AUDIO_BYTES,
        "voice recording must contain 1 through {MAX_AUDIO_BYTES} bytes"
    );
    assert!(
        !expected_transcript.trim().is_empty(),
        "{TEST_EXPECTED_TRANSCRIPT_ENV} must not be empty"
    );

    let audio_bytes = audio.len();
    let started = Instant::now();
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("the SpeechKit test HTTP client must build")
        .post(SPEECHKIT_RECOGNIZE_URL)
        .header(header::AUTHORIZATION, format!("Api-Key {api_key}"))
        .query(&[
            ("topic", "general"),
            ("lang", locale.as_str()),
            ("folderId", folder_id.as_str()),
            ("format", "oggopus"),
        ])
        .body(audio)
        .send()
        .await
        .unwrap_or_else(|_| panic!("SpeechKit live recognition request failed"));
    let status = response.status();
    tracing::info!(
        test_id = test_id(),
        status = status.as_u16(),
        audio_bytes,
        elapsed_ms = started.elapsed().as_millis(),
        "SpeechKit live recognition response received"
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "SpeechKit returned HTTP {}",
        status.as_u16()
    );

    let response_bytes = response
        .bytes()
        .await
        .expect("SpeechKit response body must be readable");
    assert!(
        response_bytes.len() <= MAX_RESPONSE_BYTES,
        "SpeechKit response exceeded the safe test limit"
    );
    let payload = serde_json::from_slice::<RecognitionResponse>(&response_bytes)
        .expect("SpeechKit success response must be JSON with a result field");
    let transcript = payload
        .result
        .filter(|value| !value.trim().is_empty())
        .expect("SpeechKit success response must contain a non-empty result");
    let matches_expected = normalize(&transcript).contains(&normalize(&expected_transcript));
    tracing::info!(
        test_id = test_id(),
        response_bytes = response_bytes.len(),
        recognized_characters = transcript.chars().count(),
        matches_expected,
        "SpeechKit live recognition result validated"
    );
    assert!(
        matches_expected,
        "SpeechKit result did not include the expected command"
    );
}

#[derive(Deserialize)]
struct RecognitionResponse {
    result: Option<String>,
}

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set to run this live test"))
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn init_test_logging() {
    LOGGING.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("info")
            .try_init();
    });
}

fn test_id() -> &'static str {
    TEST_ID
        .get_or_init(|| format!("speechkit-live-{}", uuid::Uuid::new_v4()))
        .as_str()
}

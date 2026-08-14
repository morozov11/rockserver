//! Generates the committed SpeechKit Ogg/Opus voice-command fixture through Yandex TTS.
//!
//! This opt-in developer command reads its credentials only from the process environment or an
//! ignored local `.env` file. It deliberately never prints an authorization value or an upstream
//! response body.

use std::{env, error::Error, fs, path::Path, time::Duration};

use reqwest::{StatusCode, header};

const TTS_SYNTHESIZE_URL: &str = "https://tts.api.cloud.yandex.net/speech/v1/tts:synthesize";
const COMMAND_TEXT: &str = "Включи спокойный джаз";
const LANGUAGE: &str = "ru-RU";
const VOICE: &str = "filipp";
const MAX_AUDIO_BYTES: usize = 1_024 * 1_024;
const AUDIO_OUTPUT: &str = "tests/fixtures/speechkit/calm-jazz-command.ogg";
const TRANSCRIPT_OUTPUT: &str = "tests/fixtures/speechkit/calm-jazz-command.expected.txt";

/// Synthesizes the repository's opt-in SpeechKit recognition fixture.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let api_key = required_env("YANDEX_AI_API_KEY")?;
    required_env("YANDEX_FOLDER_ID")?;
    let started = std::time::Instant::now();
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?
        .post(TTS_SYNTHESIZE_URL)
        .header(header::AUTHORIZATION, format!("Api-Key {api_key}"))
        .form(&[("text", COMMAND_TEXT), ("lang", LANGUAGE), ("voice", VOICE)])
        .send()
        .await?;
    let status = response.status();
    if status != StatusCode::OK {
        return Err(format!("Yandex TTS returned HTTP {}", status.as_u16()).into());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUDIO_BYTES as u64)
    {
        return Err("Yandex TTS response exceeds the 1 MiB fixture limit".into());
    }

    let audio = read_bounded(response).await?;
    if !audio.starts_with(b"OggS") {
        return Err("Yandex TTS response is not an Ogg container".into());
    }

    let audio_path = Path::new(AUDIO_OUTPUT);
    let transcript_path = Path::new(TRANSCRIPT_OUTPUT);
    fs::create_dir_all(audio_path.parent().expect("fixture path has a parent"))?;
    fs::write(audio_path, &audio)?;
    fs::write(transcript_path, format!("{COMMAND_TEXT}\n"))?;
    println!(
        "Generated {} ({} bytes) and {}; HTTP {}; {} ms.",
        audio_path.display(),
        audio.len(),
        transcript_path.display(),
        status.as_u16(),
        started.elapsed().as_millis()
    );
    Ok(())
}

/// Reads a response body without allowing it to exceed the fixture size budget.
async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_AUDIO_BYTES {
            return Err("Yandex TTS response exceeds the 1 MiB fixture limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err("Yandex TTS response is empty".into());
    }
    Ok(bytes)
}

/// Reads a required non-empty environment variable without exposing its value.
fn required_env(name: &str) -> Result<String, Box<dyn Error>> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(format!("{name} must be configured").into()),
    }
}

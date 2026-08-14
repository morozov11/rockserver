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
const MAX_ERROR_BODY_BYTES: usize = 16 * 1_024;
const AUDIO_OUTPUT: &str = "tests/fixtures/speechkit/calm-jazz-command.ogg";
const TRANSCRIPT_OUTPUT: &str = "tests/fixtures/speechkit/calm-jazz-command.expected.txt";
const DEBUG_ENV: &str = "YANDEX_SPEECHKIT_DEBUG";

/// Synthesizes the repository's opt-in SpeechKit recognition fixture.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let api_key = required_env("YANDEX_AI_API_KEY")?;
    let debug = env::var(DEBUG_ENV).is_ok_and(|value| value == "1");
    if debug {
        eprintln!(
            "Yandex TTS request: method=POST url={TTS_SYNTHESIZE_URL} authorization=Api-Key [REDACTED] content_type=application/x-www-form-urlencoded text={COMMAND_TEXT:?} lang={LANGUAGE} voice={VOICE}"
        );
    }
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
    if debug {
        eprintln!(
            "Yandex TTS response: status={} headers={:?} elapsed_ms={}",
            status.as_u16(),
            response.headers(),
            started.elapsed().as_millis()
        );
    }
    if status != StatusCode::OK {
        if debug {
            let error_body = read_bounded_with_limit(response, MAX_ERROR_BODY_BYTES).await?;
            eprintln!(
                "Yandex TTS error body (redacted, {} byte limit): {}",
                MAX_ERROR_BODY_BYTES,
                redact(&String::from_utf8_lossy(&error_body))
            );
        }
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
async fn read_bounded(response: reqwest::Response) -> Result<Vec<u8>, Box<dyn Error>> {
    read_bounded_with_limit(response, MAX_AUDIO_BYTES).await
}

/// Reads a response body without allowing it to exceed the supplied byte limit.
async fn read_bounded_with_limit(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err("Yandex TTS response exceeds the configured byte limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err("Yandex TTS response is empty".into());
    }
    Ok(bytes)
}

/// Removes authorization-style values from diagnostic provider text before it is printed.
fn redact(value: &str) -> String {
    let mut sanitized = value.to_owned();
    for marker in ["Api-Key ", "Bearer "] {
        let mut search_start = 0;
        while let Some(offset) = sanitized[search_start..].find(marker) {
            let start = search_start + offset;
            let value_start = start + marker.len();
            let value_end = sanitized[value_start..]
                .find(|character: char| character.is_whitespace() || character == '"')
                .map_or(sanitized.len(), |offset| value_start + offset);
            sanitized.replace_range(value_start..value_end, "[REDACTED]");
            search_start = value_start + "[REDACTED]".len();
        }
    }
    sanitized
}

/// Reads a required non-empty environment variable without exposing its value.
fn required_env(name: &str) -> Result<String, Box<dyn Error>> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(format!("{name} must be configured").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn diagnostic_redaction_removes_each_authorization_value() {
        let value = "Api-Key first-token; Bearer second-token";

        let redacted = redact(value);

        assert!(!redacted.contains("first-token"));
        assert!(!redacted.contains("second-token"));
        assert_eq!(redacted, "Api-Key [REDACTED] Bearer [REDACTED]");
    }
}

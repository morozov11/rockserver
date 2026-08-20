//! Sends one Yandex TTS-generated PCM command through SpeechKit v1 for diagnosis.
//!
//! This opt-in utility keeps credentials local, prints no authorization values, and writes no
//! audio files. It isolates provider recognition from microphone, WebSocket, and search code.

use std::{
    env,
    error::Error,
    time::{Duration, Instant},
};

use reqwest::header;
use serde::Deserialize;

const TTS_URL: &str = "https://tts.api.cloud.yandex.net/speech/v1/tts:synthesize";
const STT_URL: &str = "https://stt.api.cloud.yandex.net/speech/v1/stt:recognize";
const SAMPLE_RATE_HZ: u32 = 48_000;
const DEFAULT_TEXT: &str = "Включи спокойный джаз";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();
    let api_key = env::var("YANDEX_AI_API_KEY")?;
    let text = env::var("SPEECHKIT_DIAGNOSTIC_TEXT").unwrap_or_else(|_| DEFAULT_TEXT.to_owned());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;

    let tts_started = Instant::now();
    let pcm = client
        .post(TTS_URL)
        .header(header::AUTHORIZATION, format!("Api-Key {api_key}"))
        .form(&[
            ("text", text.as_str()),
            ("lang", "ru-RU"),
            ("voice", "filipp"),
            ("format", "lpcm"),
            ("sampleRateHertz", "48000"),
        ])
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let tts_elapsed = tts_started.elapsed();
    if pcm.is_empty() || pcm.len() % 2 != 0 {
        return Err("Yandex TTS returned invalid PCM16 audio".into());
    }

    let stt_started = Instant::now();
    let response = client
        .post(STT_URL)
        .header(header::AUTHORIZATION, format!("Api-Key {api_key}"))
        .query(&[
            ("topic", "general"),
            ("lang", "ru-RU"),
            ("format", "lpcm"),
            ("sampleRateHertz", "48000"),
        ])
        .body(pcm.clone())
        .send()
        .await?
        .error_for_status()?;
    let transcript = response
        .json::<RecognitionResponse>()
        .await?
        .result
        .unwrap_or_default();

    println!(
        "PCM diagnostic: text={text:?}; bytes={}; duration_ms={}; tts_ms={}; stt_ms={}; transcript={transcript:?}",
        pcm.len(),
        pcm.len() as u64 * 1_000 / (SAMPLE_RATE_HZ as u64 * 2),
        tts_elapsed.as_millis(),
        stt_started.elapsed().as_millis(),
    );
    Ok(())
}

#[derive(Deserialize)]
struct RecognitionResponse {
    result: Option<String>,
}

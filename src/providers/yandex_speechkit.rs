//! Yandex SpeechKit v1 adapter for the bounded PCM WebSocket protocol.

use std::{env, time::Duration};

use async_trait::async_trait;
use reqwest::header;
use serde::Deserialize;

use crate::voice::{
    SpeechProviderError, SpeechStreamConfig, SpeechStreamSession, StreamingSpeechRecognizer,
    TranscriptUpdate,
};

const API_KEY_ENV: &str = "YANDEX_AI_API_KEY";
const RECOGNIZE_URL: &str = "https://stt.api.cloud.yandex.net/speech/v1/stt:recognize";
const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;

/// SpeechKit recognizer using a configured API key without retaining it in logs.
#[derive(Clone)]
pub struct YandexSpeechKitRecognizer {
    client: reqwest::Client,
    api_key: String,
}

impl YandexSpeechKitRecognizer {
    /// Selects SpeechKit only if its API key is configured locally.
    pub fn optional_from_env() -> Result<Option<Self>, SpeechProviderError> {
        let api_key = match env::var(API_KEY_ENV) {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) | Err(env::VarError::NotPresent) => return Ok(None),
            Err(env::VarError::NotUnicode(_)) => {
                return Err(SpeechProviderError::safe(
                    "YANDEX_AI_API_KEY must contain valid Unicode",
                ));
            }
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| SpeechProviderError::safe("SpeechKit HTTP client configuration failed"))?;
        Ok(Some(Self { client, api_key }))
    }
}

#[async_trait]
impl StreamingSpeechRecognizer for YandexSpeechKitRecognizer {
    async fn start(
        &self,
        config: SpeechStreamConfig,
    ) -> Result<Box<dyn SpeechStreamSession>, SpeechProviderError> {
        Ok(Box::new(YandexSpeechKitSession {
            client: self.client.clone(),
            api_key: self.api_key.clone(),
            config,
            audio: Vec::new(),
        }))
    }
}

struct YandexSpeechKitSession {
    client: reqwest::Client,
    api_key: String,
    config: SpeechStreamConfig,
    audio: Vec<u8>,
}

#[async_trait]
impl SpeechStreamSession for YandexSpeechKitSession {
    async fn push_audio(
        &mut self,
        audio: &[u8],
    ) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        if self.audio.len().saturating_add(audio.len()) > MAX_AUDIO_BYTES {
            return Err(SpeechProviderError::safe(
                "SpeechKit audio buffer limit exceeded",
            ));
        }
        self.audio.extend_from_slice(audio);
        Ok(Vec::new())
    }

    async fn finish(&mut self) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        if self.audio.is_empty() {
            return Err(SpeechProviderError::safe("SpeechKit audio buffer is empty"));
        }
        let response = self
            .client
            .post(RECOGNIZE_URL)
            .header(header::AUTHORIZATION, format!("Api-Key {}", self.api_key))
            .query(&[
                ("topic", "general"),
                ("lang", self.config.locale.as_str()),
                ("format", "lpcm"),
                ("sampleRateHertz", &self.config.sample_rate_hz.to_string()),
            ])
            .body(std::mem::take(&mut self.audio))
            .send()
            .await
            .map_err(|_| SpeechProviderError::safe("SpeechKit recognition request failed"))?;
        if !response.status().is_success() {
            return Err(SpeechProviderError::safe(format!(
                "SpeechKit returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let payload = response.json::<RecognitionResponse>().await.map_err(|_| {
            SpeechProviderError::safe("SpeechKit returned invalid recognition JSON")
        })?;
        let transcript = payload
            .result
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| SpeechProviderError::safe("SpeechKit returned an empty transcript"))?;
        Ok(vec![TranscriptUpdate {
            transcript,
            is_final: true,
        }])
    }
}

#[derive(Deserialize)]
struct RecognitionResponse {
    result: Option<String>,
}

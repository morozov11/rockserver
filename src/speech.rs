//! Provider-neutral streaming speech-recognition boundaries.

use async_trait::async_trait;
use std::{error::Error, fmt};

/// Validated audio and language settings for one recognition session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeechStreamConfig {
    /// BCP 47-style locale requested by the client.
    pub locale: String,
    /// PCM sample rate in hertz.
    pub sample_rate_hz: u32,
}

/// One incremental or final transcript emitted by a speech provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptUpdate {
    /// Recognized text accumulated by the provider.
    pub transcript: String,
    /// Whether the provider considers this utterance complete.
    pub is_final: bool,
}

/// Sanitized provider failure safe to log and map to the public API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeechProviderError {
    message: String,
}

impl SpeechProviderError {
    /// Creates a provider failure without retaining credentials or raw upstream payloads.
    pub fn safe(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SpeechProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SpeechProviderError {}

/// Active bidirectional recognition session owned by one client WebSocket.
#[async_trait]
pub trait SpeechStreamSession: Send {
    /// Pushes one bounded raw PCM chunk and returns all immediately available updates.
    async fn push_audio(
        &mut self,
        audio: &[u8],
    ) -> Result<Vec<TranscriptUpdate>, SpeechProviderError>;

    /// Commits buffered audio and returns the provider's remaining updates.
    async fn finish(&mut self) -> Result<Vec<TranscriptUpdate>, SpeechProviderError>;
}

/// Factory for replaceable Yandex, OpenAI, or deterministic recognition sessions.
#[async_trait]
pub trait StreamingSpeechRecognizer: Send + Sync {
    /// Starts one isolated recognition session with validated transport settings.
    async fn start(
        &self,
        config: SpeechStreamConfig,
    ) -> Result<Box<dyn SpeechStreamSession>, SpeechProviderError>;
}

/// Recognizer used when no external STT provider has been configured.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableSpeechRecognizer;

#[async_trait]
impl StreamingSpeechRecognizer for UnavailableSpeechRecognizer {
    async fn start(
        &self,
        _config: SpeechStreamConfig,
    ) -> Result<Box<dyn SpeechStreamSession>, SpeechProviderError> {
        Err(SpeechProviderError::safe(
            "streaming speech provider is not configured",
        ))
    }
}

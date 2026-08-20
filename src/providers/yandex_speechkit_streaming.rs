//! Yandex SpeechKit v3 bidirectional streaming recognizer.

use std::{env, sync::Arc};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, metadata::MetadataValue};
use url::Url;
use yandex_cloud::speechkit::stt::v3::{
    AlternativeUpdate, AudioChunk, AudioFormatOptions, Eou, EouClassifierOptions,
    ExternalEouClassifier, LanguageRestrictionOptions, RawAudio, RecognitionModelOptions,
    StreamingOptions, StreamingRequest, StreamingResponse, audio_format_options::AudioFormat,
    eou_classifier_options::Classifier as EouClassifier,
    language_restriction_options::LanguageRestrictionType, raw_audio::AudioEncoding,
    recognition_model_options::AudioProcessingType, recognizer_client::RecognizerClient,
    streaming_request::Event as RequestEvent, streaming_response::Event as ResponseEvent,
};

use crate::voice::{
    SpeechProviderError, SpeechStreamConfig, SpeechStreamSession, StreamingSpeechRecognizer,
    TranscriptUpdate,
};

const DEFAULT_ENDPOINT: &str = "https://stt.api.cloud.yandex.net";
const CHANNEL_CAPACITY: usize = 16;

/// SpeechKit v3 provider using a gRPC stream for a single client voice session.
#[derive(Clone)]
pub struct YandexSpeechKitStreamingRecognizer {
    api_key: Arc<str>,
    endpoint: String,
}

impl YandexSpeechKitStreamingRecognizer {
    /// Builds the provider only when an API key is configured.
    pub fn optional_from_env() -> Result<Option<Self>, SpeechProviderError> {
        let Some(api_key) = env::var("YANDEX_AI_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let endpoint = env::var("YANDEX_SPEECHKIT_STREAMING_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        validate_endpoint(&endpoint)?;
        Ok(Some(Self {
            api_key: Arc::from(api_key),
            endpoint,
        }))
    }
}

#[async_trait]
impl StreamingSpeechRecognizer for YandexSpeechKitStreamingRecognizer {
    async fn start(
        &self,
        config: SpeechStreamConfig,
    ) -> Result<Box<dyn SpeechStreamSession>, SpeechProviderError> {
        let (request_tx, request_rx) = mpsc::channel(CHANNEL_CAPACITY);
        request_tx
            .send(streaming_options(&config))
            .await
            .map_err(|_| SpeechProviderError::safe("SpeechKit streaming request channel closed"))?;

        let mut client = RecognizerClient::connect(self.endpoint.clone())
            .await
            .map_err(|_| SpeechProviderError::safe("SpeechKit streaming connection failed"))?;
        let mut request = Request::new(ReceiverStream::new(request_rx));
        let authorization = MetadataValue::try_from(format!("Api-Key {}", self.api_key))
            .map_err(|_| SpeechProviderError::safe("SpeechKit API key is invalid"))?;
        request
            .metadata_mut()
            .insert("authorization", authorization);
        let response = client
            .recognize_streaming(request)
            .await
            .map_err(|_| SpeechProviderError::safe("SpeechKit streaming request failed"))?;

        let (updates_tx, updates_rx) = mpsc::channel(CHANNEL_CAPACITY);
        tokio::spawn(forward_updates(response.into_inner(), updates_tx));
        Ok(Box::new(YandexSpeechKitStreamingSession {
            request_tx: Some(request_tx),
            updates_rx,
            latest_transcript: None,
        }))
    }
}

struct YandexSpeechKitStreamingSession {
    request_tx: Option<mpsc::Sender<StreamingRequest>>,
    updates_rx: mpsc::Receiver<Result<TranscriptUpdate, SpeechProviderError>>,
    latest_transcript: Option<String>,
}

#[async_trait]
impl SpeechStreamSession for YandexSpeechKitStreamingSession {
    async fn push_audio(
        &mut self,
        audio: &[u8],
    ) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        let sender = self.request_tx.as_ref().ok_or_else(|| {
            SpeechProviderError::safe("SpeechKit streaming session has already been committed")
        })?;
        sender
            .send(StreamingRequest {
                event: Some(RequestEvent::Chunk(AudioChunk {
                    data: audio.to_vec(),
                })),
            })
            .await
            .map_err(|_| SpeechProviderError::safe("SpeechKit streaming request channel closed"))?;
        self.take_available_updates()
    }

    async fn finish(&mut self) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        if let Some(sender) = self.request_tx.take() {
            sender
                .send(StreamingRequest {
                    event: Some(RequestEvent::Eou(Eou {})),
                })
                .await
                .map_err(|_| {
                    SpeechProviderError::safe("SpeechKit streaming request channel closed")
                })?;
        }
        let mut updates = self.take_available_updates()?;
        while let Some(update) = self.updates_rx.recv().await {
            let update = update?;
            self.remember(&update);
            updates.push(update);
        }
        if !updates.iter().any(|update| update.is_final)
            && let Some(transcript) = self.latest_transcript.clone()
        {
            updates.push(TranscriptUpdate {
                transcript,
                is_final: true,
            });
        }
        Ok(updates)
    }
}

impl YandexSpeechKitStreamingSession {
    fn take_available_updates(&mut self) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        let mut updates = Vec::new();
        loop {
            match self.updates_rx.try_recv() {
                Ok(update) => {
                    let update = update?;
                    self.remember(&update);
                    updates.push(update);
                }
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                    return Ok(updates);
                }
            }
        }
    }

    fn remember(&mut self, update: &TranscriptUpdate) {
        if !update.transcript.trim().is_empty() {
            self.latest_transcript = Some(update.transcript.clone());
        }
    }
}

async fn forward_updates(
    mut response: tonic::Streaming<StreamingResponse>,
    updates_tx: mpsc::Sender<Result<TranscriptUpdate, SpeechProviderError>>,
) {
    loop {
        match response.message().await {
            Ok(Some(message)) => {
                if let Some(update) = transcript_update(message)
                    && updates_tx.send(Ok(update)).await.is_err()
                {
                    return;
                }
            }
            Ok(None) => return,
            Err(_) => {
                let _ = updates_tx
                    .send(Err(SpeechProviderError::safe(
                        "SpeechKit streaming response failed",
                    )))
                    .await;
                return;
            }
        }
    }
}

fn streaming_options(config: &SpeechStreamConfig) -> StreamingRequest {
    StreamingRequest {
        event: Some(RequestEvent::SessionOptions(StreamingOptions {
            recognition_model: Some(RecognitionModelOptions {
                model: "general".to_owned(),
                audio_format: Some(AudioFormatOptions {
                    audio_format: Some(AudioFormat::RawAudio(RawAudio {
                        audio_encoding: AudioEncoding::Linear16Pcm as i32,
                        sample_rate_hertz: i64::from(config.sample_rate_hz),
                        audio_channel_count: 1_i64,
                    })),
                }),
                text_normalization: None,
                language_restriction: Some(LanguageRestrictionOptions {
                    restriction_type: LanguageRestrictionType::Whitelist as i32,
                    language_code: vec![config.locale.clone()],
                }),
                audio_processing_type: AudioProcessingType::RealTime as i32,
            }),
            eou_classifier: Some(EouClassifierOptions {
                classifier: Some(EouClassifier::ExternalClassifier(ExternalEouClassifier {})),
            }),
            recognition_classifier: None,
            speech_analysis: None,
            speaker_labeling: None,
        })),
    }
}

fn transcript_update(message: StreamingResponse) -> Option<TranscriptUpdate> {
    match message.event? {
        ResponseEvent::Partial(update) => alternative_transcript(update, false),
        ResponseEvent::Final(update) => alternative_transcript(update, true),
        _ => None,
    }
}

fn alternative_transcript(update: AlternativeUpdate, is_final: bool) -> Option<TranscriptUpdate> {
    let transcript = update.alternatives.into_iter().next()?.text;
    (!transcript.trim().is_empty()).then_some(TranscriptUpdate {
        transcript,
        is_final,
    })
}

fn validate_endpoint(endpoint: &str) -> Result<(), SpeechProviderError> {
    let url = Url::parse(endpoint)
        .map_err(|_| SpeechProviderError::safe("YANDEX_SPEECHKIT_STREAMING_ENDPOINT is invalid"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || (url.path() != "/" && !url.path().is_empty())
    {
        return Err(SpeechProviderError::safe(
            "YANDEX_SPEECHKIT_STREAMING_ENDPOINT must be an HTTPS origin without credentials",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yandex_cloud::speechkit::stt::v3::{
        Alternative, streaming_response::Event as ResponseEvent,
    };

    #[test]
    fn streaming_options_use_pcm16_mono() {
        let request = streaming_options(&SpeechStreamConfig {
            locale: "ru-RU".to_owned(),
            sample_rate_hz: 48_000,
        });
        let Some(RequestEvent::SessionOptions(options)) = request.event else {
            panic!("expected streaming session options");
        };
        assert_eq!(options.recognition_model.unwrap().model, "general");
    }

    #[test]
    fn translates_partial_and_final_transcripts() {
        let response = StreamingResponse {
            event: Some(ResponseEvent::Final(AlternativeUpdate {
                alternatives: vec![Alternative {
                    text: "рок радио".to_owned(),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        };
        assert_eq!(
            transcript_update(response),
            Some(TranscriptUpdate {
                transcript: "рок радио".to_owned(),
                is_final: true,
            })
        );
    }

    #[test]
    fn endpoint_must_be_safe_https_origin() {
        assert!(validate_endpoint("https://stt.api.cloud.yandex.net").is_ok());
        assert!(validate_endpoint("http://localhost:8080").is_err());
        assert!(validate_endpoint("https://key@example.test").is_err());
    }
}

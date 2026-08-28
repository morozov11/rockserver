//! Minimal SpeechKit v3 protobuf and gRPC client boundary.
//!
//! The published `yandex-cloud` crate currently pins `tonic` 0.9 and its
//! vulnerable `rustls-webpki` dependency chain. RockServer only needs the
//! bidirectional `Recognizer/RecognizeStreaming` method, so this module keeps
//! that protocol surface local and uses the maintained tonic release.

use prost::Message;

/// External end-of-utterance classifier configuration.
#[derive(Clone, PartialEq, Message)]
pub struct ExternalEouClassifier {}

/// SpeechKit end-of-utterance classifier options.
#[derive(Clone, PartialEq, Message)]
pub struct EouClassifierOptions {
    #[prost(oneof = "eou_classifier_options::Classifier", tags = "2")]
    pub classifier: Option<eou_classifier_options::Classifier>,
}

/// Variants of the end-of-utterance classifier configuration.
pub mod eou_classifier_options {
    use super::ExternalEouClassifier;

    /// Supported classifier variants used by the streaming API.
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Classifier {
        /// Use the caller-provided end-of-utterance events.
        #[prost(message, tag = "2")]
        ExternalClassifier(ExternalEouClassifier),
    }
}

/// Raw PCM audio format.
#[derive(Clone, PartialEq, Message)]
pub struct RawAudio {
    /// PCM encoding identifier.
    #[prost(enumeration = "raw_audio::AudioEncoding", tag = "1")]
    pub audio_encoding: i32,
    /// PCM sample rate in hertz.
    #[prost(int64, tag = "2")]
    pub sample_rate_hertz: i64,
    /// Number of audio channels.
    #[prost(int64, tag = "3")]
    pub audio_channel_count: i64,
}

/// Raw-audio encoding values understood by SpeechKit.
pub mod raw_audio {
    /// Supported raw audio encodings.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum AudioEncoding {
        /// Unspecified encoding.
        Unspecified = 0,
        /// Signed little-endian 16-bit linear PCM.
        Linear16Pcm = 1,
    }
}

/// SpeechKit audio format options.
#[derive(Clone, PartialEq, Message)]
pub struct AudioFormatOptions {
    #[prost(oneof = "audio_format_options::AudioFormat", tags = "1")]
    pub audio_format: Option<audio_format_options::AudioFormat>,
}

/// Variants of the audio format options.
pub mod audio_format_options {
    use super::RawAudio;

    /// Supported streaming audio formats.
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum AudioFormat {
        /// Raw PCM audio without a container.
        #[prost(message, tag = "1")]
        RawAudio(RawAudio),
    }
}

/// Language restriction for recognition.
#[derive(Clone, PartialEq, Message)]
pub struct LanguageRestrictionOptions {
    #[prost(
        enumeration = "language_restriction_options::LanguageRestrictionType",
        tag = "1"
    )]
    pub restriction_type: i32,
    #[prost(string, repeated, tag = "2")]
    pub language_code: Vec<String>,
}

/// Language restriction modes.
pub mod language_restriction_options {
    /// Supported language restriction modes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum LanguageRestrictionType {
        /// Do not apply a language restriction.
        Unspecified = 0,
        /// Allow only the listed languages.
        Whitelist = 1,
        /// Reject the listed languages.
        Blacklist = 2,
    }
}

/// Recognition model configuration.
#[derive(Clone, PartialEq, Message)]
pub struct RecognitionModelOptions {
    #[prost(string, tag = "1")]
    pub model: String,
    #[prost(message, optional, tag = "2")]
    pub audio_format: Option<AudioFormatOptions>,
    #[prost(message, optional, tag = "4")]
    pub language_restriction: Option<LanguageRestrictionOptions>,
    #[prost(
        enumeration = "recognition_model_options::AudioProcessingType",
        tag = "5"
    )]
    pub audio_processing_type: i32,
}

/// Audio processing modes.
pub mod recognition_model_options {
    /// Supported processing modes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum AudioProcessingType {
        /// Let SpeechKit select the mode.
        Unspecified = 0,
        /// Process audio in real time.
        RealTime = 1,
        /// Process only after all audio is received.
        FullData = 2,
    }
}

/// Options for a bidirectional recognition stream.
#[derive(Clone, PartialEq, Message)]
pub struct StreamingOptions {
    #[prost(message, optional, tag = "1")]
    pub recognition_model: Option<RecognitionModelOptions>,
    #[prost(message, optional, tag = "2")]
    pub eou_classifier: Option<EouClassifierOptions>,
}

/// A chunk of raw audio data.
#[derive(Clone, PartialEq, Message)]
pub struct AudioChunk {
    #[prost(bytes = "vec", tag = "1")]
    pub data: Vec<u8>,
}

/// Explicit end-of-utterance event.
#[derive(Clone, PartialEq, Message)]
pub struct Eou {}

/// One request sent to SpeechKit during a streaming session.
#[derive(Clone, PartialEq, Message)]
pub struct StreamingRequest {
    #[prost(oneof = "streaming_request::Event", tags = "1, 2, 4")]
    pub event: Option<streaming_request::Event>,
}

/// Streaming request events.
pub mod streaming_request {
    use super::{AudioChunk, Eou, StreamingOptions};

    /// Supported streaming request events.
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Event {
        /// The first message containing stream options.
        #[prost(message, tag = "1")]
        SessionOptions(StreamingOptions),
        /// A raw audio data chunk.
        #[prost(message, tag = "2")]
        Chunk(AudioChunk),
        /// End the current utterance.
        #[prost(message, tag = "4")]
        Eou(Eou),
    }
}

/// A recognized transcript alternative.
#[derive(Clone, PartialEq, Message)]
pub struct Alternative {
    #[prost(string, tag = "2")]
    pub text: String,
}

/// A partial or final transcript update.
#[derive(Clone, PartialEq, Message)]
pub struct AlternativeUpdate {
    #[prost(message, repeated, tag = "1")]
    pub alternatives: Vec<Alternative>,
}

/// Server-side end-of-utterance update.
#[derive(Clone, PartialEq, Message)]
pub struct EouUpdate {
    #[prost(int64, tag = "2")]
    pub time_ms: i64,
}

/// One response received from SpeechKit.
#[derive(Clone, PartialEq, Message)]
pub struct StreamingResponse {
    #[prost(oneof = "streaming_response::Event", tags = "4, 5")]
    pub event: Option<streaming_response::Event>,
}

/// Streaming response events consumed by RockServer.
pub mod streaming_response {
    use super::{AlternativeUpdate, EouUpdate};

    /// Supported transcript response events.
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Event {
        /// A provisional transcript.
        #[prost(message, tag = "4")]
        Partial(AlternativeUpdate),
        /// A finalized transcript.
        #[prost(message, tag = "5")]
        Final(AlternativeUpdate),
        /// End-of-utterance metadata not used by the voice boundary.
        #[prost(message, tag = "6")]
        EouUpdate(EouUpdate),
    }
}

/// Generated-compatible client for SpeechKit's streaming recognizer.
pub mod recognizer_client {
    use super::{StreamingRequest, StreamingResponse};
    use tonic::codegen::*;

    /// Client for the SpeechKit recognizer service.
    #[derive(Debug, Clone)]
    pub struct RecognizerClient<T> {
        inner: tonic::client::Grpc<T>,
    }

    impl RecognizerClient<tonic::transport::Channel> {
        /// Connects to a SpeechKit endpoint.
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }

    impl<T> RecognizerClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::Body>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    {
        /// Wraps an established transport channel.
        pub fn new(inner: T) -> Self {
            Self {
                inner: tonic::client::Grpc::new(inner),
            }
        }

        /// Starts the bidirectional recognition stream.
        pub async fn recognize_streaming(
            &mut self,
            request: impl tonic::IntoStreamingRequest<Message = StreamingRequest>,
        ) -> Result<tonic::Response<tonic::codec::Streaming<StreamingResponse>>, tonic::Status>
        {
            self.inner.ready().await.map_err(|error| {
                tonic::Status::new(
                    tonic::Code::Unknown,
                    format!("SpeechKit recognizer was not ready: {}", error.into()),
                )
            })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/speechkit.stt.v3.Recognizer/RecognizeStreaming",
            );
            let mut request = request.into_streaming_request();
            request.extensions_mut().insert(GrpcMethod::new(
                "speechkit.stt.v3.Recognizer",
                "RecognizeStreaming",
            ));
            self.inner.streaming(request, path, codec).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Alternative, AlternativeUpdate, StreamingResponse, streaming_response::Event};
    use prost::Message;

    #[test]
    fn decodes_speechkit_partial_response_wire_tags() {
        let response =
            StreamingResponse::decode([0x22, 0x06, 0x0a, 0x04, 0x12, 0x02, 0x6f, 0x6b].as_ref())
                .expect("SpeechKit partial response should decode");

        assert_eq!(
            response.event,
            Some(Event::Partial(AlternativeUpdate {
                alternatives: vec![Alternative {
                    text: "ok".to_owned(),
                }],
            }))
        );
    }

    #[test]
    fn encodes_speechkit_audio_chunk_wire_tags() {
        let request = super::StreamingRequest {
            event: Some(super::streaming_request::Event::Chunk(super::AudioChunk {
                data: vec![1, 2],
            })),
        };

        assert_eq!(request.encode_to_vec(), vec![0x12, 0x04, 0x0a, 0x02, 1, 2]);
    }
}

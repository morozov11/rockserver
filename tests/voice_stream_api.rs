use std::sync::Arc;

use async_trait::async_trait;
use rockserver::{
    http::{DEFAULT_VOICE_COMMAND_TIMEOUT, router_with_services},
    search::{InMemoryStationRepository, SearchService},
    speech::{
        SpeechProviderError, SpeechStreamConfig, SpeechStreamSession, StreamingSpeechRecognizer,
        TranscriptUpdate,
    },
};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn websocket_stream_emits_transcript_and_station_result() {
    let app = router_with_services(
        SearchService::new(Arc::new(InMemoryStationRepository::with_builtin_catalog())),
        Arc::new(FakeRecognizer),
        DEFAULT_VOICE_COMMAND_TIMEOUT,
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET /api/v1/voice/stream HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Bearer {}\r\nX-Request-Id: stream-test\r\n\r\n",
        rockserver::http::TEST_API_BEARER_TOKEN,
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let headers = read_http_headers(&mut stream).await;
    assert!(headers.starts_with("HTTP/1.1 101 Switching Protocols"));
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("x-request-id: stream-test")
    );

    write_client_frame(
        &mut stream,
        0x1,
        br#"{"type":"start","locale":"en-US","sample_rate_hz":16000}"#,
    )
    .await;
    let ready = read_json_frame(&mut stream).await;
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["request_id"], "stream-test");

    write_client_frame(&mut stream, 0x2, &[1, 2, 3, 4]).await;
    let partial = read_json_frame(&mut stream).await;
    assert_eq!(partial["type"], "transcript");
    assert_eq!(partial["is_final"], false);

    write_client_frame(&mut stream, 0x1, br#"{"type":"commit"}"#).await;
    let final_transcript = read_json_frame(&mut stream).await;
    assert_eq!(final_transcript["type"], "transcript");
    assert_eq!(final_transcript["transcript"], "jazz");
    assert_eq!(final_transcript["is_final"], true);
    let result = read_json_frame(&mut stream).await;
    assert_eq!(result["type"], "result");
    assert_eq!(result["request_id"], "stream-test");
    assert_eq!(result["transcript"], "jazz");
    assert!(result["selected_station"].is_object());

    server.abort();
}

#[derive(Clone, Copy)]
struct FakeRecognizer;

#[async_trait]
impl StreamingSpeechRecognizer for FakeRecognizer {
    async fn start(
        &self,
        config: SpeechStreamConfig,
    ) -> Result<Box<dyn SpeechStreamSession>, SpeechProviderError> {
        assert_eq!(config.sample_rate_hz, 16_000);
        Ok(Box::new(FakeSession))
    }
}

struct FakeSession;

#[async_trait]
impl SpeechStreamSession for FakeSession {
    async fn push_audio(
        &mut self,
        audio: &[u8],
    ) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        assert_eq!(audio, [1, 2, 3, 4]);
        Ok(vec![TranscriptUpdate {
            transcript: "ja".to_owned(),
            is_final: false,
        }])
    }

    async fn finish(&mut self) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        Ok(vec![TranscriptUpdate {
            transcript: "jazz".to_owned(),
            is_final: true,
        }])
    }
}

async fn read_http_headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let byte = stream.read_u8().await.unwrap();
        bytes.push(byte);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes).unwrap();
        }
    }
}

async fn write_client_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) {
    assert!(payload.len() < 126);
    let mask = [1_u8, 2, 3, 4];
    let mut frame = vec![0x80 | opcode, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    stream.write_all(&frame).await.unwrap();
}

async fn read_json_frame(stream: &mut TcpStream) -> Value {
    let first = stream.read_u8().await.unwrap();
    assert_eq!(first & 0x0f, 0x1);
    let second = stream.read_u8().await.unwrap();
    assert_eq!(second & 0x80, 0);
    let length = match second & 0x7f {
        value @ 0..=125 => usize::from(value),
        126 => usize::from(stream.read_u16().await.unwrap()),
        127 => usize::try_from(stream.read_u64().await.unwrap()).unwrap(),
        _ => unreachable!(),
    };
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await.unwrap();
    serde_json::from_slice(&payload).unwrap()
}

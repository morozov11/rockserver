//! Native device-control WebSocket ingress and its bounded v1 handshake.

use std::time::Duration;

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::HeaderMap,
    response::Response,
};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    device_control_auth::{DeviceControlAuthenticationError, DeviceControlPrincipal},
    device_control_presence::{ConnectionRegistry, DisconnectReason, control_shutdown_subscriber},
};

use super::{
    control_auth::authenticate_control_ingress,
    state::AppState,
    transport::{error_response, request_id, retry_after, unauthorized_response, with_request_id},
};

const MAX_FRAME_BYTES: usize = 65_536;
const MAX_PAYLOAD_BYTES: usize = 61_440;

/// Authenticates then upgrades the canonical device-control WebSocket endpoint.
pub(super) async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let request_id = request_id(&headers);
    let Some(resolver) = state.control_session_resolver.as_ref() else {
        return retry_after(
            error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "control_unavailable",
                "Device control is temporarily unavailable.",
                &request_id,
                json!({}),
            ),
            1,
        );
    };
    let principal = match authenticate_control_ingress(&headers, resolver.as_ref()).await {
        Ok(principal) => principal,
        Err(DeviceControlAuthenticationError::InvalidCredential) => {
            return unauthorized_response(&request_id);
        }
        Err(DeviceControlAuthenticationError::Unavailable) => {
            return retry_after(
                error_response(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "control_auth_unavailable",
                    "Device control authentication is temporarily unavailable.",
                    &request_id,
                    json!({}),
                ),
                1,
            );
        }
    };
    let registry = state.control_registry.clone();
    let timing = state.control_timing;
    with_request_id(
        upgrade
            .max_message_size(MAX_FRAME_BYTES + 1)
            .on_upgrade(move |socket| run(socket, principal, registry, timing)),
        &request_id,
    )
}

async fn run(
    mut socket: WebSocket,
    principal: DeviceControlPrincipal,
    registry: ConnectionRegistry,
    timing: super::state::ControlTiming,
) {
    let connection_id = Uuid::new_v4();
    match receive_envelope(&mut socket, timing.registration_deadline).await {
        Ok(envelope) if hello_supports_v1(&envelope) => {}
        Ok(_) => {
            let _ = protocol_close(&mut socket, "unsupported_protocol_version", 1002).await;
            return;
        }
        Err(ProtocolParseError::TooLarge) => {
            let _ = protocol_close(&mut socket, "frame_too_large", 1009).await;
            return;
        }
        Err(ProtocolParseError::Invalid) => {
            let _ = protocol_close(&mut socket, "invalid_message", 1007).await;
            return;
        }
    }
    if send_envelope(&mut socket, "protocol.welcome", json!({"selected_protocol_version":1,"connection_id":connection_id,"registration_deadline_seconds":10,"limits":limits()})).await.is_err() { return; }
    let register = match receive_envelope(&mut socket, timing.registration_deadline).await {
        Ok(envelope) if is_registration(&envelope) => envelope,
        Err(ProtocolParseError::TooLarge) => {
            let _ = protocol_close(&mut socket, "frame_too_large", 1009).await;
            return;
        }
        _ => {
            let _ = protocol_close(&mut socket, "registration_required", 1008).await;
            return;
        }
    };
    if !valid_registration(&register) {
        let _ = protocol_close(&mut socket, "invalid_message", 1007).await;
        return;
    }
    let (replacement, mut replaced) = registry.replacement_channel();
    registry.register(
        principal.user_id,
        principal.device_id,
        connection_id,
        replacement,
        std::time::Instant::now(),
    );
    if send_envelope(&mut socket, "device.registered", json!({"connection_id":connection_id,"authenticated_user_id":principal.user_id,"authenticated_device_id":principal.device_id,"granted_scopes":["device.directory.read","media.control"],"heartbeat_interval_seconds":20,"offline_ttl_seconds":60,"require_full_state":true})).await.is_err() {
        registry.disconnect(principal.device_id, connection_id, DisconnectReason::TransportLost, std::time::Instant::now()); return;
    }
    let mut reason = DisconnectReason::TransportLost;
    let mut last_seen = tokio::time::Instant::now();
    let mut shutdown = control_shutdown_subscriber();
    loop {
        tokio::select! {
            _ = shutdown.recv() => { let _ = socket.send(Message::Close(Some(CloseFrame { code: 1001, reason: "server_shutdown".into() }))).await; break; }
            _ = tokio::time::sleep_until(last_seen + timing.offline_ttl) => { reason = DisconnectReason::HeartbeatExpired; let _ = protocol_close(&mut socket, "heartbeat_expired", 1008).await; break; }
            replacement_reason = replaced.recv() => { reason = replacement_reason.unwrap_or(DisconnectReason::TransportLost); let _ = socket.send(Message::Close(Some(CloseFrame { code: 4001, reason: "replaced".into() }))).await; break; }
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => match parse_envelope(&text) {
                    Ok(envelope) if envelope["type"] == "device.heartbeat" => {
                        let Some(sequence) = envelope["payload"]["sequence"].as_u64() else { let _ = protocol_close(&mut socket, "invalid_message", 1007).await; break; };
                        last_seen = tokio::time::Instant::now();
                        if !registry.heartbeat(principal.device_id, connection_id, std::time::Instant::now()) { reason = DisconnectReason::Replaced; break; }
                        if send_envelope(&mut socket, "device.heartbeat_ack", json!({"sequence":sequence,"server_time":now()})).await.is_err() { break; }
                    }
                    Err(ProtocolParseError::TooLarge) => { let _ = protocol_close(&mut socket, "frame_too_large", 1009).await; break; }
                    Err(ProtocolParseError::Invalid) => { let _ = protocol_close(&mut socket, "invalid_message", 1007).await; break; }
                    _ => { let _ = protocol_close(&mut socket, "unexpected_message", 1008).await; break; }
                },
                Some(Ok(Message::Close(_))) => { reason = DisconnectReason::GracefulDisconnect; break; }
                Some(Ok(Message::Binary(_))) => { let _ = protocol_close(&mut socket, "invalid_message", 1003).await; break; }
                Some(Ok(_)) => {},
                Some(Err(_)) | None => break,
            }
        }
    }
    registry.disconnect(
        principal.device_id,
        connection_id,
        reason,
        std::time::Instant::now(),
    );
}

enum ProtocolParseError {
    Invalid,
    TooLarge,
}

async fn receive_envelope(
    socket: &mut WebSocket,
    timeout: Duration,
) -> Result<Value, ProtocolParseError> {
    match tokio::time::timeout(timeout, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => parse_envelope(&text),
        _ => Err(ProtocolParseError::Invalid),
    }
}

fn parse_envelope(text: &str) -> Result<Value, ProtocolParseError> {
    if text.len() > MAX_FRAME_BYTES {
        return Err(ProtocolParseError::TooLarge);
    }
    let value: Value = serde_json::from_str(text).map_err(|_| ProtocolParseError::Invalid)?;
    if value.get("protocol_version").and_then(Value::as_u64) != Some(1)
        || value
            .get("message_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .is_none()
        || value.get("type").and_then(Value::as_str).is_none()
        || value.get("sent_at").and_then(Value::as_str).is_none()
        || value
            .get("payload")
            .map(|payload| payload.to_string().len() <= MAX_PAYLOAD_BYTES)
            != Some(true)
    {
        return Err(ProtocolParseError::Invalid);
    }
    Ok(value)
}

fn hello_supports_v1(value: &Value) -> bool {
    value["type"] == "protocol.hello"
        && value["payload"]["supported_protocol_versions"]
            .as_array()
            .is_some_and(|versions| {
                versions.len() <= 4 && versions.iter().any(|version| version.as_u64() == Some(1))
            })
}
fn is_registration(value: &Value) -> bool {
    value["type"] == "device.register"
}
fn valid_registration(value: &Value) -> bool {
    let payload = &value["payload"];
    payload.as_object().is_some_and(|payload| {
        payload.len() == 3
            && payload
                .get("device_type")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty() && value.len() <= 64)
            && payload
                .get("app_version")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty() && value.len() <= 64)
            && payload.get("manifest").is_some_and(Value::is_object)
    })
}
fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("RFC3339 formatter is valid")
}
fn limits() -> Value {
    json!({"max_json_frame_bytes":65536,"max_payload_bytes":61440,"max_capabilities":32,"max_entities":256,"max_surfaces":16,"max_directory_devices":50,"heartbeat_interval_seconds":20,"offline_ttl_seconds":60,"max_in_flight_per_connection":16,"max_in_flight_per_target_device":8,"default_command_timeout_seconds":10,"max_command_timeout_seconds":30,"command_idempotency_window_seconds":86400,"max_stream_redirects":5})
}
async fn send_envelope(socket: &mut WebSocket, kind: &str, payload: Value) -> Result<(), ()> {
    socket.send(Message::Text(json!({"protocol_version":1,"message_id":Uuid::new_v4(),"type":kind,"sent_at":now(),"payload":payload}).to_string().into())).await.map_err(|_| ())
}
async fn protocol_close(socket: &mut WebSocket, code: &str, close_code: u16) -> Result<(), ()> {
    let _ = send_envelope(
        socket,
        "protocol.error",
        json!({"error":{"code":code,"message":"Device-control protocol error."}}),
    )
    .await;
    socket
        .send(Message::Close(Some(CloseFrame {
            code: close_code,
            reason: code.into(),
        })))
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        net::SocketAddr,
        sync::{Arc, Mutex, OnceLock},
        time::Duration,
    };

    use axum::Router;
    use sha2::{Digest, Sha256};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    use super::*;
    use crate::{
        auth::{ActiveSession, NativeSessionLookupError, NativeSessionResolver, SecretHash},
        http::endpoints::{
            build_router,
            state::{AppState, ControlTiming, PublicLimitState},
        },
        search::{InMemoryStationRepository, SearchService},
        voice::{SpeechRecognizers, UnavailableSpeechRecognizer},
    };

    static TRANSPORT_TEST_GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    #[derive(Default)]
    struct FakeResolver {
        sessions: Mutex<HashMap<Vec<u8>, ActiveSession>>,
        unavailable: bool,
    }

    #[async_trait::async_trait]
    impl NativeSessionResolver for FakeResolver {
        async fn resolve_active_native_session(
            &self,
            hash: &SecretHash,
        ) -> Result<Option<ActiveSession>, NativeSessionLookupError> {
            if self.unavailable {
                return Err(NativeSessionLookupError);
            }
            Ok(self
                .sessions
                .lock()
                .expect("test resolver mutex")
                .get(hash.as_bytes())
                .copied())
        }
    }

    fn resolver(token: &str, session: ActiveSession) -> Arc<FakeResolver> {
        let resolver = Arc::new(FakeResolver::default());
        resolver
            .sessions
            .lock()
            .expect("test resolver mutex")
            .insert(Sha256::digest(token.as_bytes()).to_vec(), session);
        resolver
    }

    fn app(
        resolver: Arc<dyn NativeSessionResolver>,
        registry: ConnectionRegistry,
        ttl: Duration,
    ) -> Router {
        build_router(AppState {
            search_service: SearchService::new(Arc::new(
                InMemoryStationRepository::with_builtin_catalog().unwrap(),
            )),
            speech_recognizers: SpeechRecognizers::same(Arc::new(UnavailableSpeechRecognizer)),
            voice_command_timeout: Duration::from_secs(5),
            api_bearer_token: "unrelated".to_owned(),
            account_store: None,
            admin_store: None,
            trusted_proxy_token: None,
            local_admin_origin: None,
            public_limits: Arc::new(Mutex::new(PublicLimitState::default())),
            control_registry: registry,
            control_session_resolver: Some(resolver),
            control_timing: ControlTiming {
                registration_deadline: Duration::from_millis(100),
                offline_ttl: ttl,
            },
        })
    }

    async fn server(app: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (address, task)
    }

    async fn upgrade(address: SocketAddr, token: &str) -> (TcpStream, String) {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(format!("GET /api/v1/devices/connect HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Bearer {token}\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = Vec::new();
        loop {
            let byte = stream.read_u8().await.unwrap();
            response.push(byte);
            if response.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        (stream, String::from_utf8(response).unwrap())
    }

    async fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) {
        let mut frame = vec![0x80 | opcode];
        match payload.len() {
            length @ 0..=125 => frame.push(0x80 | length as u8),
            length if length <= usize::from(u16::MAX) => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(length as u16).to_be_bytes());
            }
            length => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(length as u64).to_be_bytes());
            }
        }
        let mask = [1_u8, 2, 3, 4];
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
        stream.write_all(&frame).await.unwrap();
    }

    async fn read_frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
        let opcode = stream.read_u8().await.unwrap() & 0x0f;
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
        (opcode, payload)
    }

    fn envelope(kind: &str, payload: Value) -> String {
        json!({"protocol_version":1,"message_id":Uuid::new_v4(),"type":kind,"sent_at":"2026-09-02T12:00:00Z","payload":payload}).to_string()
    }

    async fn registered(address: SocketAddr, token: &str) -> (TcpStream, Value) {
        let (mut stream, response) = upgrade(address, token).await;
        assert!(response.starts_with("HTTP/1.1 101"));
        write_frame(
            &mut stream,
            1,
            envelope("protocol.hello", json!({"supported_protocol_versions":[1]})).as_bytes(),
        )
        .await;
        let (opcode, welcome) = read_frame(&mut stream).await;
        assert_eq!(opcode, 1);
        assert_eq!(
            serde_json::from_slice::<Value>(&welcome).unwrap()["type"],
            "protocol.welcome"
        );
        write_frame(
            &mut stream,
            1,
            envelope(
                "device.register",
                json!({"device_type":"rockcast","app_version":"test","manifest":{}}),
            )
            .as_bytes(),
        )
        .await;
        let (_, registered) = read_frame(&mut stream).await;
        let registered: Value = serde_json::from_slice(&registered).unwrap();
        assert_eq!(registered["type"], "device.registered");
        (stream, registered)
    }

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        for _ in 0..50 {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(predicate(), "condition was not met");
    }

    #[tokio::test]
    async fn authentication_happens_before_upgrade_and_registered_identity_is_server_derived() {
        let _gate = TRANSPORT_TEST_GATE
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let principal = ActiveSession {
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
        };
        let registry = ConnectionRegistry::default();
        let resolver = resolver("native", principal);
        let (address, task) = server(app(resolver, registry.clone(), Duration::from_secs(1))).await;
        let (_, denied) = upgrade(address, "invalid").await;
        assert!(denied.starts_with("HTTP/1.1 401"));
        let (stream, response) = registered(address, "native").await;
        assert_eq!(
            response["payload"]["authenticated_user_id"],
            principal.user_id.to_string()
        );
        assert_eq!(
            response["payload"]["authenticated_device_id"],
            principal.device_id.to_string()
        );
        drop(stream);
        wait_until(|| registry.snapshot_for(principal.user_id).is_empty()).await;
        task.abort();

        let unavailable = Arc::new(FakeResolver {
            sessions: Mutex::new(HashMap::new()),
            unavailable: true,
        });
        let (address, task) = server(app(
            unavailable,
            ConnectionRegistry::default(),
            Duration::from_secs(1),
        ))
        .await;
        let (_, unavailable) = upgrade(address, "native").await;
        assert!(unavailable.starts_with("HTTP/1.1 503"));
        task.abort();
    }

    #[tokio::test]
    async fn heartbeat_reconnect_transport_loss_and_ttl_have_one_active_offline_transition() {
        let _gate = TRANSPORT_TEST_GATE
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let principal = ActiveSession {
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
        };
        let registry = ConnectionRegistry::default();
        let resolver = resolver("native", principal);
        let (address, task) =
            server(app(resolver, registry.clone(), Duration::from_millis(70))).await;
        let (mut old, old_registered) = registered(address, "native").await;
        let old_id = old_registered["payload"]["connection_id"]
            .as_str()
            .unwrap()
            .to_owned();
        write_frame(
            &mut old,
            1,
            envelope("device.heartbeat", json!({"sequence":7})).as_bytes(),
        )
        .await;
        let (_, ack) = read_frame(&mut old).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&ack).unwrap()["payload"]["sequence"],
            7
        );
        let (new, new_registered) = registered(address, "native").await;
        assert_ne!(
            old_id,
            new_registered["payload"]["connection_id"].as_str().unwrap()
        );
        drop(old);
        assert_eq!(registry.snapshot_for(principal.user_id).len(), 1);
        drop(new);
        wait_until(|| registry.snapshot_for(principal.user_id).is_empty()).await;
        assert_eq!(
            registry
                .events_for(principal.user_id)
                .iter()
                .filter(|event| !event.online)
                .count(),
            1
        );
        let (mut graceful, _) = registered(address, "native").await;
        write_frame(&mut graceful, 8, &[]).await;
        wait_until(|| registry.snapshot_for(principal.user_id).is_empty()).await;
        assert_eq!(
            registry
                .events_for(principal.user_id)
                .last()
                .unwrap()
                .reason,
            Some(DisconnectReason::GracefulDisconnect)
        );
        let (_ttl_stream, _) = registered(address, "native").await;
        wait_until(|| registry.snapshot_for(principal.user_id).is_empty()).await;
        assert_eq!(
            registry
                .events_for(principal.user_id)
                .last()
                .unwrap()
                .reason,
            Some(DisconnectReason::HeartbeatExpired)
        );
        task.abort();
    }

    #[tokio::test]
    async fn wrong_first_frame_binary_and_registration_timeout_close_without_presence() {
        let _gate = TRANSPORT_TEST_GATE
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let principal = ActiveSession {
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
        };
        let registry = ConnectionRegistry::default();
        let resolver = resolver("native", principal);
        let (address, task) = server(app(resolver, registry.clone(), Duration::from_secs(1))).await;
        let (mut wrong, _) = upgrade(address, "native").await;
        write_frame(
            &mut wrong,
            1,
            envelope("device.register", json!({})).as_bytes(),
        )
        .await;
        assert_eq!(read_frame(&mut wrong).await.0, 1);
        let (mut binary, _) = upgrade(address, "native").await;
        write_frame(&mut binary, 2, b"binary").await;
        assert_eq!(read_frame(&mut binary).await.0, 1);
        let (mut oversized, _) = upgrade(address, "native").await;
        write_frame(&mut oversized, 1, &vec![b'x'; MAX_FRAME_BYTES + 1]).await;
        let (_, error) = read_frame(&mut oversized).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&error).unwrap()["payload"]["error"]["code"],
            "frame_too_large"
        );
        let (mut identity, _) = upgrade(address, "native").await;
        write_frame(
            &mut identity,
            1,
            envelope("protocol.hello", json!({"supported_protocol_versions":[1]})).as_bytes(),
        )
        .await;
        let _ = read_frame(&mut identity).await;
        write_frame(
            &mut identity,
            1,
            envelope(
                "device.register",
                json!({"device_type":"rockcast","app_version":"test","manifest":{},"device_id":Uuid::new_v4()}),
            )
            .as_bytes(),
        )
        .await;
        assert_eq!(read_frame(&mut identity).await.0, 1);
        let (_timeout, _) = upgrade(address, "native").await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(registry.snapshot_for(principal.user_id).is_empty());
        task.abort();
    }

    #[tokio::test]
    async fn server_shutdown_closes_registered_connections_and_cleans_presence() {
        let _gate = TRANSPORT_TEST_GATE
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let principal = ActiveSession {
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
        };
        let registry = ConnectionRegistry::default();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(crate::serve(
            listener,
            app(
                resolver("native", principal),
                registry.clone(),
                Duration::from_secs(1),
            ),
            async move {
                let _ = shutdown_rx.await;
            },
        ));
        let (mut stream, _) = registered(address, "native").await;
        shutdown_tx.send(()).unwrap();
        assert_eq!(read_frame(&mut stream).await.0, 8);
        wait_until(|| registry.snapshot_for(principal.user_id).is_empty()).await;
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

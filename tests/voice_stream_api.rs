use std::{
    collections::HashMap,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use rockserver::{
    auth::{ActiveSession, NativeSessionLookupError, NativeSessionResolver, SecretHash},
    device_control::{
        CATALOG_STATION_SOURCE, CommandBody, CommandId, CommandLifecycle, CommandReservation,
        CommandResult, CommandStatus, DeviceCapabilities, DeviceCapability, DeviceCommand,
        DeviceControlScope, DeviceControlStore, DeviceId, DeviceManifest, DeviceRole,
        DeviceRuntimeState, DeviceStateSnapshot, DomainError, Entity, EntityState, PlaybackState,
        StoreError, StoreOutcome, Surface, SurfaceKind, Timestamp, VolumeCommand,
    },
    device_control_command::CommandRouter,
    device_control_presence::{ConnectionRegistration, ConnectionRegistry, OutboundFrame},
    device_control_state::StateHub,
    http::{
        DEFAULT_VOICE_COMMAND_TIMEOUT, router_with_device_voice_personal_services,
        router_with_device_voice_services, router_with_services,
    },
    personal_data::{
        CollectionChanges, FavouriteUpsert, InMemoryPersonalDataStore, PersonalDataStore,
        SyncRequest,
    },
    search::{
        RankedStation, RepositoryError, SearchConstraints, SearchQuery, SearchService, Station,
        StationHealth, StationRepository,
    },
    speech::{
        SpeechProviderError, SpeechStreamConfig, SpeechStreamSession, StreamingSpeechRecognizer,
        TranscriptUpdate,
    },
    voice::{CommandInterpretationError, CommandInterpreter, Intent, RadioQuery, VoiceCommand},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

#[tokio::test]
async fn device_voice_routes_stop_resume_and_volume_through_the_normal_router() {
    for (transcript, expected, expected_intent) in [
        ("stop", ExpectedCommand::Stop, "stop"),
        ("volume 37", ExpectedCommand::Volume(37), "set_volume"),
        ("resume", ExpectedCommand::Resume, "resume"),
        ("громче", ExpectedCommand::VolumeChange(5), "volume_change"),
        ("тише", ExpectedCommand::VolumeChange(-5), "volume_change"),
    ] {
        let harness = Harness::new(transcript, stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
        let (command, frame) = harness.run_success_pair().await;
        assert_eq!(command.target.device_id, DeviceId(harness.device_id));
        assert_eq!(frame["intent"], expected_intent);
        match (expected, command.body) {
            (ExpectedCommand::Stop, CommandBody::Playback { action }) => {
                assert_eq!(action, "stop");
            }
            (ExpectedCommand::Resume, CommandBody::Playback { action }) => {
                assert_eq!(action, "play");
            }
            (
                ExpectedCommand::Volume(level),
                CommandBody::Volume {
                    command: VolumeCommand::SetLevel { level: actual },
                },
            ) => assert_eq!(actual, level),
            (
                ExpectedCommand::VolumeChange(delta),
                CommandBody::Volume {
                    command: VolumeCommand::Change { delta: actual },
                },
            ) => assert_eq!(actual, delta),
            (_, body) => panic!("unexpected routed command: {body:?}"),
        }
    }
}

#[tokio::test]
async fn device_voice_search_returns_the_ranked_list_for_device_autoplay() {
    let harness = Harness::new("play rock", stations(3), DEFAULT_VOICE_COMMAND_TIMEOUT);
    let frame = harness.run_direct_result().await;
    assert_eq!(frame["type"], "result");
    assert_eq!(frame["intent"], "play_radio");
    assert_eq!(frame["transcript"], "play rock");
    assert_eq!(frame["selected_station"]["id"], "station-1");
    assert_eq!(frame["stations"].as_array().unwrap().len(), 3);
    assert!(frame.get("status").is_none());
    // The server dispatches nothing for search: the device plays the first
    // entry itself through the same station.play_station path as a catalog tap.
    assert!(
        harness
            .target_messages
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .try_recv()
            .is_err()
    );
    assert!(harness.store.commands.lock().unwrap().is_empty());
}

#[tokio::test]
async fn device_voice_search_reports_nothing_found_without_side_effects() {
    let harness = Harness::new("play rock", Vec::new(), DEFAULT_VOICE_COMMAND_TIMEOUT);
    assert_eq!(harness.run_error().await, "station_not_found");
    assert!(harness.store.commands.lock().unwrap().is_empty());
}

#[tokio::test]
async fn equal_top_scores_no_longer_block_the_device_search() {
    // The device shows the ranked list and plays the first entry itself, so a
    // top-2 score tie is no longer a clarification: it is a normal result.
    let harness = Harness::new("play rock", stations(2), DEFAULT_VOICE_COMMAND_TIMEOUT);
    let frame = harness.run_direct_result().await;
    assert_eq!(frame["intent"], "play_radio");
    assert_eq!(frame["stations"].as_array().unwrap().len(), 2);
    assert_eq!(frame["selected_station"]["id"], "station-1");
}

#[tokio::test]
async fn device_voice_routes_navigation_when_the_manifest_declares_it() {
    for (transcript, expected_action, expected_intent) in [
        ("next", "next", "next_station"),
        ("previous", "previous", "previous_station"),
    ] {
        let harness = Harness::with_manifest(
            transcript,
            stations(1),
            DEFAULT_VOICE_COMMAND_TIMEOUT,
            navigating_manifest(),
        );
        let (command, frame) = harness.run_success_pair().await;
        assert_eq!(command.target.device_id, DeviceId(harness.device_id));
        assert_eq!(frame["intent"], expected_intent);
        match command.body {
            CommandBody::Playback { action } => assert_eq!(action, expected_action),
            body => panic!("unexpected routed command: {body:?}"),
        }
    }
}

#[tokio::test]
async fn navigation_without_manifest_support_is_rejected_without_side_effects() {
    for transcript in ["next", "previous"] {
        let harness = Harness::new(transcript, stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
        assert_eq!(harness.run_error().await, "capability_not_supported");
        assert!(harness.store.commands.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn voice_like_adds_the_current_station_to_account_favourites() {
    for (transcript, status) in [("лайк", "playing"), ("like", "paused")] {
        let personal = Arc::new(InMemoryPersonalDataStore::default());
        let harness = Harness::with_favourites(
            transcript,
            Some(personal.clone()),
            Some((status, "station-9")),
        );
        let result = harness.run_direct_result().await;
        assert_eq!(result["type"], "result");
        assert_eq!(result["status"], "succeeded");
        assert_eq!(result["intent"], "add_favorite");
        let outcome = personal
            .synchronize(harness.owner_id, SyncRequest::default())
            .await
            .unwrap();
        let live: Vec<_> = outcome
            .favourites
            .iter()
            .filter(|record| record.deleted_at.is_none())
            .collect();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].station_id.as_deref(), Some("station-9"));
    }
}

#[tokio::test]
async fn voice_like_is_idempotent_for_an_already_favourited_station() {
    let personal = Arc::new(InMemoryPersonalDataStore::default());
    let harness = Harness::with_favourites(
        "лайк",
        Some(personal.clone()),
        Some(("playing", "station-9")),
    );
    let added_at = rockserver::personal_data::Timestamp::parse("2026-10-24T00:00:00Z").unwrap();
    personal
        .synchronize(
            harness.owner_id,
            SyncRequest {
                since_revision: 0,
                favourites: CollectionChanges {
                    upserts: vec![FavouriteUpsert {
                        record_id: Uuid::new_v4(),
                        station_id: "station-9".to_owned(),
                        added_at: added_at.clone(),
                        updated_at: added_at,
                    }],
                    deletes: Vec::new(),
                },
                history: CollectionChanges::default(),
            },
        )
        .await
        .unwrap();
    let result = harness.run_direct_result().await;
    assert_eq!(result["type"], "result");
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["intent"], "add_favorite");
    let outcome = personal
        .synchronize(harness.owner_id, SyncRequest::default())
        .await
        .unwrap();
    let live: Vec<_> = outcome
        .favourites
        .iter()
        .filter(|record| record.deleted_at.is_none())
        .collect();
    assert_eq!(live.len(), 1);
}

#[tokio::test]
async fn voice_like_without_a_current_station_changes_nothing() {
    let personal = Arc::new(InMemoryPersonalDataStore::default());
    let harness = Harness::with_favourites("лайк", Some(personal.clone()), None);
    assert_eq!(harness.run_error().await, "station_not_found");
    let outcome = personal
        .synchronize(harness.owner_id, SyncRequest::default())
        .await
        .unwrap();
    assert!(outcome.favourites.is_empty());
}

#[tokio::test]
async fn voice_like_without_a_personal_store_is_persistence_unavailable() {
    let harness = Harness::with_favourites("лайк", None, Some(("playing", "station-9")));
    assert_eq!(harness.run_error().await, "persistence_unavailable");
}

#[tokio::test]
async fn device_voice_returns_failed_terminal_command_status() {
    let harness = Harness::new("stop", stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
    let (command, frame) = harness.run_terminal(CommandStatus::Failed, "failed").await;
    assert!(matches!(command.body, CommandBody::Playback { .. }));
    // The intent echo survives a failure so clients can say what did not work.
    assert_eq!(frame["intent"], "stop");
}

#[tokio::test]
async fn cancel_releases_recognition_and_emits_one_terminal_cancelled_error() {
    let harness = Harness::new("play rock", stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
    let drops = Arc::clone(&harness.session_drops);
    let (mut stream, server) = harness.connect(Some("native-token")).await;
    assert!(
        read_http_headers(&mut stream)
            .await
            .starts_with("HTTP/1.1 101 Switching Protocols")
    );
    start(&mut stream, true, 16_000).await;
    assert_eq!(read_json_frame(&mut stream).await["type"], "ready");
    write_client_frame(&mut stream, 0x1, br#"{"type":"cancel"}"#).await;
    let error = read_json_frame(&mut stream).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["code"], "cancelled");
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    let next = tokio::time::timeout(Duration::from_millis(200), stream.read_u8()).await;
    assert!(!matches!(next, Ok(Ok(byte)) if byte & 0x0f == 0x1));
    server.abort();
}

#[tokio::test]
async fn invalid_device_session_is_401_while_anonymous_search_remains_unchanged() {
    let harness = Harness::new("play rock", stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
    let (mut invalid, invalid_server) = harness.connect(Some("invalid-token")).await;
    let headers = read_http_headers(&mut invalid).await;
    assert!(headers.starts_with("HTTP/1.1 401 Unauthorized"));
    invalid_server.abort();

    let app = router_with_services(
        SearchService::new(Arc::new(FakeRepository::new(stations(1)))),
        Arc::new(FakeRecognizer::new(Some("rock"))),
        DEFAULT_VOICE_COMMAND_TIMEOUT,
    );
    let (address, server) = server(app).await;
    let mut stream = websocket_request(address, None).await;
    assert!(
        read_http_headers(&mut stream)
            .await
            .starts_with("HTTP/1.1 101 Switching Protocols")
    );
    start(&mut stream, false, 16_000).await;
    let ready = read_json_frame(&mut stream).await;
    assert!(ready.get("source_device_id").is_none());
    write_client_frame(&mut stream, 0x1, br#"{"type":"commit"}"#).await;
    assert_eq!(read_json_frame(&mut stream).await["type"], "transcript");
    let result = read_json_frame(&mut stream).await;
    assert_eq!(result["type"], "result");
    assert!(result["selected_station"].is_object());
    assert!(result.get("status").is_none());
    server.abort();
}

#[tokio::test]
async fn unsupported_intent_is_a_deterministic_error() {
    let unsupported = Harness::new("show sensors", stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
    assert_eq!(unsupported.run_error().await, "unsupported_intent");
}

#[tokio::test]
async fn device_session_without_surface_keeps_legacy_station_search() {
    let harness = Harness::new("play rock", stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
    let (mut stream, server) = harness.connect(Some("native-token")).await;
    assert!(
        read_http_headers(&mut stream)
            .await
            .starts_with("HTTP/1.1 101 Switching Protocols")
    );
    start(&mut stream, false, 16_000).await;
    let ready = read_json_frame(&mut stream).await;
    assert!(ready.get("source_device_id").is_none());
    write_client_frame(&mut stream, 0x1, br#"{"type":"commit"}"#).await;
    assert_eq!(read_json_frame(&mut stream).await["type"], "transcript");
    let result = read_json_frame(&mut stream).await;
    assert_eq!(result["type"], "result");
    assert!(result["selected_station"].is_object());
    assert!(result.get("status").is_none());
    // The legacy frame never carries the device intent marker.
    assert!(result.get("intent").is_none());
    server.abort();
}

#[tokio::test]
async fn device_start_with_surface_keeps_runtime_limit_validation() {
    let harness = Harness::new("play rock", stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
    let (mut stream, server) = harness.connect(Some("native-token")).await;
    assert!(
        read_http_headers(&mut stream)
            .await
            .starts_with("HTTP/1.1 101 Switching Protocols")
    );
    start(&mut stream, true, 8_000).await;
    assert_eq!(
        read_json_frame(&mut stream).await["code"],
        "validation_failed"
    );
    server.abort();
}

#[tokio::test]
async fn silence_and_recognition_failure_have_frozen_error_codes() {
    let silent = Harness::with_recognizer(
        FakeRecognizer::new(None),
        "play rock",
        stations(1),
        DEFAULT_VOICE_COMMAND_TIMEOUT,
    );
    assert_eq!(silent.run_error().await, "speech_not_recognized");

    let failed = Harness::with_recognizer(
        FakeRecognizer::failing(),
        "play rock",
        stations(1),
        DEFAULT_VOICE_COMMAND_TIMEOUT,
    );
    assert_eq!(failed.run_error().await, "speech_provider_error");
}

#[tokio::test]
async fn offline_source_and_search_timeout_are_deterministic() {
    let offline = Harness::new_unregistered("play rock", stations(1), Duration::from_millis(1));
    assert_eq!(offline.run_start_error().await, "target_offline");

    let timed_out = Harness::new_with_search_delay(
        "play rock",
        stations(1),
        Duration::from_millis(1),
        Duration::from_millis(50),
    );
    assert_eq!(timed_out.run_error().await, "search_timeout");
}

#[tokio::test]
async fn recognized_text_is_not_written_to_logs() {
    let marker = "SECRET_TRANSCRIPT_NEVER_LOGGED";
    let harness = Harness::new(marker, stations(1), DEFAULT_VOICE_COMMAND_TIMEOUT);
    let logs = LogWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .without_time()
        .with_writer(logs.clone())
        .finish();
    async {
        assert_eq!(harness.run_error().await, "unsupported_intent");
    }
    .with_subscriber(subscriber)
    .await;
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(!output.contains(marker));
}

#[derive(Clone, Copy)]
enum ExpectedCommand {
    Stop,
    Resume,
    Volume(u8),
    VolumeChange(i8),
}

struct Harness {
    app: axum::Router,
    owner_id: Uuid,
    device_id: Uuid,
    connection_id: Uuid,
    registry: ConnectionRegistry,
    router: CommandRouter,
    store: Arc<MemoryStore>,
    target_messages: Mutex<Option<mpsc::Receiver<OutboundFrame>>>,
    session_drops: Arc<AtomicUsize>,
}
impl Harness {
    fn new(transcript: &str, stations: Vec<RankedStation>, timeout: Duration) -> Self {
        Self::with_recognizer(
            FakeRecognizer::new(Some(transcript)),
            transcript,
            stations,
            timeout,
        )
    }

    fn new_with_search_delay(
        transcript: &str,
        stations: Vec<RankedStation>,
        timeout: Duration,
        search_delay: Duration,
    ) -> Self {
        Self::build(
            FakeRecognizer::new(Some(transcript)),
            transcript,
            stations,
            timeout,
            search_delay,
            true,
        )
    }

    fn new_unregistered(transcript: &str, stations: Vec<RankedStation>, timeout: Duration) -> Self {
        Self::build(
            FakeRecognizer::new(Some(transcript)),
            transcript,
            stations,
            timeout,
            Duration::ZERO,
            false,
        )
    }

    fn with_recognizer(
        recognizer: FakeRecognizer,
        transcript: &str,
        stations: Vec<RankedStation>,
        timeout: Duration,
    ) -> Self {
        Self::build(
            recognizer,
            transcript,
            stations,
            timeout,
            Duration::ZERO,
            true,
        )
    }

    fn with_manifest(
        transcript: &str,
        stations: Vec<RankedStation>,
        timeout: Duration,
        manifest: DeviceManifest,
    ) -> Self {
        Self::build_with_manifest(
            FakeRecognizer::new(Some(transcript)),
            transcript,
            stations,
            timeout,
            Duration::ZERO,
            true,
            manifest,
        )
    }

    /// Harness for account-favourite scenarios: wires the personal-data store and
    /// pre-seeds the device's reported playback state. `playing` is `(status, station_id)`.
    fn with_favourites(
        transcript: &str,
        personal: Option<Arc<InMemoryPersonalDataStore>>,
        playing: Option<(&str, &str)>,
    ) -> Self {
        let owner_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let connection_id = Uuid::new_v4();
        let recognizer = FakeRecognizer::new(Some(transcript));
        let resolver = Arc::new(FakeResolver::new("native-token", owner_id, device_id));
        let search = SearchService::new(Arc::new(FakeRepository::new(stations(1))));
        let router = CommandRouter::default().with_station_catalog(Arc::new(search.clone()));
        let registry = ConnectionRegistry::default();
        let store = Arc::new(MemoryStore::default());
        let (replacement, _) = registry.replacement_channel();
        let (outbound, receiver) = registry.outbound_channel();
        registry.register(
            ConnectionRegistration {
                user_id: owner_id,
                device_id,
                connection_id,
                replacement,
                outbound,
                manifest: radio_manifest(),
                scopes: vec![DeviceControlScope::MediaControl],
            },
            Instant::now(),
        );
        let hub = StateHub::default();
        hub.publish_device_state(
            owner_id,
            DeviceId(device_id),
            DeviceStateSnapshot {
                state_revision: 1,
                observed_at: Timestamp::parse("2026-10-25T00:00:00Z").unwrap(),
                received_at: None,
                state: DeviceRuntimeState {
                    playback: playing.map(|(status, station)| PlaybackState {
                        status: status.to_owned(),
                        station_id: Some(station.to_owned()),
                        track_title: None,
                    }),
                    volume: None,
                    display: None,
                },
            },
        );
        let app = router_with_device_voice_personal_services(
            search,
            Arc::new(recognizer.clone()),
            Arc::new(FakeInterpreter::new(transcript)),
            DEFAULT_VOICE_COMMAND_TIMEOUT,
            resolver,
            (
                registry.clone(),
                router.clone(),
                store.clone() as Arc<dyn DeviceControlStore>,
            ),
            personal.map(|store| store as Arc<dyn PersonalDataStore>),
            hub,
        );
        Self {
            app,
            owner_id,
            device_id,
            connection_id,
            registry,
            router,
            store,
            target_messages: Mutex::new(Some(receiver)),
            session_drops: Arc::clone(&recognizer.drops),
        }
    }

    fn build(
        recognizer: FakeRecognizer,
        transcript: &str,
        stations: Vec<RankedStation>,
        timeout: Duration,
        search_delay: Duration,
        register_device: bool,
    ) -> Self {
        Self::build_with_manifest(
            recognizer,
            transcript,
            stations,
            timeout,
            search_delay,
            register_device,
            radio_manifest(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_with_manifest(
        recognizer: FakeRecognizer,
        transcript: &str,
        stations: Vec<RankedStation>,
        timeout: Duration,
        search_delay: Duration,
        register_device: bool,
        manifest: DeviceManifest,
    ) -> Self {
        let owner_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let connection_id = Uuid::new_v4();
        let resolver = Arc::new(FakeResolver::new("native-token", owner_id, device_id));
        let mut repository = FakeRepository::new(stations);
        repository.delay = search_delay;
        let search = SearchService::new(Arc::new(repository));
        let router = CommandRouter::default().with_station_catalog(Arc::new(search.clone()));
        let registry = ConnectionRegistry::default();
        let store = Arc::new(MemoryStore::default());
        let target_messages = if register_device {
            let (replacement, _) = registry.replacement_channel();
            let (outbound, receiver) = registry.outbound_channel();
            registry.register(
                ConnectionRegistration {
                    user_id: owner_id,
                    device_id,
                    connection_id,
                    replacement,
                    outbound,
                    manifest,
                    scopes: vec![DeviceControlScope::MediaControl],
                },
                Instant::now(),
            );
            Some(receiver)
        } else {
            None
        };
        let store_trait: Arc<dyn DeviceControlStore> = store.clone();
        let app = router_with_device_voice_services(
            search,
            Arc::new(recognizer.clone()),
            Arc::new(FakeInterpreter::new(transcript)),
            timeout,
            resolver,
            (registry.clone(), router.clone(), store_trait),
        );
        Self {
            app,
            owner_id,
            device_id,
            connection_id,
            registry,
            router,
            store,
            target_messages: Mutex::new(target_messages),
            session_drops: Arc::clone(&recognizer.drops),
        }
    }

    async fn connect(&self, token: Option<&str>) -> (TcpStream, tokio::task::JoinHandle<()>) {
        let (address, server) = server(self.app.clone()).await;
        (websocket_request(address, token).await, server)
    }

    /// Runs one command-intent session to a succeeded terminal and returns the
    /// routed device command together with the terminal result frame.
    async fn run_success_pair(&self) -> (DeviceCommand, Value) {
        self.run_terminal(CommandStatus::Succeeded, "succeeded")
            .await
    }

    /// Runs one command-intent session end to end and returns the routed device
    /// command together with the terminal result frame for intent-echo checks.
    async fn run_terminal(
        &self,
        terminal_status: CommandStatus,
        expected_status: &str,
    ) -> (DeviceCommand, Value) {
        let mut target_messages = self.target_messages.lock().unwrap().take().unwrap();
        let router = self.router.clone();
        let registry = self.registry.clone();
        let store: Arc<dyn DeviceControlStore> = self.store.clone();
        let owner_id = self.owner_id;
        let device_id = self.device_id;
        let connection_id = self.connection_id;
        let failed = terminal_status == CommandStatus::Failed;
        let responder = tokio::spawn(async move {
            loop {
                let frame = target_messages.recv().await.unwrap();
                if frame.kind != "device.command" {
                    continue;
                }
                let command: DeviceCommand = serde_json::from_value(frame.payload).unwrap();
                router
                    .result(
                        &registry,
                        Some(&store),
                        owner_id,
                        device_id,
                        connection_id,
                        CommandResult {
                            command_id: command.command_id,
                            error: failed.then(|| DomainError {
                                code: "device_failure".to_owned(),
                                message: "Device command failed.".to_owned(),
                                request_id: "test-device-command".to_owned(),
                                details: Default::default(),
                            }),
                            status: terminal_status,
                            completed_at: Timestamp::parse("2026-09-09T00:00:00Z").unwrap(),
                        },
                    )
                    .await
                    .unwrap();
                return command;
            }
        });
        let (mut stream, server) = self.connect(Some("native-token")).await;
        assert!(
            read_http_headers(&mut stream)
                .await
                .starts_with("HTTP/1.1 101 Switching Protocols")
        );
        start(&mut stream, true, 16_000).await;
        let ready = read_json_frame(&mut stream).await;
        assert_eq!(ready["source_device_id"], self.device_id.to_string());
        assert_eq!(ready["surface_id"], "voice.main");
        write_client_frame(&mut stream, 0x1, br#"{"type":"commit"}"#).await;
        assert_eq!(read_json_frame(&mut stream).await["type"], "transcript");
        let result = read_json_frame(&mut stream).await;
        assert_eq!(result["type"], "result");
        assert_eq!(result["status"], expected_status);
        assert!(result.get("transcript").is_none());
        assert!(result.get("stations").is_none());
        let command = responder.await.unwrap();
        server.abort();
        (command, result)
    }

    async fn run_error(&self) -> String {
        let (mut stream, server) = self.connect(Some("native-token")).await;
        assert!(
            read_http_headers(&mut stream)
                .await
                .starts_with("HTTP/1.1 101 Switching Protocols")
        );
        start(&mut stream, true, 16_000).await;
        assert_eq!(read_json_frame(&mut stream).await["type"], "ready");
        write_client_frame(&mut stream, 0x1, br#"{"type":"commit"}"#).await;
        let first = read_json_frame(&mut stream).await;
        let error = if first["type"] == "transcript" {
            read_json_frame(&mut stream).await
        } else {
            first
        };
        server.abort();
        error["code"].as_str().unwrap().to_owned()
    }

    /// Runs one session whose terminal frame is produced without a device command
    /// (favourites) and returns that terminal frame for inspection.
    async fn run_direct_result(&self) -> Value {
        let (mut stream, server) = self.connect(Some("native-token")).await;
        assert!(
            read_http_headers(&mut stream)
                .await
                .starts_with("HTTP/1.1 101 Switching Protocols")
        );
        start(&mut stream, true, 16_000).await;
        assert_eq!(read_json_frame(&mut stream).await["type"], "ready");
        write_client_frame(&mut stream, 0x1, br#"{"type":"commit"}"#).await;
        assert_eq!(read_json_frame(&mut stream).await["type"], "transcript");
        let result = read_json_frame(&mut stream).await;
        server.abort();
        result
    }

    async fn run_start_error(&self) -> String {
        let (mut stream, server) = self.connect(Some("native-token")).await;
        assert!(
            read_http_headers(&mut stream)
                .await
                .starts_with("HTTP/1.1 101 Switching Protocols")
        );
        start(&mut stream, true, 16_000).await;
        let error = read_json_frame(&mut stream).await;
        server.abort();
        error["code"].as_str().unwrap().to_owned()
    }
}

#[derive(Clone)]
struct FakeRecognizer {
    transcript: Option<String>,
    fail: bool,
    drops: Arc<AtomicUsize>,
}

impl FakeRecognizer {
    fn new(transcript: Option<&str>) -> Self {
        Self {
            transcript: transcript.map(str::to_owned),
            fail: false,
            drops: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn failing() -> Self {
        Self {
            transcript: None,
            fail: true,
            drops: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl StreamingSpeechRecognizer for FakeRecognizer {
    async fn start(
        &self,
        config: SpeechStreamConfig,
    ) -> Result<Box<dyn SpeechStreamSession>, SpeechProviderError> {
        assert_eq!(config.sample_rate_hz, 16_000);
        Ok(Box::new(FakeSession {
            transcript: self.transcript.clone(),
            fail: self.fail,
            drops: Arc::clone(&self.drops),
        }))
    }
}

struct FakeSession {
    transcript: Option<String>,
    fail: bool,
    drops: Arc<AtomicUsize>,
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl SpeechStreamSession for FakeSession {
    async fn push_audio(
        &mut self,
        _audio: &[u8],
    ) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        Ok(Vec::new())
    }

    async fn finish(&mut self) -> Result<Vec<TranscriptUpdate>, SpeechProviderError> {
        if self.fail {
            return Err(SpeechProviderError::safe("safe failure"));
        }
        Ok(self
            .transcript
            .clone()
            .map(|transcript| {
                vec![TranscriptUpdate {
                    transcript,
                    is_final: true,
                }]
            })
            .unwrap_or_default())
    }
}

struct FakeInterpreter {
    transcript: String,
}

impl FakeInterpreter {
    fn new(transcript: &str) -> Self {
        Self {
            transcript: transcript.to_owned(),
        }
    }
}

#[async_trait]
impl CommandInterpreter for FakeInterpreter {
    async fn interpret(
        &self,
        transcript: &str,
        _locale: &str,
    ) -> Result<VoiceCommand, CommandInterpretationError> {
        assert_eq!(transcript, self.transcript);
        // Stand-in for the production LLM classifier: identical transcripts in
        // any language map to the same typed intent, proving the server path
        // below the interpreter is language-agnostic.
        let (intent, volume_level, volume_delta, query) = match transcript {
            "play rock" => (
                Intent::PlayRadio,
                None,
                None,
                Some(RadioQuery {
                    genres: vec!["rock".to_owned()],
                    ..RadioQuery::default()
                }),
            ),
            "stop" => (Intent::Stop, None, None, None),
            "volume 37" => (Intent::SetVolume, Some(37), None, None),
            "resume" | "продолжай воспроизведение" => {
                (Intent::Resume, None, None, None)
            }
            "louder" | "громче" => (Intent::VolumeChange, None, Some(5), None),
            "quieter" | "тише" => (Intent::VolumeChange, None, Some(-5), None),
            "next" | "дальше" => (Intent::NextStation, None, None, None),
            "previous" | "назад" => (Intent::PreviousStation, None, None, None),
            "like" | "лайк" => (Intent::AddFavorite, None, None, None),
            _ => (Intent::Unknown, None, None, None),
        };
        Ok(VoiceCommand {
            intent,
            query,
            volume_delta,
            volume_level,
        })
    }
}

#[derive(Default)]
struct FakeResolver {
    sessions: Vec<(SecretHash, ActiveSession)>,
}

impl FakeResolver {
    fn new(token: &str, user_id: Uuid, device_id: Uuid) -> Self {
        let sessions = vec![(
            SecretHash::new(Sha256::digest(token.as_bytes()).into()),
            ActiveSession {
                session_id: Uuid::new_v4(),
                user_id,
                device_id,
            },
        )];
        Self { sessions }
    }
}

#[async_trait]
impl NativeSessionResolver for FakeResolver {
    async fn resolve_active_native_session(
        &self,
        access_hash: &SecretHash,
    ) -> Result<Option<ActiveSession>, NativeSessionLookupError> {
        Ok(self
            .sessions
            .iter()
            .find(|(hash, _)| hash == access_hash)
            .map(|(_, session)| *session))
    }
}

struct FakeRepository {
    stations: Vec<RankedStation>,
    delay: Duration,
}

impl FakeRepository {
    fn new(stations: Vec<RankedStation>) -> Self {
        Self {
            stations,
            delay: Duration::ZERO,
        }
    }
}

#[async_trait]
impl StationRepository for FakeRepository {
    async fn search(
        &self,
        _query: &SearchQuery,
        constraints: &SearchConstraints,
        _embedding: Option<&rockserver::search::Embedding>,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        tokio::time::sleep(self.delay).await;
        Ok(self
            .stations
            .iter()
            .take(constraints.limit)
            .cloned()
            .collect())
    }

    async fn check_readiness(&self) -> Result<(), RepositoryError> {
        Ok(())
    }

    async fn get_public(&self, id: &str) -> Result<Option<Station>, RepositoryError> {
        Ok(self
            .stations
            .iter()
            .find(|station| station.station.id == id)
            .map(|station| station.station.clone()))
    }
}

type Stored = (Uuid, DeviceId, CommandReservation, Option<CommandResult>);

#[derive(Default)]
struct MemoryStore {
    commands: Mutex<HashMap<CommandId, Stored>>,
}

#[async_trait]
impl DeviceControlStore for MemoryStore {
    async fn apply_manifest(
        &self,
        _: Uuid,
        _: DeviceId,
        _: DeviceManifest,
    ) -> Result<StoreOutcome, StoreError> {
        Ok(StoreOutcome::Accepted)
    }

    async fn load_manifest(
        &self,
        _: Uuid,
        _: DeviceId,
    ) -> Result<Option<DeviceManifest>, StoreError> {
        Ok(None)
    }

    async fn list_entities(&self, _: Uuid, _: DeviceId) -> Result<Vec<Entity>, StoreError> {
        Ok(Vec::new())
    }

    async fn list_surfaces(&self, _: Uuid, _: DeviceId) -> Result<Vec<Surface>, StoreError> {
        Ok(Vec::new())
    }

    async fn list_capabilities(
        &self,
        _: Uuid,
        _: DeviceId,
    ) -> Result<Vec<DeviceCapability>, StoreError> {
        Ok(Vec::new())
    }

    async fn store_device_state(
        &self,
        _: Uuid,
        _: DeviceId,
        _: DeviceStateSnapshot,
    ) -> Result<StoreOutcome, StoreError> {
        Ok(StoreOutcome::Accepted)
    }

    async fn load_device_state(
        &self,
        _: Uuid,
        _: DeviceId,
    ) -> Result<Option<DeviceStateSnapshot>, StoreError> {
        Ok(None)
    }

    async fn store_entity_state(
        &self,
        _: Uuid,
        _: DeviceId,
        _: EntityState,
    ) -> Result<StoreOutcome, StoreError> {
        Ok(StoreOutcome::Accepted)
    }

    async fn load_entity_state(
        &self,
        _: Uuid,
        _: DeviceId,
        _: &str,
    ) -> Result<Option<EntityState>, StoreError> {
        Ok(None)
    }

    async fn reserve_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        request: CommandReservation,
    ) -> Result<StoreOutcome, StoreError> {
        self.commands.lock().unwrap().insert(
            request.command.command_id,
            (user_id, device_id, request, None),
        );
        Ok(StoreOutcome::Accepted)
    }

    async fn load_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        command_id: CommandId,
    ) -> Result<Option<CommandLifecycle>, StoreError> {
        Ok(self
            .commands
            .lock()
            .unwrap()
            .get(&command_id)
            .filter(|(owner, target, _, _)| *owner == user_id && *target == device_id)
            .map(|(_, _, request, result)| CommandLifecycle {
                command: request.command.clone(),
                result: result.clone(),
            }))
    }

    async fn complete_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        result: CommandResult,
    ) -> Result<StoreOutcome, StoreError> {
        let mut commands = self.commands.lock().unwrap();
        let Some((owner, target, _, stored_result)) = commands.get_mut(&result.command_id) else {
            return Ok(StoreOutcome::NotOwned);
        };
        if *owner != user_id || *target != device_id || stored_result.is_some() {
            return Ok(StoreOutcome::Conflict);
        }
        *stored_result = Some(result);
        Ok(StoreOutcome::Accepted)
    }

    async fn prune_commands(&self, _: u32) -> Result<u64, StoreError> {
        Ok(0)
    }
}

fn radio_manifest() -> DeviceManifest {
    DeviceManifest {
        manifest_revision: 1,
        roles: vec![
            DeviceRole::Controller,
            DeviceRole::Player,
            DeviceRole::VoiceEndpoint,
        ],
        capabilities: DeviceCapabilities {
            revision: 1,
            items: vec![
                DeviceCapability::Playback {
                    actions: vec!["play".to_owned(), "stop".to_owned()],
                },
                DeviceCapability::Station {
                    sources: vec![CATALOG_STATION_SOURCE.to_owned()],
                },
                DeviceCapability::Volume {
                    step: 1,
                    mute: true,
                },
                DeviceCapability::VoiceInput {
                    formats: vec!["pcm16_mono_16000".to_owned()],
                },
            ],
        },
        entities: Vec::new(),
        surfaces: vec![Surface {
            surface_id: "voice.main".to_owned(),
            kind: SurfaceKind::Voice,
            label: "Voice".to_owned(),
            views: Vec::new(),
        }],
    }
}

/// Manifest of a future firmware that declares playback navigation actions.
fn navigating_manifest() -> DeviceManifest {
    let mut manifest = radio_manifest();
    let Some(DeviceCapability::Playback { actions }) = manifest
        .capabilities
        .items
        .iter_mut()
        .find(|capability| matches!(capability, DeviceCapability::Playback { .. }))
    else {
        panic!("radio manifest must contain a playback capability");
    };
    actions.extend(["next".to_owned(), "previous".to_owned()]);
    manifest
}

fn stations(count: usize) -> Vec<RankedStation> {
    (1..=count)
        .map(|index| RankedStation {
            station: Station {
                id: format!("station-{index}"),
                name: format!("Station {index}"),
                stream_url: format!("https://streams.example.com/{index}.mp3"),
                homepage_url: None,
                favicon_url: None,
                tags: vec!["rock".to_owned()],
                language: None,
                country_code: None,
                codec: Some("MP3".to_owned()),
                bitrate_kbps: Some(128),
                health: StationHealth::Healthy,
            },
            score: 0.9,
            reason: "fixture".to_owned(),
            total_matches: None,
        })
        .collect()
}

async fn server(app: axum::Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, server)
}

async fn websocket_request(address: std::net::SocketAddr, token: Option<&str>) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let authorization = token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "GET /api/v1/voice/stream HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{authorization}X-Request-Id: stream-test\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream
}

async fn start(stream: &mut TcpStream, surface: bool, sample_rate: u32) {
    let surface = if surface {
        r#", "surface_id":"voice.main""#
    } else {
        ""
    };
    let payload = format!(
        r#"{{"type":"start","locale":"en-US","sample_rate_hz":{sample_rate},"recognizer_mode":"streaming_v3"{surface}}}"#
    );
    write_client_frame(stream, 0x1, payload.as_bytes()).await;
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

#[derive(Clone, Default)]
struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = LogSink;

    fn make_writer(&'a self) -> Self::Writer {
        LogSink(Arc::clone(&self.0))
    }
}

struct LogSink(Arc<Mutex<Vec<u8>>>);

impl Write for LogSink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

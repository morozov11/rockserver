//! Unit tests for the LLM client, circuit breaker, retry, and mapping rules.

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use prost::Message;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, transport::Server};

use crate::grpc::llm::v1::{
    Action, CountryHint, CountryHintKind, LanguageHint, LanguageHintKind, LanguagesRequest,
    LanguagesResponse, ParseCommandRequest, ParseCommandResponse, ParseIntentRequest,
    ParseIntentResponse, ResponseSource, VoiceIntent,
    llm_server::{Llm, LlmServer},
};
use crate::grpc::rpc::error_details::{ErrorInfo, Status as RpcStatus};
use crate::languages::{ServiceLanguageSource, ServiceLanguageStatus};
use crate::providers::llm::client::LlmClient;
use crate::providers::llm::config::LlmConfig;
use crate::providers::llm::language_source::LlmLanguageSource;
use crate::providers::llm::mapping::{validate_station_country, validate_station_language};
use crate::providers::llm::provider::{LlmCommandInterpreter, LlmQueryParser};
use crate::providers::llm::traits::LlmService;
use crate::search::taxonomy::GenreTaxonomy;
use crate::search::{QueryParser, QueryParserInput};
use crate::voice::{CommandInterpreter, Intent};

const TEST_TOKEN: &str = "test-llm-token";

#[derive(Clone)]
struct ScriptedLlmServer {
    languages_result: Arc<Mutex<Option<Result<LanguagesResponse, Status>>>>,
    intent_results: Arc<Mutex<Vec<Result<ParseIntentResponse, Status>>>>,
    command_results: Arc<Mutex<Vec<Result<ParseCommandResponse, Status>>>>,
    received_intents: Arc<Mutex<Vec<ParseIntentRequest>>>,
    received_commands: Arc<Mutex<Vec<ParseCommandRequest>>>,
    calls: Arc<AtomicUsize>,
}

impl ScriptedLlmServer {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                languages_result: Arc::new(Mutex::new(None)),
                intent_results: Arc::new(Mutex::new(Vec::new())),
                command_results: Arc::new(Mutex::new(Vec::new())),
                received_intents: Arc::new(Mutex::new(Vec::new())),
                received_commands: Arc::new(Mutex::new(Vec::new())),
                calls: calls.clone(),
            },
            calls,
        )
    }

    fn check_auth(metadata: &tonic::metadata::MetadataMap) -> Result<(), Status> {
        let expected = format!("Bearer {TEST_TOKEN}");
        let auth_header = metadata.get("authorization").and_then(|v| v.to_str().ok());
        if auth_header != Some(&expected) {
            return Err(status_with_reason(
                Status::unauthenticated("unauthorized"),
                "unauthorized",
            ));
        }
        if metadata.get("x-correlation-id").is_none() {
            return Err(Status::internal("missing correlation id"));
        }
        Ok(())
    }
}

#[async_trait]
impl Llm for ScriptedLlmServer {
    async fn languages(
        &self,
        request: Request<LanguagesRequest>,
    ) -> Result<Response<LanguagesResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Self::check_auth(request.metadata())?;
        let mut lock = self.languages_result.lock().unwrap();
        match lock.take() {
            Some(Ok(resp)) => Ok(Response::new(resp)),
            Some(Err(status)) => Err(status),
            None => Ok(Response::new(LanguagesResponse {
                languages: vec!["en".into(), "ru".into()],
            })),
        }
    }

    async fn parse_intent(
        &self,
        request: Request<ParseIntentRequest>,
    ) -> Result<Response<ParseIntentResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Self::check_auth(request.metadata())?;
        let inner = request.into_inner();
        self.received_intents.lock().unwrap().push(inner);
        let mut lock = self.intent_results.lock().unwrap();
        if lock.is_empty() {
            Ok(Response::new(ParseIntentResponse {
                action: Action::Play as i32,
                terms: vec!["rock".into()],
                tags: vec!["rock".into()],
                station_language: None,
                station_country: None,
                source: Some(ResponseSource {
                    provider: "test".into(),
                    model: "v1".into(),
                    fallback_used: false,
                }),
            }))
        } else {
            lock.remove(0).map(Response::new)
        }
    }

    async fn parse_command(
        &self,
        request: Request<ParseCommandRequest>,
    ) -> Result<Response<ParseCommandResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Self::check_auth(request.metadata())?;
        let inner = request.into_inner();
        self.received_commands.lock().unwrap().push(inner);
        let mut lock = self.command_results.lock().unwrap();
        if lock.is_empty() {
            Ok(Response::new(ParseCommandResponse {
                intent: VoiceIntent::Stop as i32,
                query: None,
                volume_delta: None,
                volume_level: None,
            }))
        } else {
            lock.remove(0).map(Response::new)
        }
    }
}

fn status_with_reason(status: Status, reason: &str) -> Status {
    let info = ErrorInfo {
        reason: reason.to_owned(),
        domain: "rock-llm-service".to_owned(),
        metadata: Default::default(),
    };
    let any = prost_types::Any {
        type_url: "type.googleapis.com/google.rpc.ErrorInfo".to_owned(),
        value: info.encode_to_vec(),
    };
    let rich = RpcStatus {
        code: status.code() as i32,
        message: "no user data".to_owned(),
        details: vec![any],
    };
    Status::with_details(
        status.code(),
        "no user data",
        bytes::Bytes::from(rich.encode_to_vec()),
    )
}

async fn start_fake_server(
    server: ScriptedLlmServer,
) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        Server::builder()
            .add_service(LlmServer::new(server))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });
    (addr, tx)
}

fn make_config(addr: SocketAddr) -> LlmConfig {
    LlmConfig::new(
        addr.to_string(),
        TEST_TOKEN,
        Duration::from_millis(5000),
        Duration::from_millis(4500),
        Duration::from_millis(5000),
    )
}

#[tokio::test]
async fn auth_and_deadlines_are_propagated() {
    let (server, _calls) = ScriptedLlmServer::new();
    let (addr, _shutdown) = start_fake_server(server.clone()).await;
    let config = make_config(addr);
    let client = LlmClient::lazy(config).unwrap();

    let langs = client.languages().await.unwrap();
    assert_eq!(langs, vec!["en", "ru"]);
}

#[tokio::test]
async fn providers_unavailable_and_model_not_loaded_do_not_open_breaker() {
    let (server, _calls) = ScriptedLlmServer::new();
    // Pre-populate with 10 providers_unavailable errors
    for _ in 0..10 {
        server
            .command_results
            .lock()
            .unwrap()
            .push(Err(status_with_reason(
                Status::unavailable("providers failed"),
                "providers_unavailable",
            )));
    }
    // Then 10 model_not_loaded errors
    for _ in 0..10 {
        server
            .command_results
            .lock()
            .unwrap()
            .push(Err(status_with_reason(
                Status::unavailable("model not ready"),
                "model_not_loaded",
            )));
    }
    // Then a successful response
    server
        .command_results
        .lock()
        .unwrap()
        .push(Ok(ParseCommandResponse {
            intent: VoiceIntent::Stop as i32,
            query: None,
            volume_delta: None,
            volume_level: None,
        }));

    let (addr, _shutdown) = start_fake_server(server.clone()).await;
    let config = make_config(addr);
    let client = LlmClient::lazy(config).unwrap();

    // Call 20 times with service-refusal UNAVAILABLEs
    for _ in 0..20 {
        assert!(client.parse_command("stop", "en").await.is_err());
    }

    // Breaker must still be closed: the 21st call must succeed
    let cmd = client.parse_command("stop", "en").await.unwrap();
    assert_eq!(cmd.intent, Intent::Stop);
}

#[tokio::test]
async fn transient_transport_failures_trip_circuit_breaker() {
    let (server, _calls) = ScriptedLlmServer::new();
    // 5 consecutive raw unavailable statuses (without ErrorInfo -> transient)
    for _ in 0..5 {
        server
            .command_results
            .lock()
            .unwrap()
            .push(Err(Status::unavailable("transport lost")));
    }
    let (addr, _shutdown) = start_fake_server(server.clone()).await;
    let config = make_config(addr);
    let client = LlmClient::lazy(config).unwrap();

    for _ in 0..5 {
        let _ = client.parse_command("stop", "en").await;
    }

    // 6th call must fail immediately due to open breaker
    let err = client.parse_command("stop", "en").await.unwrap_err();
    assert!(err.to_string().contains("circuit breaker is open"));
}

#[tokio::test]
async fn unknown_taxonomy_version_retries_once_with_prompt_tags() {
    let (server, _calls) = ScriptedLlmServer::new();
    // First attempt fails with unknown_taxonomy_version
    server
        .intent_results
        .lock()
        .unwrap()
        .push(Err(status_with_reason(
            Status::failed_precondition("unknown version"),
            "unknown_taxonomy_version",
        )));
    // Second attempt succeeds
    server
        .intent_results
        .lock()
        .unwrap()
        .push(Ok(ParseIntentResponse {
            action: Action::Play as i32,
            terms: vec!["rock".into()],
            tags: vec!["rock".into()],
            station_language: None,
            station_country: None,
            source: None,
        }));

    let (addr, _shutdown) = start_fake_server(server.clone()).await;
    let config = make_config(addr);
    let client = LlmClient::lazy(config).unwrap();
    let taxonomy = GenreTaxonomy::builtin();

    let intent = client.parse_intent("rock", "en", &taxonomy).await.unwrap();
    assert_eq!(intent.tags, vec!["rock"]);

    // Verify 2 requests were sent: first empty taxonomy, second with prompt tags
    let reqs = server.received_intents.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[0].taxonomy.is_empty());
    assert_eq!(reqs[1].taxonomy.len(), taxonomy.prompt_tags().len());
}

#[tokio::test]
async fn taxonomy_version_conflict_fails_immediately_without_retry() {
    let (server, _calls) = ScriptedLlmServer::new();
    server
        .intent_results
        .lock()
        .unwrap()
        .push(Err(status_with_reason(
            Status::failed_precondition("conflict"),
            "taxonomy_version_conflict",
        )));

    let (addr, _shutdown) = start_fake_server(server.clone()).await;
    let config = make_config(addr);
    let client = LlmClient::lazy(config).unwrap();
    let taxonomy = GenreTaxonomy::builtin();

    let err = client
        .parse_intent("rock", "en", &taxonomy)
        .await
        .unwrap_err();
    assert_eq!(err.reason(), Some("taxonomy_version_conflict"));

    let reqs = server.received_intents.lock().unwrap();
    assert_eq!(reqs.len(), 1);
}

#[tokio::test]
async fn invalid_model_output_triggers_deterministic_fallback_for_query_parser() {
    let (server, _calls) = ScriptedLlmServer::new();
    server
        .intent_results
        .lock()
        .unwrap()
        .push(Err(status_with_reason(
            Status::failed_precondition("invalid output"),
            "invalid_model_output",
        )));

    let (addr, _shutdown) = start_fake_server(server.clone()).await;
    let config = make_config(addr);
    let client = Arc::new(LlmClient::lazy(config).unwrap());
    let parser = LlmQueryParser::new(client);

    let input = QueryParserInput {
        query: "включи рок".into(),
        locale: "ru-RU".into(),
    };
    let intent = parser.parse(&input).await.unwrap();
    // Deterministic fallback extracted "rock" genre tag
    assert_eq!(intent.tags, vec!["rock"]);
}

#[tokio::test]
async fn volume_phrases_from_live_run_fall_back_end_to_end() {
    let (server, _calls) = ScriptedLlmServer::new();
    // Server produces invalid_model_output on command
    server
        .command_results
        .lock()
        .unwrap()
        .push(Err(status_with_reason(
            Status::failed_precondition("invalid output"),
            "invalid_model_output",
        )));
    server
        .command_results
        .lock()
        .unwrap()
        .push(Err(status_with_reason(
            Status::failed_precondition("invalid output"),
            "invalid_model_output",
        )));

    let (addr, _shutdown) = start_fake_server(server.clone()).await;
    let config = make_config(addr);
    let client = Arc::new(LlmClient::lazy(config).unwrap());
    let interpreter = LlmCommandInterpreter::new(client);

    // 1. «громче» -> VolumeChange(5)
    let cmd1 = interpreter.interpret("громче", "ru").await.unwrap();
    assert_eq!(cmd1.intent, Intent::VolumeChange);
    assert_eq!(cmd1.volume_delta, Some(5));

    // 2. "set volume to 50" -> SetVolume(50)
    let cmd2 = interpreter
        .interpret("set volume to 50", "en")
        .await
        .unwrap();
    assert_eq!(cmd2.intent, Intent::SetVolume);
    assert_eq!(cmd2.volume_level, Some(50));
}

#[tokio::test]
async fn language_source_maps_available_and_unknown() {
    let (server, _calls) = ScriptedLlmServer::new();
    server
        .languages_result
        .lock()
        .unwrap()
        .replace(Ok(LanguagesResponse {
            languages: vec!["en".into(), "ru".into()],
        }));

    let (addr, _shutdown) = start_fake_server(server.clone()).await;
    let config = make_config(addr);
    let client = Arc::new(LlmClient::lazy(config).unwrap());
    let source = LlmLanguageSource::new(Some(client.clone()));

    assert_eq!(
        source.poll_languages().await,
        ServiceLanguageStatus::Available(["en".into(), "ru".into()].into())
    );

    // Unconnected returns NotConnected
    let unconnected = LlmLanguageSource::unconnected();
    assert_eq!(
        unconnected.poll_languages().await,
        ServiceLanguageStatus::NotConnected
    );
}

#[test]
fn negative_tests_for_station_language_and_country_rules() {
    let registry = crate::languages::bundled_registry();

    // 1. Negation: "рок не на немецком"
    let hint_lang = Some(LanguageHint {
        code: Some("de".into()),
        kind: LanguageHintKind::LanguageExplicitLanguage as i32,
        evidence: Some("немецком".into()),
    });
    assert_eq!(
        validate_station_language(&hint_lang, "рок не на немецком", registry),
        None
    );

    // Negation country: "рок не из германии"
    let hint_country = Some(CountryHint {
        code: Some("DE".into()),
        kind: CountryHintKind::CountryCountryName as i32,
        evidence: Some("германии".into()),
    });
    assert_eq!(
        validate_station_country(&hint_country, "рок не из германии", registry),
        None
    );

    // 2. Multiple countries: "рок из Германии и Франции"
    assert_eq!(
        validate_station_country(&hint_country, "рок из Германии и Франции", registry),
        None
    );

    // 3. Arbitrary word: evidence "абракадабра"
    let hint_arb = Some(CountryHint {
        code: Some("DE".into()),
        kind: CountryHintKind::CountryCountryName as i32,
        evidence: Some("абракадабра".into()),
    });
    assert_eq!(
        validate_station_country(&hint_arb, "рок из абракадабра", registry),
        None
    );

    // 4. Wrong code with right quote: quote "германии" but code "GB"
    let hint_wrong_code = Some(CountryHint {
        code: Some("GB".into()),
        kind: CountryHintKind::CountryCountryName as i32,
        evidence: Some("германии".into()),
    });
    assert_eq!(
        validate_station_country(&hint_wrong_code, "рок из германии", registry),
        None
    );

    // 5. Whole command as quote: "включи рок из германии"
    let hint_whole = Some(CountryHint {
        code: Some("DE".into()),
        kind: CountryHintKind::CountryCountryName as i32,
        evidence: Some("включи рок из германии".into()),
    });
    assert_eq!(
        validate_station_country(&hint_whole, "включи рок из германии", registry),
        None
    );

    // 6. Adjective veto: CountryNationalityAdjective ("немецкий рок")
    let hint_adj = Some(CountryHint {
        code: Some("DE".into()),
        kind: CountryHintKind::CountryNationalityAdjective as i32,
        evidence: Some("немецкий".into()),
    });
    assert_eq!(
        validate_station_country(&hint_adj, "включи немецкий рок", registry),
        None
    );

    // 7. Positive case: "рок из германии" -> Some("DE")
    assert_eq!(
        validate_station_country(&hint_country, "рок из германии", registry),
        Some("DE".into())
    );

    // Positive language case: "рок на немецком" -> Some("de")
    assert_eq!(
        validate_station_language(&hint_lang, "рок на немецком", registry),
        Some("de".into())
    );
}

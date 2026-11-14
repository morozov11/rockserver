//! Unit tests for the embedder client against an in-process `embed.v1` server.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use prost::Message;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, transport::Server};

use crate::grpc::embed::v1::{
    EmbedQueryRequest, EmbedQueryResponse, EmbedStationsRequest, EmbedStationsResponse,
    InfoRequest, InfoResponse, StationInput, StationVector, embedder_server,
    embedder_server::Embedder,
};
use crate::persistence::{EMBEDDINGS_MODEL, MAX_BATCH_STATIONS, PendingStationEmbedding};
use crate::providers::embedder::{
    EMBED_API_VERSION, EmbedderClient, EmbedderConfig, EmbedderGateway, EmbedderService,
    ServiceEmbeddingProvider,
};
use crate::search::{Embedding, EmbeddingProvenance, EmbeddingProvider, EmbeddingProviderError};

const TOKEN: &str = "test-embedding-token";
const DIM: usize = 4;

/// Scriptable in-process embed.v1 service.
struct FakeEmbedder {
    api_version: u32,
    /// Override for `EmbedQuery` responses only (Info stays valid).
    query_version: Option<u32>,
    delay: Duration,
    /// Scripted failure returned by every RPC instead of a real response.
    scripted_status: Option<Status>,
    calls: Arc<AtomicUsize>,
}

impl FakeEmbedder {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                api_version: EMBED_API_VERSION,
                query_version: None,
                delay: Duration::ZERO,
                scripted_status: None,
                calls: calls.clone(),
            },
            calls,
        )
    }

    fn vector() -> Vec<f32> {
        vec![1.0, 0.0, 0.0, 0.0]
    }

    fn check_auth(request_metadata: &tonic::metadata::MetadataMap) -> Result<(), Status> {
        let expected = format!("Bearer {TOKEN}");
        let authorized = request_metadata
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == expected);
        if !authorized {
            return Err(unauthorized_status());
        }
        if request_metadata.get("x-correlation-id").is_none() {
            return Err(Status::internal("missing correlation id"));
        }
        Ok(())
    }
}

/// Builds the contract `UNAUTHENTICATED`/`unauthorized` status with ErrorInfo.
fn unauthorized_status() -> Status {
    status_with_reason(Status::unauthenticated("no user data"), "unauthorized")
}

/// Builds a status carrying `google.rpc.ErrorInfo` like the real service.
fn status_with_reason(status: Status, reason: &str) -> Status {
    let info = crate::grpc::rpc::error_details::ErrorInfo {
        reason: reason.to_owned(),
        domain: "rock-llm-service".to_owned(),
        metadata: Default::default(),
    };
    let any = prost_types::Any {
        type_url: "type.googleapis.com/google.rpc.ErrorInfo".to_owned(),
        value: info.encode_to_vec(),
    };
    let rich = crate::grpc::rpc::error_details::Status {
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

#[async_trait]
impl Embedder for FakeEmbedder {
    async fn info(&self, request: Request<InfoRequest>) -> Result<Response<InfoResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Self::check_auth(request.metadata())?;
        tokio::time::sleep(self.delay).await;
        if let Some(status) = &self.scripted_status {
            return Err(status.clone());
        }
        Ok(Response::new(InfoResponse {
            version: self.api_version,
            model: EMBEDDINGS_MODEL.to_owned(),
            model_revision: "test-revision".to_owned(),
            dim: DIM as u32,
            vector_version: "v1".to_owned(),
            image_digest: "sha256:test".to_owned(),
        }))
    }

    async fn embed_query(
        &self,
        request: Request<EmbedQueryRequest>,
    ) -> Result<Response<EmbedQueryResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Self::check_auth(request.metadata())?;
        tokio::time::sleep(self.delay).await;
        if let Some(status) = &self.scripted_status {
            return Err(status.clone());
        }
        Ok(Response::new(EmbedQueryResponse {
            version: self.query_version.unwrap_or(self.api_version),
            vector: Self::vector(),
        }))
    }

    async fn embed_stations(
        &self,
        request: Request<EmbedStationsRequest>,
    ) -> Result<Response<EmbedStationsResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Self::check_auth(request.metadata())?;
        if let Some(status) = &self.scripted_status {
            return Err(status.clone());
        }
        let vectors = request
            .into_inner()
            .stations
            .into_iter()
            .map(|station: StationInput| StationVector {
                id: station.id,
                vector: Self::vector(),
            })
            .collect();
        Ok(Response::new(EmbedStationsResponse {
            version: self.api_version,
            vectors,
        }))
    }
}

/// Serves a fake embedder on a loopback port; returns its address and the
/// shared call counter.
async fn serve_fake(fake: FakeEmbedder) -> (SocketAddr, Arc<AtomicUsize>) {
    let calls = fake.calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(embedder_server::EmbedderServer::new(fake))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    (address, calls)
}

fn client_config(address: SocketAddr) -> EmbedderConfig {
    EmbedderConfig::new(
        address.to_string(),
        TOKEN,
        Duration::from_millis(2_000),
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
}

fn pending(id: &str, name: &str) -> PendingStationEmbedding {
    PendingStationEmbedding {
        station_id: id.to_owned(),
        name: name.to_owned(),
        tags: vec!["rock".to_owned()],
        language: Some("en".to_owned()),
        country_code: Some("US".to_owned()),
        fields_hash: "a".repeat(64),
    }
}

#[tokio::test]
async fn info_verifies_provenance_and_rejects_unexpected_api_versions() {
    let (fake, _) = FakeEmbedder::new();
    let fake = FakeEmbedder {
        api_version: EMBED_API_VERSION + 1,
        ..fake
    };
    let (address, _) = serve_fake(fake).await;
    let client = EmbedderClient::lazy(client_config(address)).unwrap();
    let error = client
        .info()
        .await
        .expect_err("unexpected API version must be rejected");
    assert!(error.to_string().contains("unexpected API version"));
}

#[tokio::test]
async fn query_embedding_requires_bearer_token_and_maps_provenance() {
    let (fake, _) = FakeEmbedder::new();
    let (address, _) = serve_fake(fake).await;
    let client = EmbedderClient::lazy(client_config(address)).unwrap();

    let provenance = client.info().await.unwrap();
    assert_eq!(
        provenance,
        EmbeddingProvenance {
            model: EMBEDDINGS_MODEL.to_owned(),
            version: "v1".to_owned(),
            dimension: DIM,
        }
    );
    let embedding = client.embed_query("classic rock radio").await.unwrap();
    assert_eq!(embedding.provenance(), &provenance);
    assert_eq!(embedding.values(), FakeEmbedder::vector().as_slice());

    let bad_token = EmbedderClient::lazy(EmbedderConfig::new(
        address.to_string(),
        "wrong-token",
        Duration::from_secs(2),
        Duration::from_secs(5),
        Duration::from_secs(5),
    ))
    .unwrap();
    let error = bad_token
        .embed_query("rock")
        .await
        .expect_err("unauthorized calls must fail");
    assert!(error.to_string().contains("unauthorized"));
}

#[tokio::test]
async fn batch_embedding_preserves_request_order_and_enforces_limits() {
    let (fake, _) = FakeEmbedder::new();
    let (address, _) = serve_fake(fake).await;
    let client = EmbedderClient::lazy(client_config(address)).unwrap();
    client.info().await.unwrap();

    let batch = vec![
        pending("station-a", "Alpha Rock"),
        pending("station-b", "Beta Rock"),
    ];
    let embeddings = client.embed_stations(&batch).await.unwrap();
    assert_eq!(embeddings.len(), 2);
    assert_eq!(embeddings[0].values(), FakeEmbedder::vector().as_slice());

    let oversized: Vec<_> = (0..=MAX_BATCH_STATIONS)
        .map(|index| pending(&format!("s-{index}"), "Name"))
        .collect();
    let error = client
        .embed_stations(&oversized)
        .await
        .expect_err("oversized batches are refused client-side");
    assert!(error.to_string().contains("contract bounds"));

    let duplicated = vec![pending("same", "A"), pending("same", "B")];
    let error = client
        .embed_stations(&duplicated)
        .await
        .expect_err("duplicate ids are refused client-side");
    assert!(error.to_string().contains("duplicate"));
}

#[tokio::test]
async fn response_with_wrong_version_is_rejected() {
    let (fake, _) = FakeEmbedder::new();
    let fake = FakeEmbedder {
        query_version: Some(EMBED_API_VERSION + 3),
        ..fake
    };
    let (address, _) = serve_fake(fake).await;
    let client = EmbedderClient::lazy(client_config(address)).unwrap();
    client.info().await.unwrap();
    let error = client
        .embed_query("rock")
        .await
        .expect_err("wrong response version must be rejected");
    assert!(error.to_string().contains("unexpected API version"));
}

#[tokio::test]
async fn deadline_failures_open_the_service_breaker() {
    let (fake, _) = FakeEmbedder::new();
    let fake = FakeEmbedder {
        delay: Duration::from_millis(400),
        ..fake
    };
    let (address, calls) = serve_fake(fake).await;
    let config = EmbedderConfig::new(
        address.to_string(),
        TOKEN,
        Duration::from_millis(40),
        Duration::from_secs(5),
        Duration::from_secs(5),
    );
    let client = EmbedderClient::lazy(config).unwrap();
    client.info().await.unwrap();

    for _ in 0..5 {
        assert!(client.embed_query("rock").await.is_err());
    }
    let blocked = client
        .embed_query("rock")
        .await
        .expect_err("breaker must be open after five transient failures");
    assert!(blocked.to_string().contains("circuit breaker"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        6,
        "the blocked call must not reach the service"
    );
}

#[tokio::test]
async fn service_refusals_do_not_open_the_breaker() {
    let (fake, _) = FakeEmbedder::new();
    let fake = FakeEmbedder {
        scripted_status: Some(status_with_reason(
            Status::unavailable("no user data"),
            "model_not_loaded",
        )),
        ..fake
    };
    let (address, calls) = serve_fake(fake).await;
    let client = EmbedderClient::lazy(client_config(address)).unwrap();

    for _ in 0..8 {
        let error = client.embed_query("rock").await.unwrap_err();
        assert!(error.to_string().contains("model_not_loaded"));
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        8,
        "contract refusals must keep the breaker closed"
    );
}

/// A deterministic embedder service used without gRPC for adapter tests.
struct FakeService;

#[async_trait]
impl crate::providers::embedder::EmbedderService for FakeService {
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        Some(EmbeddingProvenance {
            model: EMBEDDINGS_MODEL.to_owned(),
            version: "v1".to_owned(),
            dimension: DIM,
        })
    }

    async fn info(&self) -> Result<EmbeddingProvenance, EmbeddingProviderError> {
        Ok(self.provenance().unwrap())
    }

    async fn embed_query(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
        Ok(Embedding::new(EMBEDDINGS_MODEL, "v1", DIM, FakeEmbedder::vector()).unwrap())
    }

    async fn embed_stations(
        &self,
        batch: &[PendingStationEmbedding],
    ) -> Result<Vec<Embedding>, EmbeddingProviderError> {
        let mut embeddings = Vec::with_capacity(batch.len());
        for _ in batch {
            embeddings
                .push(Embedding::new(EMBEDDINGS_MODEL, "v1", DIM, FakeEmbedder::vector()).unwrap());
        }
        Ok(embeddings)
    }
}

#[tokio::test]
async fn gateway_degrades_until_the_lifecycle_enables_it() {
    let gateway = EmbedderGateway::disabled();
    assert!(!gateway.is_enabled());
    assert_eq!(gateway.provenance(), None);
    assert!(gateway.embed("rock").await.is_err());

    gateway.enable(Arc::new(ServiceEmbeddingProvider::new(Arc::new(
        FakeService,
    ))));
    assert!(gateway.is_enabled());
    assert_eq!(gateway.provenance().unwrap().dimension, DIM);
    assert!(gateway.embed("rock").await.is_ok());
    assert!(gateway.supports_semantic_intent_filters());
}

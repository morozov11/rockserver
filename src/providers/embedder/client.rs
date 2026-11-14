//! gRPC client of the embedding service (`embed.v1`).

use std::{
    sync::RwLock,
    time::{Duration, Instant},
};

use tonic::{
    Request,
    transport::{Channel, Endpoint},
};
use uuid::Uuid;

use crate::grpc::embed::v1::{
    EmbedQueryRequest, EmbedQueryResponse, EmbedStationsRequest, EmbedStationsResponse,
    InfoRequest, InfoResponse, StationInput, embedder_client::EmbedderClient as GrpcEmbedderClient,
};
use crate::persistence::{MAX_BATCH_STATIONS, PendingStationEmbedding};
use crate::search::{
    CircuitBreaker, CircuitBreakerPolicy, Embedding, EmbeddingProvenance, EmbeddingProviderError,
};

use super::config::EmbedderConfig;
use super::error::{EmbedderFailure, classify, safe_status_summary};
use super::traits::EmbedderService;

/// API version of the vendored `embed.v1` contract this client speaks.
pub const EMBED_API_VERSION: u32 = 1;

/// Contract limit for `EmbedQuery` text after trimming.
const MAX_QUERY_TEXT_CHARS: usize = 512;

/// A failed call together with its breaker outcome.
struct CallFailure {
    error: EmbeddingProviderError,
    kind: EmbedderFailure,
}

impl CallFailure {
    /// Maps a gRPC status failure per the error contract.
    fn from_status(context: &str, status: &tonic::Status) -> Self {
        let kind = classify(status);
        Self {
            error: EmbeddingProviderError::safe(safe_status_summary(context, status)),
            kind,
        }
    }

    /// A client-side contract violation; the service is not to blame.
    fn refused(summary: impl Into<String>) -> Self {
        Self {
            error: EmbeddingProviderError::safe(summary),
            kind: EmbedderFailure::ServiceRefused,
        }
    }
}

type CallResult<T> = Result<T, CallFailure>;

/// tonic client of `embed.v1` with deadlines, bearer auth, and one breaker.
///
/// The channel is created lazily, so construction never fails while the
/// service is down; the breaker absorbs repeated transport failures.
pub struct EmbedderClient {
    inner: GrpcEmbedderClient<Channel>,
    config: EmbedderConfig,
    breaker: CircuitBreaker,
    provenance: RwLock<Option<EmbeddingProvenance>>,
}

impl EmbedderClient {
    /// Creates the client without connecting; calls dial on first use.
    pub fn lazy(config: EmbedderConfig) -> Result<Self, EmbeddingProviderError> {
        let endpoint_uri = format!("http://{}", config.address());
        let endpoint = Endpoint::from_shared(endpoint_uri)
            .map_err(|_| EmbeddingProviderError::safe("embedding service address is invalid"))?;
        Ok(Self {
            inner: GrpcEmbedderClient::new(endpoint.connect_lazy()),
            breaker: CircuitBreaker::with_policy(CircuitBreakerPolicy::service_contract()),
            provenance: RwLock::new(None),
            config,
        })
    }

    /// Applies the request deadline, bearer token, and correlation metadata.
    fn request<T>(&self, message: T, timeout: Duration) -> Request<T> {
        let mut request = Request::new(message);
        request.set_timeout(timeout);
        if let Ok(value) = format!("Bearer {}", self.config.token()).parse() {
            request.metadata_mut().insert("authorization", value);
        }
        if let Ok(value) = Uuid::new_v4().to_string().parse() {
            request.metadata_mut().insert("x-correlation-id", value);
        }
        request
    }

    /// Runs one guarded call through the shared breaker.
    async fn guarded<T>(
        &self,
        context: &str,
        call: impl std::future::Future<Output = Result<T, tonic::Status>>,
    ) -> Result<T, EmbeddingProviderError> {
        let permit = self.breaker.admit(Instant::now()).ok_or_else(|| {
            EmbeddingProviderError::safe("embedding service circuit breaker is open")
        })?;
        let outcome: CallResult<T> = match call.await {
            Ok(value) => Ok(value),
            Err(status) => Err(CallFailure::from_status(context, &status)),
        };
        let transient = matches!(&outcome, Err(failure) if failure.kind.is_transient());
        permit.finish(transient, Instant::now());
        outcome.map_err(|failure| failure.error)
    }

    fn remember_provenance(&self, provenance: &EmbeddingProvenance) {
        *self.provenance.write().unwrap() = Some(provenance.clone());
    }
}

#[async_trait::async_trait]
impl EmbedderService for EmbedderClient {
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        self.provenance.read().unwrap().clone()
    }

    async fn info(&self) -> Result<EmbeddingProvenance, EmbeddingProviderError> {
        let request = self.request(InfoRequest {}, self.config.info_timeout());
        let response = self
            .guarded("embedding service info", async {
                self.inner.clone().info(request).await
            })
            .await?;
        match verify_info(response.into_inner()) {
            Ok(provenance) => {
                self.remember_provenance(&provenance);
                Ok(provenance)
            }
            Err(failure) => {
                self.provenance.write().unwrap().take();
                Err(failure.error)
            }
        }
    }

    async fn embed_query(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        let trimmed = text.trim();
        let char_count = trimmed.chars().count();
        if char_count == 0 || char_count > MAX_QUERY_TEXT_CHARS {
            return Err(CallFailure::refused("embedding query text is empty or too long").error);
        }
        let request = self.request(
            EmbedQueryRequest {
                text: trimmed.to_owned(),
            },
            self.config.query_timeout(),
        );
        let response = self
            .guarded("embedding service query", async {
                self.inner.clone().embed_query(request).await
            })
            .await?;
        verify_query_response(response.into_inner(), &self.provenance())
            .map(|(_, embedding)| embedding)
            .map_err(|failure| failure.error)
    }

    async fn embed_stations(
        &self,
        batch: &[PendingStationEmbedding],
    ) -> Result<Vec<Embedding>, EmbeddingProviderError> {
        if batch.is_empty() || batch.len() > MAX_BATCH_STATIONS {
            return Err(CallFailure::refused(
                "embedding station batch size is out of contract bounds",
            )
            .error);
        }
        let mut seen_ids = std::collections::BTreeSet::new();
        for station in batch {
            if station.name.trim().is_empty() || !seen_ids.insert(station.station_id.as_str()) {
                return Err(CallFailure::refused(
                    "embedding station batch contains an empty name or duplicate id",
                )
                .error);
            }
        }
        let stations = batch
            .iter()
            .map(|station| StationInput {
                id: station.station_id.clone(),
                name: station.name.clone(),
                tags: station.tags.clone(),
                language: station.language.clone(),
                country_code: station.country_code.clone(),
            })
            .collect::<Vec<_>>();
        let expected_ids = batch
            .iter()
            .map(|station| station.station_id.clone())
            .collect::<Vec<_>>();
        let request = self.request(
            EmbedStationsRequest { stations },
            self.config.batch_timeout(),
        );
        let response = self
            .guarded("embedding service batch", async {
                self.inner.clone().embed_stations(request).await
            })
            .await?;
        verify_station_response(response.into_inner(), &self.provenance(), &expected_ids)
            .map_err(|failure| failure.error)
    }
}

/// Validates an `Info` response against the vendored contract version.
fn verify_info(response: InfoResponse) -> CallResult<EmbeddingProvenance> {
    if response.version != EMBED_API_VERSION {
        return Err(CallFailure::refused(
            "embedding service speaks an unexpected API version",
        ));
    }
    if response.model.trim().is_empty() || response.vector_version.trim().is_empty() {
        return Err(CallFailure::refused(
            "embedding service info is missing model identity",
        ));
    }
    if response.dim == 0 {
        return Err(CallFailure::refused(
            "embedding service info declares no dimension",
        ));
    }
    Ok(EmbeddingProvenance {
        model: response.model,
        version: response.vector_version,
        dimension: response.dim as usize,
    })
}

/// Validates an `EmbedQuery` response; requires known provenance.
fn verify_query_response(
    response: EmbedQueryResponse,
    provenance: &Option<EmbeddingProvenance>,
) -> CallResult<(EmbeddingProvenance, Embedding)> {
    if response.version != EMBED_API_VERSION {
        return Err(CallFailure::refused(
            "embedding service response carries an unexpected API version",
        ));
    }
    let Some(provenance) = provenance else {
        return Err(CallFailure::refused(
            "embedding service provenance was not verified by info",
        ));
    };
    let embedding = Embedding::new(
        provenance.model.clone(),
        provenance.version.clone(),
        provenance.dimension,
        response.vector,
    )
    .map_err(|error| {
        CallFailure::refused(format!("embedding service vector is invalid: {error}"))
    })?;
    Ok((provenance.clone(), embedding))
}

/// Validates an `EmbedStations` response: version, one vector per request id
/// in the same order, each dimensionally valid.
fn verify_station_response(
    response: EmbedStationsResponse,
    provenance: &Option<EmbeddingProvenance>,
    expected_ids: &[String],
) -> CallResult<Vec<Embedding>> {
    if response.version != EMBED_API_VERSION {
        return Err(CallFailure::refused(
            "embedding service response carries an unexpected API version",
        ));
    }
    let Some(provenance) = provenance else {
        return Err(CallFailure::refused(
            "embedding service provenance was not verified by info",
        ));
    };
    if response.vectors.len() != expected_ids.len() {
        return Err(CallFailure::refused(
            "embedding service batch response size mismatch",
        ));
    }
    response
        .vectors
        .into_iter()
        .zip(expected_ids)
        .map(|(vector, expected_id)| {
            if &vector.id != expected_id {
                return Err(CallFailure::refused(
                    "embedding service batch response order mismatch",
                ));
            }
            Embedding::new(
                provenance.model.clone(),
                provenance.version.clone(),
                provenance.dimension,
                vector.vector,
            )
            .map_err(|error| {
                CallFailure::refused(format!("embedding service vector is invalid: {error}"))
            })
        })
        .collect()
}

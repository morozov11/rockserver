//! gRPC client of the neural LLM service (`llm.v1`).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use tonic::{
    Request,
    transport::{Channel, Endpoint},
};
use uuid::Uuid;

use crate::grpc::llm::v1::{
    LanguagesRequest, ParseCommandRequest, ParseIntentRequest,
    llm_client::LlmClient as GrpcLlmClient,
};
use crate::languages::extract_base_language;
use crate::search::taxonomy::GenreTaxonomy;
use crate::search::{CircuitBreaker, CircuitBreakerPolicy, QueryIntent};
use crate::voice::VoiceCommand;

use super::config::LlmConfig;
use super::error::{LlmClientError, classify};
use super::mapping::{map_command_response, map_intent_response};
use super::traits::LlmService;

/// Minimum remaining deadline required to attempt a taxonomy retry.
const MIN_RETRY_DEADLINE: Duration = Duration::from_millis(500);

/// tonic client of `llm.v1` with deadlines, bearer auth, and one circuit breaker.
pub struct LlmClient {
    inner: GrpcLlmClient<Channel>,
    config: LlmConfig,
    breaker: CircuitBreaker,
}

impl LlmClient {
    /// Creates the client without connecting immediately; calls dial on first use.
    pub fn lazy(config: LlmConfig) -> Result<Self, LlmClientError> {
        let endpoint_uri = format!("http://{}", config.address());
        let endpoint = Endpoint::from_shared(endpoint_uri)
            .map_err(|_| LlmClientError::safe("LLM service address is invalid"))?;
        Ok(Self {
            inner: GrpcLlmClient::new(endpoint.connect_lazy()),
            breaker: CircuitBreaker::with_policy(CircuitBreakerPolicy::service_contract()),
            config,
        })
    }

    /// Accesses the underlying client configuration.
    pub fn config(&self) -> &LlmConfig {
        &self.config
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

    /// Runs one guarded call through the shared circuit breaker.
    async fn guarded<T>(
        &self,
        context: &str,
        call: impl std::future::Future<Output = Result<T, tonic::Status>>,
    ) -> Result<T, LlmClientError> {
        let permit = self
            .breaker
            .admit(Instant::now())
            .ok_or_else(|| LlmClientError::transient("LLM service circuit breaker is open"))?;
        let outcome = call.await;
        match outcome {
            Ok(value) => {
                permit.finish(false, Instant::now());
                Ok(value)
            }
            Err(status) => {
                let failure = classify(&status);
                permit.finish(failure.is_transient(), Instant::now());
                let error = LlmClientError::from_status(context, &status);
                Err(error)
            }
        }
    }
}

#[async_trait]
impl LlmService for LlmClient {
    async fn parse_intent(
        &self,
        query: &str,
        locale: &str,
        taxonomy: &GenreTaxonomy,
    ) -> Result<QueryIntent, LlmClientError> {
        let language = extract_base_language(locale);
        let start = Instant::now();
        let total_timeout = self.config.intent_timeout();

        let req1 = self.request(
            ParseIntentRequest {
                text: query.to_owned(),
                language: language.clone(),
                taxonomy_version: taxonomy.version().to_owned(),
                taxonomy: vec![],
            },
            total_timeout,
        );

        let res1 = self
            .guarded("llm service parse_intent", async {
                self.inner.clone().parse_intent(req1).await
            })
            .await;

        let response = match res1 {
            Ok(resp) => resp.into_inner(),
            Err(err) if err.reason() == Some("unknown_taxonomy_version") => {
                let elapsed = start.elapsed();
                let remaining = total_timeout.saturating_sub(elapsed);
                if remaining >= MIN_RETRY_DEADLINE {
                    let prompt_tags = taxonomy
                        .prompt_tags()
                        .iter()
                        .map(String::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    let req2 = self.request(
                        ParseIntentRequest {
                            text: query.to_owned(),
                            language,
                            taxonomy_version: taxonomy.version().to_owned(),
                            taxonomy: prompt_tags,
                        },
                        remaining,
                    );
                    let res2 = self
                        .guarded("llm service parse_intent retry", async {
                            self.inner.clone().parse_intent(req2).await
                        })
                        .await?;
                    res2.into_inner()
                } else {
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        };

        map_intent_response(response, query, taxonomy)
    }

    async fn parse_command(
        &self,
        transcript: &str,
        locale: &str,
    ) -> Result<VoiceCommand, LlmClientError> {
        let language = extract_base_language(locale);
        let req = self.request(
            ParseCommandRequest {
                text: transcript.to_owned(),
                language,
            },
            self.config.command_timeout(),
        );

        let response = self
            .guarded("llm service parse_command", async {
                self.inner.clone().parse_command(req).await
            })
            .await?;

        map_command_response(response.into_inner())
    }

    async fn languages(&self) -> Result<Vec<String>, LlmClientError> {
        let req = self.request(LanguagesRequest {}, self.config.info_timeout());
        let response = self
            .guarded("llm service languages", async {
                self.inner.clone().languages(req).await
            })
            .await?;

        let mut langs = response.into_inner().languages;
        langs.sort();
        langs.dedup();
        Ok(langs)
    }
}

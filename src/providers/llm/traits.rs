//! Abstract service boundary for the neural LLM service.

use async_trait::async_trait;

use crate::search::QueryIntent;
use crate::search::taxonomy::GenreTaxonomy;
use crate::voice::VoiceCommand;

use super::error::LlmClientError;

/// Core service operations provided by the neural LLM service (`llm.v1`).
#[async_trait]
pub trait LlmService: Send + Sync {
    /// Dispatches a structured radio query parsing request with taxonomy versioning.
    async fn parse_intent(
        &self,
        query: &str,
        locale: &str,
        taxonomy: &GenreTaxonomy,
    ) -> Result<QueryIntent, LlmClientError>;

    /// Dispatches a structured voice command parsing request.
    async fn parse_command(
        &self,
        transcript: &str,
        locale: &str,
    ) -> Result<VoiceCommand, LlmClientError>;

    /// Queries the service for its loaded communication languages.
    async fn languages(&self) -> Result<Vec<String>, LlmClientError>;
}

//! Search domain, provider boundaries, fallback semantics, and catalog boundary.

mod circuit_breaker;
mod domain;
mod embedding;
mod in_memory;
mod language;
mod query;
mod ranking;
mod service;
pub mod taxonomy;

#[cfg(test)]
mod tests;

pub use crate::providers::llm::LlmQueryParser;
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerPolicy};
pub use domain::{
    MIN_RELEVANCE_SCORE, RankedStation, RepositoryError, SearchConstraints, SearchOutcome,
    SearchQuery, Station, StationHealth, StationRepository,
};
pub use embedding::{
    Embedding, EmbeddingProvenance, EmbeddingProvider, EmbeddingProviderError, EmbeddingStoreError,
    EmbeddingValidationError, MAX_EMBEDDING_DIMENSION,
};
pub use in_memory::{InMemoryStationRepository, UnavailableStationRepository};
pub use query::{
    DeterministicQueryParser, QueryIntent, QueryParser, QueryParserError, QueryParserInput,
    SearchAction, normalize_query, tokenize,
};
pub use ranking::{METADATA_WEIGHT, SEMANTIC_WEIGHT};
pub use service::SearchService;

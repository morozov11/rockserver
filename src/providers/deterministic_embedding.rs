//! Explicit development-only deterministic embedding provider.

use std::{env, error::Error, fmt};

use async_trait::async_trait;

use crate::search::{
    Embedding, EmbeddingProvider, EmbeddingProviderError, MAX_EMBEDDING_DIMENSION,
};

/// Environment variable selecting the optional request-path embedding provider.
pub const SEMANTIC_PROVIDER_ENV: &str = "ROCKSERVER_SEMANTIC_PROVIDER";
/// Environment variable setting the deterministic development vector dimension.
pub const EMBEDDING_DIMENSION_ENV: &str = "ROCKSERVER_EMBEDDING_DIMENSION";
/// Explicit value enabling the non-production deterministic provider.
pub const DETERMINISTIC_DEV_PROVIDER: &str = "deterministic-dev";
/// Provenance model stored for deterministic development embeddings.
pub const DETERMINISTIC_MODEL: &str = "rockserver-deterministic-dev";
/// Provenance version stored for deterministic development embeddings.
pub const DETERMINISTIC_VERSION: &str = "1";
/// Default development dimension; storage itself remains dimension-neutral.
pub const DEFAULT_DETERMINISTIC_DIMENSION: usize = 32;

/// Deterministic hash-based embedder intended only for local development and tests.
#[derive(Clone, Debug)]
pub struct DeterministicEmbeddingProvider {
    dimension: usize,
}

impl DeterministicEmbeddingProvider {
    /// Creates a development provider with a valid pgvector dimension.
    pub fn new(dimension: usize) -> Result<Self, DeterministicEmbeddingConfigError> {
        if dimension == 0 || dimension > MAX_EMBEDDING_DIMENSION {
            return Err(DeterministicEmbeddingConfigError::InvalidDimension(
                dimension,
            ));
        }
        Ok(Self { dimension })
    }

    /// Loads the development dimension from the environment or its documented default.
    pub fn from_env() -> Result<Self, DeterministicEmbeddingConfigError> {
        let dimension = match env::var(EMBEDDING_DIMENSION_ENV) {
            Ok(value) => value
                .parse::<usize>()
                .map_err(|_| DeterministicEmbeddingConfigError::InvalidDimensionText(value))?,
            Err(env::VarError::NotPresent) => DEFAULT_DETERMINISTIC_DIMENSION,
            Err(env::VarError::NotUnicode(_)) => {
                return Err(DeterministicEmbeddingConfigError::InvalidUnicode);
            }
        };
        Self::new(dimension)
    }

    /// Selects the development provider only when explicitly enabled by environment.
    pub fn optional_from_env() -> Result<Option<Self>, DeterministicEmbeddingConfigError> {
        match env::var(SEMANTIC_PROVIDER_ENV) {
            Ok(value) if value == DETERMINISTIC_DEV_PROVIDER => Self::from_env().map(Some),
            Ok(value) => Err(DeterministicEmbeddingConfigError::InvalidProvider(value)),
            Err(env::VarError::NotPresent) => Ok(None),
            Err(env::VarError::NotUnicode(_)) => {
                Err(DeterministicEmbeddingConfigError::InvalidProviderUnicode)
            }
        }
    }

    /// Returns the configured vector dimension.
    pub fn dimension(&self) -> usize {
        self.dimension
    }
}

#[async_trait]
impl EmbeddingProvider for DeterministicEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        let mut values = vec![0.0_f32; self.dimension];
        for token in text
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_lowercase)
        {
            let hash = token.bytes().fold(0xcbf29ce484222325_u64, |state, byte| {
                state.wrapping_mul(0x100000001b3) ^ u64::from(byte)
            });
            let index = (hash % self.dimension as u64) as usize;
            values[index] += 1.0;
        }
        if values.iter().all(|value| *value == 0.0) {
            values[0] = 1.0;
        }

        Embedding::new(
            DETERMINISTIC_MODEL,
            DETERMINISTIC_VERSION,
            self.dimension,
            values,
        )
        .map_err(|error| EmbeddingProviderError::safe(error.to_string()))
    }
}

/// Configuration failure for the explicit development embedding provider.
#[derive(Debug)]
pub enum DeterministicEmbeddingConfigError {
    /// Optional provider selector was not the explicit development value.
    InvalidProvider(String),
    /// Optional provider selector was not valid Unicode.
    InvalidProviderUnicode,
    /// Parsed dimension is outside the supported range.
    InvalidDimension(usize),
    /// Dimension text is not a positive integer.
    InvalidDimensionText(String),
    /// Environment value was not valid Unicode.
    InvalidUnicode,
}

impl fmt::Display for DeterministicEmbeddingConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProvider(value) => write!(
                formatter,
                "{SEMANTIC_PROVIDER_ENV} must be {DETERMINISTIC_DEV_PROVIDER:?} when set, got {value:?}"
            ),
            Self::InvalidProviderUnicode => {
                write!(formatter, "{SEMANTIC_PROVIDER_ENV} must be valid Unicode")
            }
            Self::InvalidDimension(value) => write!(
                formatter,
                "{EMBEDDING_DIMENSION_ENV} must be between 1 and {MAX_EMBEDDING_DIMENSION}, got {value}"
            ),
            Self::InvalidDimensionText(value) => write!(
                formatter,
                "{EMBEDDING_DIMENSION_ENV} must be an integer, got {value:?}"
            ),
            Self::InvalidUnicode => {
                write!(formatter, "{EMBEDDING_DIMENSION_ENV} must be valid Unicode")
            }
        }
    }
}

impl Error for DeterministicEmbeddingConfigError {}

#[cfg(test)]
mod tests {
    use crate::search::EmbeddingProvider;

    use super::DeterministicEmbeddingProvider;

    #[tokio::test]
    async fn deterministic_embeddings_are_repeatable_and_dimensioned() {
        let provider = DeterministicEmbeddingProvider::new(8).unwrap();

        let first = provider.embed("calm jazz").await.unwrap();
        let second = provider.embed("calm jazz").await.unwrap();

        assert_eq!(first, second);
        assert_eq!(first.provenance().dimension, 8);
        assert_eq!(first.values().len(), 8);
    }
}

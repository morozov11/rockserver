//! Provider-neutral embeddings and controlled station backfill orchestration.

use std::{error::Error, fmt};

use async_trait::async_trait;

/// Maximum dimension supported by pgvector's unbounded `vector` storage type.
pub const MAX_EMBEDDING_DIMENSION: usize = 16_000;

/// Model identity carried with every query and station embedding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingProvenance {
    /// Provider-neutral model identifier.
    pub model: String,
    /// Model or preprocessing version.
    pub version: String,
    /// Number of vector components.
    pub dimension: usize,
}

/// Validated embedding plus the provenance required for compatible comparisons.
#[derive(Clone, Debug, PartialEq)]
pub struct Embedding {
    provenance: EmbeddingProvenance,
    values: Vec<f32>,
}

impl Embedding {
    /// Validates model identity, declared dimension, finite values, and non-zero norm.
    pub fn new(
        model: impl Into<String>,
        version: impl Into<String>,
        dimension: usize,
        values: Vec<f32>,
    ) -> Result<Self, EmbeddingValidationError> {
        let model = model.into();
        let version = version.into();
        if model.trim().is_empty() {
            return Err(EmbeddingValidationError::EmptyModel);
        }
        if version.trim().is_empty() {
            return Err(EmbeddingValidationError::EmptyVersion);
        }
        if dimension == 0 || dimension > MAX_EMBEDDING_DIMENSION {
            return Err(EmbeddingValidationError::UnsupportedDimension(dimension));
        }
        if values.len() != dimension {
            return Err(EmbeddingValidationError::DimensionMismatch {
                declared: dimension,
                actual: values.len(),
            });
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(EmbeddingValidationError::NonFiniteValue);
        }
        if values.iter().all(|value| *value == 0.0) {
            return Err(EmbeddingValidationError::ZeroVector);
        }

        Ok(Self {
            provenance: EmbeddingProvenance {
                model,
                version,
                dimension,
            },
            values,
        })
    }

    /// Returns the immutable provenance validated with this embedding.
    pub fn provenance(&self) -> &EmbeddingProvenance {
        &self.provenance
    }

    /// Returns the immutable finite, non-zero vector components.
    pub fn values(&self) -> &[f32] {
        &self.values
    }
}

/// Invalid embedding data rejected before it reaches persistence or ranking.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EmbeddingValidationError {
    /// Model identity was blank.
    EmptyModel,
    /// Model version was blank.
    EmptyVersion,
    /// Declared dimension is outside pgvector's supported unbounded-vector range.
    UnsupportedDimension(usize),
    /// Declared and actual dimensions differ.
    DimensionMismatch {
        /// Dimension declared by the provider.
        declared: usize,
        /// Number of returned values.
        actual: usize,
    },
    /// A vector component was NaN or infinite.
    NonFiniteValue,
    /// Cosine similarity is undefined for an all-zero vector.
    ZeroVector,
}

impl fmt::Display for EmbeddingValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyModel => formatter.write_str("embedding model must not be empty"),
            Self::EmptyVersion => formatter.write_str("embedding version must not be empty"),
            Self::UnsupportedDimension(value) => {
                write!(formatter, "embedding dimension {value} is unsupported")
            }
            Self::DimensionMismatch { declared, actual } => write!(
                formatter,
                "embedding declared dimension {declared} but returned {actual} values"
            ),
            Self::NonFiniteValue => formatter.write_str("embedding values must be finite"),
            Self::ZeroVector => formatter.write_str("embedding vector must not be all zero"),
        }
    }
}

impl Error for EmbeddingValidationError {}

/// Safe failure returned by an embedding provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingProviderError {
    summary: String,
}

impl EmbeddingProviderError {
    /// Creates a provider-safe failure summary for logs.
    pub fn safe(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }
}

impl fmt::Display for EmbeddingProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.summary)
    }
}

impl Error for EmbeddingProviderError {}

/// Boundary for embedding one query or one station document at a time.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Declares the stable model, preprocessing version, and output dimension before inference.
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        None
    }

    /// Produces a validated embedding for the supplied text.
    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError>;

    /// Produces a station-document embedding; symmetric models use the query implementation.
    async fn embed_document(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.embed(text).await
    }

    /// Reports whether this provider produces semantic vectors suitable for intent hard filters.
    ///
    /// Test fixtures and deterministic development embeddings return `false` by default because
    /// their vector similarity is not meaningful enough to constrain a station search.
    fn supports_semantic_intent_filters(&self) -> bool {
        false
    }
}

/// Persistence failure returned by the embedding workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingStoreError {
    summary: String,
}

impl EmbeddingStoreError {
    /// Creates a storage-safe failure summary.
    pub fn safe(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }
}

impl fmt::Display for EmbeddingStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.summary)
    }
}

impl Error for EmbeddingStoreError {}

#[cfg(test)]
#[path = "embedding/tests.rs"]
mod tests;

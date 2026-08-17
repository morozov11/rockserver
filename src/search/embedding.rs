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
    /// Produces a validated embedding for the supplied text.
    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError>;

    /// Produces a station-document embedding; symmetric models use the query implementation.
    async fn embed_document(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.embed(text).await
    }
}

/// One station document read by the controlled embedding workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StationEmbeddingDocument {
    /// Stable station identifier used by persistence.
    pub station_id: String,
    /// Provider input assembled from this station only.
    pub text: String,
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

/// Storage boundary used only by the controlled embedding backfill/update workflow.
#[async_trait]
pub trait EmbeddingStore: Send + Sync {
    /// Returns the next stable-ID-ordered page after the optional cursor.
    async fn station_documents(
        &self,
        after_station_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StationEmbeddingDocument>, EmbeddingStoreError>;

    /// Inserts or replaces one station embedding for its exact provenance.
    async fn upsert_embedding(
        &self,
        station_id: &str,
        embedding: &Embedding,
    ) -> Result<(), EmbeddingStoreError>;
}

/// Counts produced by one controlled embedding backfill/update run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EmbeddingBackfillResult {
    /// Station documents read from persistence.
    pub processed: usize,
    /// Embeddings inserted or updated successfully.
    pub updated: usize,
}

/// Runs station embedding generation outside HTTP startup and request paths.
pub struct EmbeddingBackfill<P, S> {
    provider: P,
    store: S,
    page_size: usize,
}

impl<P, S> EmbeddingBackfill<P, S>
where
    P: EmbeddingProvider,
    S: EmbeddingStore,
{
    /// Creates a controlled backfill with a positive bounded page size.
    pub fn new(provider: P, store: S, page_size: usize) -> Self {
        Self {
            provider,
            store,
            page_size: page_size.max(1),
        }
    }

    /// Embeds and upserts all stations in stable ID order.
    pub async fn run(&self) -> Result<EmbeddingBackfillResult, Box<dyn Error + Send + Sync>> {
        let mut result = EmbeddingBackfillResult::default();
        let mut cursor = None::<String>;

        loop {
            let documents = self
                .store
                .station_documents(cursor.as_deref(), self.page_size)
                .await?;
            if documents.is_empty() {
                break;
            }
            let page_len = documents.len();
            for document in documents {
                let embedding = self.provider.embed_document(&document.text).await?;
                self.store
                    .upsert_embedding(&document.station_id, &embedding)
                    .await?;
                cursor = Some(document.station_id);
                result.processed += 1;
                result.updated += 1;
            }
            if page_len < self.page_size {
                break;
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{
        Embedding, EmbeddingBackfill, EmbeddingProvider, EmbeddingProviderError, EmbeddingStore,
        EmbeddingStoreError, EmbeddingValidationError, StationEmbeddingDocument,
    };

    #[test]
    fn validates_model_dimension_values_and_norm() {
        assert_eq!(
            Embedding::new("", "1", 2, vec![1.0, 0.0]).unwrap_err(),
            EmbeddingValidationError::EmptyModel
        );
        assert_eq!(
            Embedding::new("test", "1", 3, vec![1.0, 0.0]).unwrap_err(),
            EmbeddingValidationError::DimensionMismatch {
                declared: 3,
                actual: 2
            }
        );
        assert_eq!(
            Embedding::new("test", "1", 2, vec![0.0, 0.0]).unwrap_err(),
            EmbeddingValidationError::ZeroVector
        );
        assert_eq!(
            Embedding::new("test", "1", 2, vec![f32::NAN, 1.0]).unwrap_err(),
            EmbeddingValidationError::NonFiniteValue
        );
    }

    struct FakeProvider;

    #[async_trait]
    impl EmbeddingProvider for FakeProvider {
        async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
            let first = if text.contains("one") { 1.0 } else { 0.0 };
            Embedding::new("fake", "1", 2, vec![first, 1.0])
                .map_err(|error| EmbeddingProviderError::safe(error.to_string()))
        }
    }

    #[derive(Default)]
    struct FakeStore {
        writes: Mutex<Vec<(String, Embedding)>>,
    }

    #[async_trait]
    impl EmbeddingStore for FakeStore {
        async fn station_documents(
            &self,
            after_station_id: Option<&str>,
            _limit: usize,
        ) -> Result<Vec<StationEmbeddingDocument>, EmbeddingStoreError> {
            Ok(match after_station_id {
                None => vec![
                    StationEmbeddingDocument {
                        station_id: "one".to_owned(),
                        text: "station one".to_owned(),
                    },
                    StationEmbeddingDocument {
                        station_id: "two".to_owned(),
                        text: "station two".to_owned(),
                    },
                ],
                Some(_) => Vec::new(),
            })
        }

        async fn upsert_embedding(
            &self,
            station_id: &str,
            embedding: &Embedding,
        ) -> Result<(), EmbeddingStoreError> {
            self.writes
                .lock()
                .unwrap()
                .push((station_id.to_owned(), embedding.clone()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn backfill_uses_deterministic_pages_and_updates_every_document() {
        let workflow = EmbeddingBackfill::new(FakeProvider, FakeStore::default(), 2);

        let result = workflow.run().await.unwrap();

        assert_eq!(result.processed, 2);
        assert_eq!(result.updated, 2);
        assert_eq!(workflow.store.writes.lock().unwrap().len(), 2);
    }
}

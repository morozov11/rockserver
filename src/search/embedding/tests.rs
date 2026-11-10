//! Deterministic embedding and backfill tests.

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
    fn provenance(&self) -> Option<super::EmbeddingProvenance> {
        Some(super::EmbeddingProvenance {
            model: "fake".to_owned(),
            version: "1".to_owned(),
            dimension: 2,
        })
    }

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
        _provenance: &super::EmbeddingProvenance,
        after_station_id: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<StationEmbeddingDocument>, EmbeddingStoreError> {
        Ok(match after_station_id {
            None => vec![
                StationEmbeddingDocument {
                    station_id: "one".to_owned(),
                    text: "station one".to_owned(),
                    source_text_sha256: "one-hash".to_owned(),
                },
                StationEmbeddingDocument {
                    station_id: "two".to_owned(),
                    text: "station two".to_owned(),
                    source_text_sha256: "two-hash".to_owned(),
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

    async fn upsert_document_embedding(
        &self,
        document: &StationEmbeddingDocument,
        embedding: &Embedding,
    ) -> Result<bool, EmbeddingStoreError> {
        self.upsert_embedding(&document.station_id, embedding)
            .await?;
        Ok(true)
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

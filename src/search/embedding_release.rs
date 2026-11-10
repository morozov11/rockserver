//! Operator-reviewed quality evidence for an offline E5 version change.

use serde::Deserialize;

use super::{EmbeddingProvenance, EmbeddingProviderError};

/// Explicit human quality decision tied to one dataset and candidate provenance.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualityReview {
    model: String,
    version: String,
    dimension: usize,
    dataset_sha256: String,
    baseline: QualityMetrics,
    candidate: QualityMetrics,
    approved: bool,
    reviewer: String,
    rationale: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QualityMetrics {
    query_count: usize,
    recall_at_5: f64,
    recall_at_10: f64,
    mrr: f64,
    empty_responses: usize,
}

impl QualityMetrics {
    fn valid(&self) -> bool {
        self.query_count > 0
            && self.empty_responses <= self.query_count
            && [self.recall_at_5, self.recall_at_10, self.mrr]
                .into_iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
    }
}

impl EmbeddingQualityReview {
    /// Requires a reviewed comparison for the exact provider selected for startup.
    pub fn validate_json(
        bytes: &[u8],
        provider: &EmbeddingProvenance,
    ) -> Result<(), EmbeddingProviderError> {
        let review: Self = serde_json::from_slice(bytes)
            .map_err(|_| EmbeddingProviderError::safe("E5 quality review JSON is invalid"))?;
        if review.model != provider.model
            || review.version != provider.version
            || review.dimension != provider.dimension
            || review.dataset_sha256.len() != 64
            || !review
                .dataset_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !review.baseline.valid()
            || !review.candidate.valid()
            || review.baseline.query_count != review.candidate.query_count
            || !review.approved
            || review.reviewer.trim().is_empty()
            || review.rationale.trim().is_empty()
        {
            return Err(EmbeddingProviderError::safe(
                "E5 quality review does not approve this provider and dataset",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "embedding_release/tests.rs"]
mod tests;

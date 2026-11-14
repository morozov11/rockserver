//! Deterministic embedding validation tests.

use super::{Embedding, EmbeddingValidationError};

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

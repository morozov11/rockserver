//! Vector formatting and release-gate invariants.

use crate::search::Embedding;

use super::{EmbeddingReleaseReadiness, vector_literal};

#[test]
fn validated_embedding_formats_as_pgvector_literal() {
    let embedding = Embedding::new("fake", "1", 3, vec![1.0, -0.5, 0.25]).unwrap();
    assert_eq!(vector_literal(&embedding), "[1,-0.5,0.25]");
}

#[test]
fn release_gate_rejects_empty_partial_or_unindexed_catalogs() {
    for state in [
        EmbeddingReleaseReadiness {
            active_stations: 0,
            current_vectors: 0,
            ann_index_ready: true,
        },
        EmbeddingReleaseReadiness {
            active_stations: 2,
            current_vectors: 1,
            ann_index_ready: true,
        },
        EmbeddingReleaseReadiness {
            active_stations: 2,
            current_vectors: 2,
            ann_index_ready: false,
        },
    ] {
        assert!(!state.is_ready());
    }
    assert!(
        EmbeddingReleaseReadiness {
            active_stations: 2,
            current_vectors: 2,
            ann_index_ready: true,
        }
        .is_ready()
    );
}

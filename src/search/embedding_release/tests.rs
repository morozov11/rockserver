//! Quality-report validation without model or network dependencies.

use serde_json::json;

use super::EmbeddingQualityReview;
use crate::search::EmbeddingProvenance;

fn provider() -> EmbeddingProvenance {
    EmbeddingProvenance {
        model: "intfloat/multilingual-e5-small".to_owned(),
        version: "onnx-v2".to_owned(),
        dimension: 384,
    }
}

fn report() -> serde_json::Value {
    json!({
        "model": "intfloat/multilingual-e5-small",
        "version": "onnx-v2",
        "dimension": 384,
        "dataset_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "baseline": {"query_count": 50, "recall_at_5": 0.8, "recall_at_10": 0.9, "mrr": 0.7, "empty_responses": 1},
        "candidate": {"query_count": 50, "recall_at_5": 0.9, "recall_at_10": 0.9, "mrr": 0.8, "empty_responses": 0},
        "approved": true,
        "reviewer": "operator",
        "rationale": "Reviewed the same labelled queries and accepted the differences"
    })
}

#[test]
fn reviewed_metrics_must_match_the_actual_provider() {
    let valid = report();
    assert!(
        EmbeddingQualityReview::validate_json(&valid.to_string().into_bytes(), &provider()).is_ok()
    );
    for (field, value) in [
        ("version", json!("onnx-v1")),
        ("approved", json!(false)),
        ("dataset_sha256", json!("bad")),
        ("reviewer", json!(" ")),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(
            EmbeddingQualityReview::validate_json(&invalid.to_string().into_bytes(), &provider())
                .is_err()
        );
    }
}

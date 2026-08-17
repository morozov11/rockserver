//! Opt-in local-only smoke test for a provisioned multilingual E5 ONNX model.

#![cfg(feature = "onnx-local")]

use std::time::Instant;

use rockserver::{
    providers::onnx_e5::{DIMENSION, OnnxE5Config, OnnxE5EmbeddingProvider},
    search::EmbeddingProvider,
};
use uuid::Uuid;

/// Runs one local CPU inference without logging model paths, input text, or vectors.
#[tokio::test]
#[ignore = "requires local ONNX E5 assets and ORT_DYLIB_PATH; no network is used"]
async fn embeds_local_multilingual_query_with_safe_logs() {
    let test_id = Uuid::new_v4();
    let provider =
        OnnxE5EmbeddingProvider::load(&OnnxE5Config::from_env().expect("local E5 configuration"))
            .expect("local E5 model must load");
    let started = Instant::now();
    let embedding = provider
        .embed("спокойный джаз")
        .await
        .expect("local E5 must embed");
    tracing::info!(%test_id, elapsed_ms = started.elapsed().as_millis(), dimension = embedding.provenance().dimension, "local E5 live test completed");
    assert_eq!(embedding.provenance().dimension, DIMENSION);
    assert!(embedding.values().iter().all(|value| value.is_finite()));
}

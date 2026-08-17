//! External catalog providers isolated from HTTP search and persistence.

use std::{env, sync::Arc};

use crate::search::{EmbeddingProvider, EmbeddingProviderError};

/// Explicit development-only deterministic embedding provider.
pub mod deterministic_embedding;
/// CPU-only local ONNX implementation of multilingual E5-small.
#[cfg(feature = "onnx-local")]
pub mod onnx_e5;
/// Radio Browser HTTP client and deterministic DTO normalization.
pub mod radio_browser;
/// Yandex AI Studio adapter for structured radio-intent generation.
pub mod yandex_llm;
/// Local-configuration Yandex SpeechKit adapter for voice streaming.
pub mod yandex_speechkit;

/// Selects an explicitly configured embedding provider without exposing model
/// files, stream data, or environment values in errors or logs.
pub fn embedding_provider_from_env()
-> Result<Option<Arc<dyn EmbeddingProvider>>, EmbeddingProviderError> {
    match env::var(deterministic_embedding::SEMANTIC_PROVIDER_ENV) {
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(EmbeddingProviderError::safe(
            "ROCKSERVER_SEMANTIC_PROVIDER must be valid Unicode",
        )),
        Ok(value) if value == deterministic_embedding::DETERMINISTIC_DEV_PROVIDER => {
            Ok(Some(Arc::new(
                deterministic_embedding::DeterministicEmbeddingProvider::from_env()
                    .map_err(|error| EmbeddingProviderError::safe(error.to_string()))?,
            )))
        }
        #[cfg(feature = "onnx-local")]
        Ok(value) if value == onnx_e5::PROVIDER => Ok(Some(Arc::new(
            onnx_e5::OnnxE5EmbeddingProvider::load(&onnx_e5::OnnxE5Config::from_env()?)?,
        ))),
        #[cfg(not(feature = "onnx-local"))]
        Ok(value) if value == "onnx-e5-local" => Err(EmbeddingProviderError::safe(
            "onnx-e5-local requires building RockServer with --features onnx-local",
        )),
        Ok(_) => Err(EmbeddingProviderError::safe(
            "ROCKSERVER_SEMANTIC_PROVIDER has an unsupported value",
        )),
    }
}

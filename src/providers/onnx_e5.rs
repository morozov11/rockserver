//! CPU-only local ONNX inference for `intfloat/multilingual-e5-small`.
//!
//! The deployment supplies an exported `model.onnx`, its matching
//! `tokenizer.json`, and a local ONNX Runtime shared library.  Neither model
//! assets nor credentials are stored in the repository.

use std::{
    env,
    path::{Path, PathBuf},
    sync::Mutex,
};

use async_trait::async_trait;
use ndarray::Array2;
use ort::{session::Session, value::TensorRef};
use tokenizers::Tokenizer;

use crate::search::{Embedding, EmbeddingProvider, EmbeddingProviderError};

/// Stable provenance for the selected compact multilingual embedding model.
pub const MODEL: &str = "intfloat/multilingual-e5-small";
/// Preprocessing/inference contract version for the ONNX export.
pub const VERSION: &str = "onnx-v1";
/// Output width of multilingual-e5-small.
pub const DIMENSION: usize = 384;
const MAX_TOKENS: usize = 512;
/// Selector value for the local production embedder.
pub const PROVIDER: &str = "onnx-e5-local";
/// Local ONNX graph path, never downloaded by the service.
pub const MODEL_PATH_ENV: &str = "ROCKSERVER_ONNX_MODEL_PATH";
/// Local matching tokenizer JSON path.
pub const TOKENIZER_PATH_ENV: &str = "ROCKSERVER_ONNX_TOKENIZER_PATH";
/// Optional ONNX Runtime intra-op CPU-thread limit.
pub const INTRA_THREADS_ENV: &str = "ROCKSERVER_ONNX_INTRA_THREADS";

/// Paths required to run the model entirely on the local CPU.
#[derive(Clone, Debug)]
pub struct OnnxE5Config {
    /// ONNX graph exported from the selected model revision.
    pub model_path: PathBuf,
    /// Hugging Face `tokenizer.json` from the exact same model revision.
    pub tokenizer_path: PathBuf,
    /// Maximum ONNX Runtime intra-op CPU threads.
    pub intra_threads: usize,
}

impl OnnxE5Config {
    /// Loads local-only configuration; model files and runtime are never fetched at startup.
    pub fn from_env() -> Result<Self, EmbeddingProviderError> {
        let model_path = required_env_path(MODEL_PATH_ENV)?;
        let tokenizer_path = required_env_path(TOKENIZER_PATH_ENV)?;
        let intra_threads = match env::var(INTRA_THREADS_ENV) {
            Ok(value) => value.parse().map_err(|_| {
                EmbeddingProviderError::safe(
                    "local E5 CPU thread setting must be a positive integer",
                )
            })?,
            Err(env::VarError::NotPresent) => 2,
            Err(env::VarError::NotUnicode(_)) => {
                return Err(EmbeddingProviderError::safe(
                    "local E5 CPU thread setting must be Unicode",
                ));
            }
        };
        Self::new(model_path, tokenizer_path, intra_threads)
    }
    /// Validates local assets before the HTTP service accepts semantic search.
    pub fn new(
        model_path: PathBuf,
        tokenizer_path: PathBuf,
        intra_threads: usize,
    ) -> Result<Self, EmbeddingProviderError> {
        for (label, path) in [("model", &model_path), ("tokenizer", &tokenizer_path)] {
            if !path.is_file() {
                return Err(EmbeddingProviderError::safe(format!(
                    "local E5 {label} file is missing"
                )));
            }
        }
        if intra_threads == 0 {
            return Err(EmbeddingProviderError::safe(
                "local E5 intra-op threads must be positive",
            ));
        }
        Ok(Self {
            model_path,
            tokenizer_path,
            intra_threads,
        })
    }
}

fn required_env_path(variable: &str) -> Result<PathBuf, EmbeddingProviderError> {
    match env::var(variable) {
        Ok(value) if !value.trim().is_empty() => Ok(PathBuf::from(value)),
        Ok(_) | Err(env::VarError::NotPresent) => Err(EmbeddingProviderError::safe(format!(
            "{variable} is required for local E5"
        ))),
        Err(env::VarError::NotUnicode(_)) => Err(EmbeddingProviderError::safe(format!(
            "{variable} must be Unicode"
        ))),
    }
}

/// Thread-safe local E5 encoder.  ONNX sessions are serialized because `ort`
/// requires a mutable session for `run`; ONNX Runtime still uses its CPU pool.
pub struct OnnxE5EmbeddingProvider {
    tokenizer: Tokenizer,
    session: Mutex<Session>,
}

impl OnnxE5EmbeddingProvider {
    /// Loads only local assets; `ORT_DYLIB_PATH` must name a local ONNX Runtime library.
    pub fn load(config: &OnnxE5Config) -> Result<Self, EmbeddingProviderError> {
        let tokenizer = Tokenizer::from_file(&config.tokenizer_path)
            .map_err(|_| EmbeddingProviderError::safe("local E5 tokenizer could not be loaded"))?;
        let builder = Session::builder().map_err(|_| {
            EmbeddingProviderError::safe("local E5 ONNX builder could not be created")
        })?;
        let mut builder = builder
            .with_intra_threads(config.intra_threads)
            .map_err(|_| EmbeddingProviderError::safe("local E5 CPU thread setup failed"))?;
        let session = builder.commit_from_file(&config.model_path).map_err(|_| {
            EmbeddingProviderError::safe("local E5 ONNX session could not be loaded")
        })?;
        Ok(Self {
            tokenizer,
            session: Mutex::new(session),
        })
    }

    /// Prefixes input as required by E5's contrastive retrieval training.
    fn prefixed_input(prefix: &str, text: &str) -> Result<String, EmbeddingProviderError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(EmbeddingProviderError::safe(
                "embedding input must not be empty",
            ));
        }
        Ok(format!("{prefix}: {text}"))
    }

    fn infer(&self, prefix: &str, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        let encoded = self
            .tokenizer
            .encode(Self::prefixed_input(prefix, text)?, true)
            .map_err(|_| EmbeddingProviderError::safe("local E5 tokenization failed"))?;
        let length = encoded.len().min(MAX_TOKENS);
        let ids = Array2::from_shape_vec(
            (1, length),
            encoded.get_ids()[..length]
                .iter()
                .map(|&id| i64::from(id))
                .collect(),
        )
        .map_err(|_| EmbeddingProviderError::safe("local E5 token tensor construction failed"))?;
        let mask_values = encoded.get_attention_mask()[..length].to_vec();
        let mask = Array2::from_shape_vec(
            (1, length),
            mask_values.iter().map(|&value| i64::from(value)).collect(),
        )
        .map_err(|_| EmbeddingProviderError::safe("local E5 mask tensor construction failed"))?;
        let type_ids = Array2::<i64>::zeros((1, length));
        let mut session = self
            .session
            .lock()
            .map_err(|_| EmbeddingProviderError::safe("local E5 session lock was poisoned"))?;
        let outputs = session.run(ort::inputs![
            "input_ids" => TensorRef::from_array_view(&ids).map_err(|_| EmbeddingProviderError::safe("local E5 input tensor failed"))?,
            "attention_mask" => TensorRef::from_array_view(&mask).map_err(|_| EmbeddingProviderError::safe("local E5 mask tensor failed"))?,
            "token_type_ids" => TensorRef::from_array_view(&type_ids).map_err(|_| EmbeddingProviderError::safe("local E5 type tensor failed"))?,
        ]).map_err(|_| EmbeddingProviderError::safe("local E5 inference failed"))?;
        let hidden = outputs
            .get("last_hidden_state")
            .unwrap_or(&outputs[0])
            .try_extract_array::<f32>()
            .map_err(|_| EmbeddingProviderError::safe("local E5 output tensor failed"))?
            .into_dimensionality::<ndarray::Ix3>()
            .map_err(|_| EmbeddingProviderError::safe("local E5 output rank was invalid"))?;
        let values = Self::mean_pool(hidden, &mask_values)?;
        Embedding::new(MODEL, VERSION, DIMENSION, values)
            .map_err(|error| EmbeddingProviderError::safe(error.to_string()))
    }

    fn mean_pool(
        hidden: ndarray::ArrayView3<'_, f32>,
        mask: &[u32],
    ) -> Result<Vec<f32>, EmbeddingProviderError> {
        if hidden.shape()[0] != 1
            || hidden.shape()[1] != mask.len()
            || hidden.shape()[2] != DIMENSION
        {
            return Err(EmbeddingProviderError::safe(
                "local E5 returned an unexpected tensor shape",
            ));
        }
        let active = mask.iter().filter(|&&value| value != 0).count();
        if active == 0 {
            return Err(EmbeddingProviderError::safe(
                "local E5 tokenizer returned an empty mask",
            ));
        }
        let mut values = vec![0.0; DIMENSION];
        for (token, &enabled) in mask.iter().enumerate() {
            if enabled != 0 {
                for (dimension, value) in values.iter_mut().enumerate() {
                    *value += hidden[[0, token, dimension]];
                }
            }
        }
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !norm.is_finite() || norm == 0.0 {
            return Err(EmbeddingProviderError::safe(
                "local E5 returned an invalid pooled vector",
            ));
        }
        for value in &mut values {
            *value /= norm;
        }
        Ok(values)
    }
}

#[async_trait]
impl EmbeddingProvider for OnnxE5EmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.infer("query", text)
    }

    async fn embed_document(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.infer("passage", text)
    }
}

#[allow(dead_code)]
fn _path_is_file(path: &Path) -> bool {
    path.is_file()
}

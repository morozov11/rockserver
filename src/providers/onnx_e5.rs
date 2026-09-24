//! CPU-only local ONNX inference for `intfloat/multilingual-e5-small`.
//!
//! Provides non-blocking, multi-session pooled embedding generation.
//! Model assets and runtime libraries are configured through local paths and never
//! downloaded over the network.

use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use ndarray::Array2;
use ort::{session::Session, value::TensorRef};
use tokenizers::Tokenizer;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
/// Optional ONNX session pool size (number of concurrent model inference sessions).
pub const SESSION_POOL_SIZE_ENV: &str = "ROCKSERVER_ONNX_SESSION_POOL_SIZE";

/// Default pool size if `ROCKSERVER_ONNX_SESSION_POOL_SIZE` is unset:
/// bounded to `[1, 4]` based on available system CPU parallelism.
pub fn default_session_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|p| p.get().clamp(1, 4))
        .unwrap_or(2)
}

/// Paths and execution limits required to run the model entirely on the local CPU.
#[derive(Clone, Debug)]
pub struct OnnxE5Config {
    /// ONNX graph exported from the selected model revision.
    pub model_path: PathBuf,
    /// Hugging Face `tokenizer.json` from the exact same model revision.
    pub tokenizer_path: PathBuf,
    /// Maximum ONNX Runtime intra-op CPU threads per session.
    pub intra_threads: usize,
    /// Number of concurrent ONNX sessions allocated in the inference pool.
    pub session_pool_size: usize,
}

impl OnnxE5Config {
    /// Loads local-only configuration; model files and runtime are never fetched at startup.
    pub fn from_env() -> Result<Self, EmbeddingProviderError> {
        Self::from_lookup(|name| match env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(env::VarError::NotPresent) => Ok(None),
            Err(env::VarError::NotUnicode(_)) => {
                let label = match name {
                    INTRA_THREADS_ENV => "local E5 CPU thread setting must be Unicode",
                    SESSION_POOL_SIZE_ENV => "local E5 session pool size setting must be Unicode",
                    _ => {
                        return Err(EmbeddingProviderError::safe(format!(
                            "{name} must be Unicode"
                        )));
                    }
                };
                Err(EmbeddingProviderError::safe(label))
            }
        })
    }

    /// Loads configuration using a variable lookup closure, allowing isolated deterministic tests.
    pub fn from_lookup(
        mut lookup: impl FnMut(&str) -> Result<Option<String>, EmbeddingProviderError>,
    ) -> Result<Self, EmbeddingProviderError> {
        let model_path = required_lookup_path(MODEL_PATH_ENV, &mut lookup)?;
        let tokenizer_path = required_lookup_path(TOKENIZER_PATH_ENV, &mut lookup)?;
        let intra_threads = match lookup(INTRA_THREADS_ENV)? {
            Some(value) => value.parse().map_err(|_| {
                EmbeddingProviderError::safe(
                    "local E5 CPU thread setting must be a positive integer",
                )
            })?,
            None => 2,
        };
        let session_pool_size = match lookup(SESSION_POOL_SIZE_ENV)? {
            Some(value) => value.parse().map_err(|_| {
                EmbeddingProviderError::safe(
                    "local E5 session pool size setting must be a positive integer",
                )
            })?,
            None => default_session_pool_size(),
        };
        Self::new(model_path, tokenizer_path, intra_threads, session_pool_size)
    }

    /// Validates local assets and runtime execution limits.
    pub fn new(
        model_path: PathBuf,
        tokenizer_path: PathBuf,
        intra_threads: usize,
        session_pool_size: usize,
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
        if session_pool_size == 0 {
            return Err(EmbeddingProviderError::safe(
                "local E5 session pool size must be positive",
            ));
        }
        Ok(Self {
            model_path,
            tokenizer_path,
            intra_threads,
            session_pool_size,
        })
    }

    /// Helper to construct a configuration using the default session pool size.
    pub fn with_default_pool_size(
        model_path: PathBuf,
        tokenizer_path: PathBuf,
        intra_threads: usize,
    ) -> Result<Self, EmbeddingProviderError> {
        Self::new(
            model_path,
            tokenizer_path,
            intra_threads,
            default_session_pool_size(),
        )
    }
}

fn required_lookup_path(
    variable: &str,
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, EmbeddingProviderError>,
) -> Result<PathBuf, EmbeddingProviderError> {
    match lookup(variable)? {
        Some(value) if !value.trim().is_empty() => Ok(PathBuf::from(value)),
        Some(_) | None => Err(EmbeddingProviderError::safe(format!(
            "{variable} is required for local E5"
        ))),
    }
}

/// Bounded resource pool managing checkouts of reusable sessions with semaphore backpressure.
struct SessionPool<T> {
    sessions: Arc<Mutex<Vec<T>>>,
    semaphore: Arc<Semaphore>,
    size: usize,
}

impl<T> SessionPool<T> {
    /// Creates a new pool populated with the provided sessions.
    fn new(sessions: Vec<T>) -> Self {
        let size = sessions.len();
        Self {
            sessions: Arc::new(Mutex::new(sessions)),
            semaphore: Arc::new(Semaphore::new(size)),
            size,
        }
    }

    /// Asynchronously checks out a session from the pool.
    ///
    /// Waits until a session permit is available, pops a session from the pool,
    /// and wraps it in a [`PooledItem`] RAII guard.
    async fn acquire(&self) -> Result<PooledItem<T>, EmbeddingProviderError> {
        let permit =
            self.semaphore.clone().acquire_owned().await.map_err(|_| {
                EmbeddingProviderError::safe("local E5 session pool semaphore closed")
            })?;
        let item = {
            let mut pool = match self.sessions.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            pool.pop()
                .expect("semaphore guarantees an available session in the pool")
        };
        Ok(PooledItem {
            item: Some(item),
            pool: Arc::clone(&self.sessions),
            _permit: permit,
        })
    }

    /// Total capacity of this session pool.
    fn size(&self) -> usize {
        self.size
    }

    /// Number of sessions currently idle and available for checkout.
    fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// RAII guard holding a checked-out session from [`SessionPool`].
///
/// When dropped, the session is guaranteed to be returned to the pool before the
/// semaphore permit is released, preventing wakeups against an empty pool.
struct PooledItem<T> {
    item: Option<T>,
    pool: Arc<Mutex<Vec<T>>>,
    _permit: OwnedSemaphorePermit,
}

impl<T> PooledItem<T> {
    /// Returns a mutable reference to the acquired session.
    fn get_mut(&mut self) -> &mut T {
        self.item
            .as_mut()
            .expect("session item is guaranteed to be present until dropped")
    }
}

impl<T> Drop for PooledItem<T> {
    fn drop(&mut self) {
        if let Some(item) = self.item.take() {
            let mut pool = match self.pool.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            pool.push(item);
        }
        // Note: `self._permit` is dropped immediately after `drop` completes,
        // which restores the semaphore permit only after the item is safely back in the pool.
    }
}

/// Thread-safe local E5 encoder backed by a pool of ONNX sessions for non-blocking concurrent CPU inference.
pub struct OnnxE5EmbeddingProvider {
    tokenizer: Arc<Tokenizer>,
    pool: SessionPool<Session>,
}

impl OnnxE5EmbeddingProvider {
    /// Loads only local assets; `ORT_DYLIB_PATH` must name a local ONNX Runtime library.
    ///
    /// Pre-allocates `config.session_pool_size` ONNX Runtime sessions to allow concurrent
    /// inferences without blocking Tokio worker threads.
    pub fn load(config: &OnnxE5Config) -> Result<Self, EmbeddingProviderError> {
        let tokenizer = Tokenizer::from_file(&config.tokenizer_path)
            .map_err(|_| EmbeddingProviderError::safe("local E5 tokenizer could not be loaded"))?;
        let mut sessions = Vec::with_capacity(config.session_pool_size);
        for _ in 0..config.session_pool_size {
            let builder = Session::builder().map_err(|_| {
                EmbeddingProviderError::safe("local E5 ONNX builder could not be created")
            })?;
            let mut builder = builder
                .with_intra_threads(config.intra_threads)
                .map_err(|_| EmbeddingProviderError::safe("local E5 CPU thread setup failed"))?;
            let session = builder.commit_from_file(&config.model_path).map_err(|_| {
                EmbeddingProviderError::safe("local E5 ONNX session could not be loaded")
            })?;
            sessions.push(session);
        }
        Ok(Self {
            tokenizer: Arc::new(tokenizer),
            pool: SessionPool::new(sessions),
        })
    }

    /// Returns the total number of ONNX sessions configured in the pool.
    pub fn session_pool_size(&self) -> usize {
        self.pool.size()
    }

    /// Returns the number of ONNX sessions currently idle and available for checkout.
    pub fn available_sessions(&self) -> usize {
        self.pool.available()
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

    /// Asynchronously runs inference by checking out a pooled session and offloading
    /// CPU-bound tokenization, tensor construction, inference, and pooling to `spawn_blocking`.
    async fn infer(
        &self,
        prefix: &'static str,
        text: &str,
    ) -> Result<Embedding, EmbeddingProviderError> {
        let input = Self::prefixed_input(prefix, text)?;
        let mut pooled = self.pool.acquire().await?;
        let tokenizer = Arc::clone(&self.tokenizer);

        let join_result = tokio::task::spawn_blocking(move || {
            Self::infer_blocking(&tokenizer, pooled.get_mut(), &input)
        })
        .await;

        match join_result {
            Ok(result) => result,
            Err(join_err) => {
                if join_err.is_panic() {
                    Err(EmbeddingProviderError::safe(
                        "local E5 inference task panicked",
                    ))
                } else {
                    Err(EmbeddingProviderError::safe(
                        "local E5 inference task was cancelled",
                    ))
                }
            }
        }
    }

    /// Executes CPU-bound tokenization, tensor construction, ONNX graph execution, and mean pooling.
    fn infer_blocking(
        tokenizer: &Tokenizer,
        session: &mut Session,
        input: &str,
    ) -> Result<Embedding, EmbeddingProviderError> {
        let encoded = tokenizer
            .encode(input, true)
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
        let outputs = session
            .run(ort::inputs![
                "input_ids" => TensorRef::from_array_view(&ids).map_err(|_| EmbeddingProviderError::safe("local E5 input tensor failed"))?,
                "attention_mask" => TensorRef::from_array_view(&mask).map_err(|_| EmbeddingProviderError::safe("local E5 mask tensor failed"))?,
                "token_type_ids" => TensorRef::from_array_view(&type_ids).map_err(|_| EmbeddingProviderError::safe("local E5 type tensor failed"))?,
            ])
            .map_err(|_| EmbeddingProviderError::safe("local E5 inference failed"))?;
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

    /// Performs mean-pooling over active tokens and applies L2 normalization.
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
        self.infer("query", text).await
    }

    async fn embed_document(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.infer("passage", text).await
    }

    fn supports_semantic_intent_filters(&self) -> bool {
        true
    }
}

#[allow(dead_code)]
fn _path_is_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use ndarray::Array3;

    use super::*;

    #[test]
    fn default_session_pool_size_is_bounded() {
        let size = default_session_pool_size();
        assert!(size >= 1);
        assert!(size <= 4);
    }

    #[test]
    fn config_from_lookup_reads_defaults_and_overrides() {
        let temp_dir = std::env::temp_dir();
        let model_file = temp_dir.join("test_model.onnx");
        let tokenizer_file = temp_dir.join("test_tokenizer.json");
        File::create(&model_file).unwrap();
        File::create(&tokenizer_file).unwrap();

        // 1. Unset pool size uses default
        let config = OnnxE5Config::from_lookup(|name| match name {
            MODEL_PATH_ENV => Ok(Some(model_file.to_str().unwrap().to_string())),
            TOKENIZER_PATH_ENV => Ok(Some(tokenizer_file.to_str().unwrap().to_string())),
            _ => Ok(None),
        })
        .unwrap();
        assert_eq!(config.intra_threads, 2);
        assert_eq!(config.session_pool_size, default_session_pool_size());

        // 2. Explicit pool size override
        let config = OnnxE5Config::from_lookup(|name| match name {
            MODEL_PATH_ENV => Ok(Some(model_file.to_str().unwrap().to_string())),
            TOKENIZER_PATH_ENV => Ok(Some(tokenizer_file.to_str().unwrap().to_string())),
            INTRA_THREADS_ENV => Ok(Some("4".to_string())),
            SESSION_POOL_SIZE_ENV => Ok(Some("3".to_string())),
            _ => Ok(None),
        })
        .unwrap();
        assert_eq!(config.intra_threads, 4);
        assert_eq!(config.session_pool_size, 3);

        // Cleanup
        let _ = std::fs::remove_file(model_file);
        let _ = std::fs::remove_file(tokenizer_file);
    }

    #[test]
    fn config_validation_rejects_invalid_values() {
        let temp_dir = std::env::temp_dir();
        let model_file = temp_dir.join("test_val_model.onnx");
        let tokenizer_file = temp_dir.join("test_val_tokenizer.json");
        File::create(&model_file).unwrap();
        File::create(&tokenizer_file).unwrap();

        // Zero threads
        let err = OnnxE5Config::new(model_file.clone(), tokenizer_file.clone(), 0, 2).unwrap_err();
        assert!(
            err.to_string()
                .contains("intra-op threads must be positive")
        );

        // Zero pool size
        let err = OnnxE5Config::new(model_file.clone(), tokenizer_file.clone(), 2, 0).unwrap_err();
        assert!(
            err.to_string()
                .contains("session pool size must be positive")
        );

        // Missing file
        let err = OnnxE5Config::new(
            temp_dir.join("non_existent_model.onnx"),
            tokenizer_file.clone(),
            2,
            2,
        )
        .unwrap_err();
        assert!(err.to_string().contains("model file is missing"));

        // Non-integer pool size via lookup
        let err = OnnxE5Config::from_lookup(|name| match name {
            MODEL_PATH_ENV => Ok(Some(model_file.to_str().unwrap().to_string())),
            TOKENIZER_PATH_ENV => Ok(Some(tokenizer_file.to_str().unwrap().to_string())),
            SESSION_POOL_SIZE_ENV => Ok(Some("not-a-number".to_string())),
            _ => Ok(None),
        })
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("session pool size setting must be a positive integer")
        );

        let _ = std::fs::remove_file(model_file);
        let _ = std::fs::remove_file(tokenizer_file);
    }

    #[tokio::test]
    async fn session_pool_bounds_concurrency_and_recycles_sessions() {
        const POOL_CAPACITY: usize = 3;
        const NUM_TASKS: usize = 20;

        let sessions: Vec<usize> = (0..POOL_CAPACITY).collect();
        let pool = Arc::new(SessionPool::new(sessions));

        assert_eq!(pool.size(), POOL_CAPACITY);
        assert_eq!(pool.available(), POOL_CAPACITY);

        let active_count = Arc::new(AtomicUsize::new(0));
        let max_observed_concurrency = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..NUM_TASKS {
            let pool_clone = Arc::clone(&pool);
            let active = Arc::clone(&active_count);
            let max_observed = Arc::clone(&max_observed_concurrency);

            handles.push(tokio::spawn(async move {
                let mut guard = pool_clone.acquire().await.unwrap();
                let _session_id = *guard.get_mut();

                // Track active concurrency while the session guard is held
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_observed.fetch_max(current, Ordering::SeqCst);

                let active_for_thread = Arc::clone(&active);
                // Run CPU-like work on blocking thread pool
                tokio::task::spawn_blocking(move || {
                    std::thread::sleep(Duration::from_millis(10));
                    active_for_thread.fetch_sub(1, Ordering::SeqCst);
                    // Guard drops inside spawn_blocking thread after releasing active count
                    drop(guard);
                })
                .await
                .unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Concurrency should never have exceeded pool capacity
        assert!(max_observed_concurrency.load(Ordering::SeqCst) <= POOL_CAPACITY);
        assert_eq!(active_count.load(Ordering::SeqCst), 0);
        // All sessions must be returned to pool
        assert_eq!(pool.available(), POOL_CAPACITY);
    }

    #[tokio::test]
    async fn session_pool_returns_item_even_on_panic() {
        let pool = Arc::new(SessionPool::new(vec![42_usize]));
        assert_eq!(pool.available(), 1);

        let pool_clone = Arc::clone(&pool);
        let join_result = tokio::spawn(async move {
            let mut guard = pool_clone.acquire().await.unwrap();
            let _ = *guard.get_mut();

            tokio::task::spawn_blocking(move || {
                // Keep guard in scope so drop runs on panic unwind
                let _held = guard;
                panic!("simulated worker thread panic");
            })
            .await
        })
        .await
        .unwrap();

        // The blocking task should have panicked
        assert!(join_result.is_err());
        assert!(join_result.unwrap_err().is_panic());

        // Invariant: the session MUST be returned to the pool even after panic!
        assert_eq!(pool.available(), 1);

        // Verification: checking out the session again succeeds
        let guard2 = pool.acquire().await.unwrap();
        assert_eq!(*guard2.item.as_ref().unwrap(), 42);
    }

    #[test]
    fn prefixed_input_handles_queries_and_empty_strings() {
        assert_eq!(
            OnnxE5EmbeddingProvider::prefixed_input("query", "  calm jazz  ").unwrap(),
            "query: calm jazz"
        );
        assert_eq!(
            OnnxE5EmbeddingProvider::prefixed_input("passage", "Rock Radio").unwrap(),
            "passage: Rock Radio"
        );
        let err = OnnxE5EmbeddingProvider::prefixed_input("query", "   ").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn mean_pool_validates_dimensions_and_normalizes() {
        // Dimension mismatch
        let array = Array3::<f32>::zeros((1, 2, 100)); // DIMENSION is 384
        let mask = vec![1, 1];
        let err = OnnxE5EmbeddingProvider::mean_pool(array.view(), &mask).unwrap_err();
        assert!(err.to_string().contains("unexpected tensor shape"));

        // All-zero mask
        let array = Array3::<f32>::zeros((1, 2, DIMENSION));
        let mask = vec![0, 0];
        let err = OnnxE5EmbeddingProvider::mean_pool(array.view(), &mask).unwrap_err();
        assert!(err.to_string().contains("empty mask"));

        // Valid pooling & L2 normalization
        let mut array = Array3::<f32>::zeros((1, 2, DIMENSION));
        array[[0, 0, 0]] = 3.0;
        array[[0, 1, 0]] = 1.0;
        let mask = vec![1, 0]; // only first token active
        let pooled = OnnxE5EmbeddingProvider::mean_pool(array.view(), &mask).unwrap();
        assert_eq!(pooled.len(), DIMENSION);
        // Vector has 3.0 at dim 0, rest 0.0 -> normalized is 1.0 at dim 0
        assert!((pooled[0] - 1.0).abs() < 1e-6);
        let norm: f32 = pooled.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }
}

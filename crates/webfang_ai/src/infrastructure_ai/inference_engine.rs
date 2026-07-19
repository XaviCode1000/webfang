//! Inference engine — ONNX model execution with ort (ONNX Runtime)
//!
//! Handles loading and executing ONNX models for sentence embedding generation:
//! - Thread-safe model bytes sharing with `Arc<Vec<u8>>` (`own-arc-shared`)
//! - Async inference via `spawn_blocking` (`async-spawn-blocking`)
//! - Clone Arc before await (`async-clone-before-await`)
//! - 384-dimensional embedding output for IBM Granite models
//! - **3-input ONNX model**: input_ids, attention_mask, token_type_ids
//!
//! # Design Decisions
//!
//! - **ort::Session is !Send**: Each `run_inference` creates a local `ort::Session` inside
//!   `spawn_blocking` and destroys it before returning. Model bytes are stored as
//!   `Arc<Vec<u8>>` for cheap cross-thread sharing without the `!Send` constraint.
//! - **384-dim invariant**: Granite-97M is natively 384d; Granite-311M uses Matryoshka
//!   truncation to 384d. No runtime dimension discovery needed.
//! - **spawn_blocking**: CPU-intensive ONNX inference runs in blocking pool to avoid
//!   starving async runtime.
//! - **No locks across await**: Clone Arc before async operations.

use std::path::Path;
use std::sync::Arc;

use ort::session::{builder::GraphOptimizationLevel, Session};
use tracing::{debug, instrument};

use crate::infrastructure_ai::cache_config::AiModel;
use webfang_core::error::SemanticError;

/// Input data for ONNX model inference
///
/// The Granite embedding models require 3 input tensors:
/// 1. `input_ids` - Token IDs (vocab indices)
/// 2. `attention_mask` - Which tokens are real (1) vs padding (0)
/// 3. `token_type_ids` - Segment IDs (0 for single sentence)
///
/// All vectors must have the same length (sequence length).
#[derive(Debug, Clone)]
pub struct ModelInput {
    /// Token IDs (vocab indices)
    pub input_ids: Vec<i64>,
    /// Attention mask (1 for real tokens, 0 for padding)
    pub attention_mask: Vec<i64>,
    /// Token type IDs (segment IDs, usually all 0s)
    pub token_type_ids: Vec<i64>,
}

impl ModelInput {
    /// Create a new model input
    ///
    /// # Arguments
    ///
    /// * `input_ids` - Token IDs including special tokens
    /// * `attention_mask` - 1 for real tokens, 0 for padding
    /// * `token_type_ids` - Segment IDs (0 for single sentence)
    ///
    /// # Panics
    ///
    /// Panics if the three vectors have different lengths.
    #[must_use]
    pub fn new(input_ids: Vec<i64>, attention_mask: Vec<i64>, token_type_ids: Vec<i64>) -> Self {
        assert_eq!(
            input_ids.len(),
            attention_mask.len(),
            "input_ids and attention_mask must have same length"
        );
        assert_eq!(
            input_ids.len(),
            token_type_ids.len(),
            "input_ids and token_type_ids must have same length"
        );

        Self {
            input_ids,
            attention_mask,
            token_type_ids,
        }
    }

    /// Get sequence length
    #[must_use]
    pub fn seq_len(&self) -> usize {
        self.input_ids.len()
    }

    /// Create from token IDs only (generates default mask and type IDs)
    ///
    /// This is a convenience method for single-sentence inputs where:
    /// - attention_mask is all 1s (no padding)
    /// - token_type_ids is all 0s (single segment)
    #[must_use]
    pub fn from_tokens(input_ids: Vec<i64>) -> Self {
        let seq_len = input_ids.len();
        Self {
            input_ids: input_ids.clone(),
            attention_mask: vec![1i64; seq_len],
            token_type_ids: vec![0i64; seq_len],
        }
    }
}

/// ONNX inference engine for sentence embeddings
///
/// Uses ort (ONNX Runtime) as the inference backend. The engine holds model
/// bytes in `Arc<Vec<u8>>` for cheap cloning and thread-safe sharing. Each
/// inference call creates a local `ort::Session` inside `spawn_blocking`
/// because `Session` is `!Send`.
///
/// # Thread Safety
///
/// `InferenceEngine` is `Clone` (cheap `Arc` clone). It is `Send + Sync`
/// because `Arc<Vec<u8>>` is `Send + Sync`.
#[derive(Debug, Clone)]
pub struct InferenceEngine {
    model_bytes: Arc<Vec<u8>>,
    model_variant: AiModel,
}

impl InferenceEngine {
    /// Load ONNX model from file
    ///
    /// Reads the model bytes from disk and stores them in an `Arc` for
    /// cheap sharing. The actual `ort::Session` is created per-inference
    /// inside `spawn_blocking`.
    ///
    /// # Arguments
    ///
    /// * `model_path` - Path to the ONNX model file
    /// * `model_variant` - Which AI model is being loaded (for dimension handling)
    ///
    /// # Returns
    ///
    /// * `Ok(InferenceEngine)` - Model bytes loaded successfully
    /// * `Err(SemanticError::ModelLoad)` - Failed to read model file
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - File doesn't exist or can't be read
    /// - File is empty
    pub async fn load_from_file<P: AsRef<Path>>(
        model_path: P,
        model_variant: AiModel,
    ) -> Result<Self, SemanticError> {
        let model_path = model_path.as_ref();

        debug!(path = ?model_path, "Loading ONNX model bytes");

        let model_data = tokio::fs::read(model_path).await.map_err(|e| {
            SemanticError::ModelLoad(std::io::Error::other(format!(
                "Failed to read model file '{}': {}",
                model_path.display(),
                e
            )))
        })?;

        if model_data.is_empty() {
            return Err(SemanticError::ModelLoad(std::io::Error::other(format!(
                "Model file is empty: '{}'",
                model_path.display()
            ))));
        }

        let model_bytes = Arc::new(model_data);

        debug!(
            bytes = model_bytes.len(),
            ?model_variant,
            "Model bytes loaded successfully"
        );

        Ok(Self {
            model_bytes,
            model_variant,
        })
    }

    /// Run inference on token inputs
    ///
    /// Creates an ephemeral `ort::Session` from the stored model bytes,
    /// runs inference, and applies mean pooling + L2 normalization.
    /// The session is created and destroyed entirely inside `spawn_blocking`
    /// because `ort::Session` is `!Send`.
    ///
    /// # Arguments
    ///
    /// * `input` - ModelInput containing input_ids, attention_mask, token_type_ids
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<f32>)` - 384-dimensional embedding vector
    /// * `Err(SemanticError::Inference)` - Inference failed
    #[instrument(skip_all)]
    pub async fn run_inference(&self, input: &ModelInput) -> Result<Vec<f32>, SemanticError> {
        // Clone Arc before await to avoid holding references across suspension
        let model_bytes = Arc::clone(&self.model_bytes);
        let input = input.clone();
        let model_native_dim = self.model_variant.embedding_dim();
        let model_output_dim = self.model_variant.output_dim();

        let result = tokio::task::spawn_blocking(move || {
            let seq_len = input.seq_len();

            // Create ephemeral ort::Session from model bytes
            let mut session = Session::builder()
                .map_err(|e| {
                    SemanticError::Inference(format!(
                        "Failed to create ONNX session builder: {}",
                        e
                    ))
                })?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| {
                    SemanticError::Inference(format!("Failed to set optimization level: {}", e))
                })?
                .with_intra_threads(num_cpus::get())
                .map_err(|e| {
                    SemanticError::Inference(format!("Failed to set intra threads: {}", e))
                })?
                .commit_from_memory(&model_bytes)
                .map_err(|e| {
                    SemanticError::Inference(format!(
                        "Failed to create ONNX session from memory: {}",
                        e
                    ))
                })?;

            // Build named input tensors using ndarray + Tensor::from_array
            let input_ids_array =
                ndarray::Array2::<i64>::from_shape_vec((1, seq_len), input.input_ids.clone())
                    .map_err(|e| {
                        SemanticError::Inference(format!("Failed to create input_ids array: {}", e))
                    })?;

            let attention_mask_array =
                ndarray::Array2::<i64>::from_shape_vec((1, seq_len), input.attention_mask.clone())
                    .map_err(|e| {
                        SemanticError::Inference(format!(
                            "Failed to create attention_mask array: {}",
                            e
                        ))
                    })?;

            let token_type_ids_array =
                ndarray::Array2::<i64>::from_shape_vec((1, seq_len), input.token_type_ids.clone())
                    .map_err(|e| {
                        SemanticError::Inference(format!(
                            "Failed to create token_type_ids array: {}",
                            e
                        ))
                    })?;

            // Run inference with named inputs
            let outputs = session
                .run(ort::inputs![
                    "input_ids" => ort::value::Tensor::from_array(input_ids_array)
                        .map_err(|e| SemanticError::Inference(format!(
                            "Failed to create input_ids tensor: {}",
                            e
                        )))?,
                    "attention_mask" => ort::value::Tensor::from_array(attention_mask_array)
                        .map_err(|e| SemanticError::Inference(format!(
                            "Failed to create attention_mask tensor: {}",
                            e
                        )))?,
                    "token_type_ids" => ort::value::Tensor::from_array(token_type_ids_array)
                        .map_err(|e| SemanticError::Inference(format!(
                            "Failed to create token_type_ids tensor: {}",
                            e
                        )))?,
                ])
                .map_err(|e| SemanticError::Inference(format!("Model execution failed: {}", e)))?;

            // Extract last_hidden_state output
            let (_shape, raw_data): (_, &[f32]) = outputs["last_hidden_state"]
                .try_extract_tensor::<f32>()
                .map_err(|e| {
                    SemanticError::Inference(format!("Failed to extract last_hidden_state: {}", e))
                })?;

            // Convert to Vec<f32>
            let embedding_flat: Vec<f32> = raw_data.to_vec();

            // Apply Mean Pooling on the native embedding dimension
            // For Granite-97M: 384d → mean_pool(384) → l2_normalize
            // For Granite-311M: 768d → mean_pool(768) → Matryoshka truncate to 384 → l2_normalize
            use crate::infrastructure_ai::embedding_ops::{l2_normalize_safe, mean_pool};
            let pooled = mean_pool(
                &embedding_flat,
                seq_len,
                model_native_dim,
                &input.attention_mask,
            );

            // Matryoshka truncation: for 311M, slice native 768d down to first 384 elements
            // For 97M, model_native_dim == model_output_dim (both 384) → no-op
            let truncated: Vec<f32> = pooled.iter().take(model_output_dim).copied().collect();

            let embedding = l2_normalize_safe(&truncated);

            Ok(embedding)
        })
        .await
        .map_err(|e| SemanticError::Inference(format!("Task join error: {}", e)))?;

        result
    }

    /// Get embedding dimension (384 for all Granite models)
    ///
    /// 384-dim is invariant: Granite-97M is natively 384d, Granite-311M
    /// uses Matryoshka truncation to 384d.
    #[must_use]
    pub fn embedding_dim(&self) -> usize {
        self.model_variant.output_dim()
    }

    /// Get the AI model variant loaded in this engine
    #[must_use]
    pub fn model_variant(&self) -> AiModel {
        self.model_variant
    }

    /// Check if engine is ready for inference
    ///
    /// Returns true if model bytes are available (non-empty Arc).
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.model_bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure_ai::cache_config::AiModel;

    /// Test that InferenceEngine type exists and compiles
    #[test]
    fn test_inference_engine_type_exists() {
        fn _assert_type_exists(_engine: InferenceEngine) {}
    }

    /// Test that InferenceEngine is Send + Sync (thread-safe)
    ///
    /// This is critical for using InferenceEngine in async contexts
    /// with tokio::spawn and across thread boundaries.
    /// Uses Arc<Vec<u8>> internally, which is Send + Sync.
    #[test]
    fn test_inference_engine_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}

        assert_send::<InferenceEngine>();
        assert_sync::<InferenceEngine>();
    }

    /// Test that InferenceEngine is Clone (cheap Arc clone)
    #[test]
    fn test_inference_engine_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<InferenceEngine>();
    }

    /// RED → GREEN: load_from_file with missing file → ModelLoad error
    #[tokio::test]
    async fn test_load_from_file_missing_file_returns_model_load_error() {
        let result = InferenceEngine::load_from_file(
            "/tmp/nonexistent_model_xyz123.onnx",
            AiModel::Granite97M,
        )
        .await;

        assert!(result.is_err());

        match result {
            Err(SemanticError::ModelLoad(_)) => {
                // Expected — missing file produces ModelLoad error
            },
            other => panic!("Expected ModelLoad error, got {:?}", other),
        }
    }

    /// RED → GREEN: load_from_file with empty file → ModelLoad error
    #[tokio::test]
    async fn test_load_from_file_empty_file_returns_model_load_error() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let model_path = dir.path().join("empty.onnx");

        // Write an empty file
        std::fs::write(&model_path, b"").expect("Failed to create empty file");

        let result = InferenceEngine::load_from_file(&model_path, AiModel::Granite97M).await;

        assert!(result.is_err());

        match result {
            Err(SemanticError::ModelLoad(_)) => {
                // Expected — empty model file should produce error
            },
            other => panic!("Expected ModelLoad error for empty file, got {:?}", other),
        }
    }

    /// RED → GREEN: engine created from valid bytes has correct model variant
    #[tokio::test]
    async fn test_engine_model_variant_is_preserved() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let model_path = dir.path().join("minimal.onnx");

        std::fs::write(&model_path, b"not a real onnx model").expect("Failed to write file");

        let engine = InferenceEngine::load_from_file(&model_path, AiModel::Granite311M)
            .await
            .expect("Should load bytes");

        assert_eq!(engine.model_variant(), AiModel::Granite311M);
        assert_eq!(engine.embedding_dim(), 384); // unified output dim
    }

    /// Test that embedding_dim returns 384 for both model variants
    #[tokio::test]
    async fn test_embedding_dim_is_always_384() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");

        // Granite-97M
        let path_97m = dir.path().join("model_97m.onnx");
        std::fs::write(&path_97m, b"bytes").expect("Failed to write file");
        let engine_97m = InferenceEngine::load_from_file(&path_97m, AiModel::Granite97M)
            .await
            .expect("Should load");
        assert_eq!(engine_97m.embedding_dim(), 384);

        // Granite-311M
        let path_311m = dir.path().join("model_311m.onnx");
        std::fs::write(&path_311m, b"bytes").expect("Failed to write file");
        let engine_311m = InferenceEngine::load_from_file(&path_311m, AiModel::Granite311M)
            .await
            .expect("Should load");
        assert_eq!(engine_311m.embedding_dim(), 384);
    }

    /// Test that native dim is correct per model
    #[tokio::test]
    async fn test_native_embedding_dim_per_model() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");

        let path_97m = dir.path().join("model_97m.onnx");
        std::fs::write(&path_97m, b"bytes").expect("Failed to write file");
        let engine_97m = InferenceEngine::load_from_file(&path_97m, AiModel::Granite97M)
            .await
            .expect("Should load");
        assert_eq!(engine_97m.model_variant.embedding_dim(), 384);

        let path_311m = dir.path().join("model_311m.onnx");
        std::fs::write(&path_311m, b"bytes").expect("Failed to write file");
        let engine_311m = InferenceEngine::load_from_file(&path_311m, AiModel::Granite311M)
            .await
            .expect("Should load");
        assert_eq!(engine_311m.model_variant.embedding_dim(), 768);
    }

    /// Test that load_from_file with valid non-empty bytes succeeds
    #[tokio::test]
    async fn test_load_from_file_with_valid_bytes_succeeds() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let model_path = dir.path().join("minimal.onnx");

        std::fs::write(&model_path, b"some model bytes").expect("Failed to write file");

        let engine = InferenceEngine::load_from_file(&model_path, AiModel::Granite97M)
            .await
            .expect("Should succeed with non-empty model file");

        assert!(engine.is_ready());
    }

    /// Test ModelInput creation
    #[test]
    fn test_model_input_creation() {
        let input = ModelInput::new(
            vec![101i64, 2054, 2003, 102],
            vec![1i64, 1, 1, 1],
            vec![0i64, 0, 0, 0],
        );
        assert_eq!(input.seq_len(), 4);
        assert_eq!(input.input_ids.len(), 4);
        assert_eq!(input.attention_mask.len(), 4);
        assert_eq!(input.token_type_ids.len(), 4);
    }

    /// Test ModelInput from tokens convenience method
    #[test]
    fn test_model_input_from_tokens() {
        let input = ModelInput::from_tokens(vec![101i64, 2054, 2003, 102]);
        assert_eq!(input.seq_len(), 4);
        assert_eq!(input.input_ids, vec![101, 2054, 2003, 102]);
        assert_eq!(input.attention_mask, vec![1, 1, 1, 1]);
        assert_eq!(input.token_type_ids, vec![0, 0, 0, 0]);
    }

    /// Test that ModelInput is Clone
    #[test]
    fn test_model_input_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<ModelInput>();
    }

    /// Test Matryoshka truncation: verify that a 768d vector gets truncated to 384d
    #[test]
    fn test_matryoshka_truncation_slices_to_384() {
        use crate::infrastructure_ai::embedding_ops::{l2_normalize_safe, mean_pool};

        // Simulate 768d native output from Granite-311M
        // The hidden state is (1, seq_len, 768) where seq_len=1
        let embedding_flat_768: Vec<f32> = (0..768).map(|i| (i as f32 + 1.0) / 768.0).collect();
        let attention_mask: Vec<i64> = vec![1i64]; // seq_len=1

        // Mean pool on native 768d (1 token, so mean_pool is just the vector itself)
        let pooled = mean_pool(&embedding_flat_768, 1, 768, &attention_mask);

        // Matryoshka truncation: take first 384 elements
        let truncated: Vec<f32> = pooled.iter().take(384).copied().collect();

        // L2 normalize the truncated result
        let normalized = l2_normalize_safe(&truncated);

        // Must be exactly 384d
        assert_eq!(
            normalized.len(),
            384,
            "Matryoshka truncation must produce 384d output"
        );

        // Verify unit length
        let norm: f32 = normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "L2 norm should be 1.0, got {}",
            norm
        );
    }
}

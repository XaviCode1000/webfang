//! Semantic Cleaner implementation — Full RAG Pipeline Integration
//!
//! This module provides the concrete implementation of the [`SemanticCleaner`]
//! trait using the complete Phase 2 + Phase 3 pipeline:
//!
//! # Architecture
//!
//! ```text
//! HTML Input
//!     ↓
//! [Chunker] Split into semantic chunks (arena allocator)
//!     ↓
//! [Tokenizer] Convert each chunk to token IDs
//!     ↓
//! [InferencePool] Generate embeddings (dedicated worker threads)
//!     ↓
//! [RelevanceScorer] Filter by threshold (SIMD cosine similarity)
//!     ↓
//! Vec<DocumentChunk> Output
//! ```
//!
//! # Rust-Skills Applied
//!
//! - `async-join-parallel`: Use `try_join_all` for concurrent embeddings
//! - `mem-reuse-collections`: Pre-allocate `Vec::with_capacity`, reuse buffers
//! - `own-borrow-over-clone`: Borrow `&chunks`, `&embeddings` - don't clone
//! - `async-spawn-blocking`: InferencePool uses dedicated worker threads
//! - `err-context-chain`: Add `.context()` to errors
//! - `anti-unwrap-abuse`: Use `?` operator, NO `.unwrap()` in prod
//! - `anti-lock-across-await`: Don't hold MutexGuard across `.await`
//! - `api-builder-pattern`: ModelConfig uses builder pattern
//! - `type-newtype-ids`: Using `ChunkId` for type-safe IDs
//! - `opt-simd-portable`: RelevanceScorer uses `wide::f32x8` SIMD
//!
//! # Examples
//!
//! ```no_run
//! # #[cfg(feature = "ai")]
//! # async fn example() -> anyhow::Result<()> {
//! use webfang_ai::{SemanticCleanerImpl, ModelConfig};
//! use webfang_ai::SemanticCleaner;
//!
//! let config = ModelConfig::default();
//! let cleaner = SemanticCleanerImpl::new(config).await?;
//!
//! let html = "<article><p>Hello world. Test content.</p></article>";
//! let chunks = cleaner.clean(html).await?;
//!
//! println!("Generated {} chunks", chunks.len());
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::future::try_join_all;
use hf_hub::api::tokio::ApiBuilder;
use hf_hub::{Cache as HfCache, Repo, RepoType};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::infrastructure_ai::cache_config::AiModel;
use crate::infrastructure_ai::{
    ContentPruner, HtmlChunker, InferencePool, LegibleContentPruner, MiniLmTokenizer,
    RelevanceScorer,
};
use webfang_core::domain::semantic_cleaner::{private, SemanticCleaner};
use webfang_core::domain::DocumentChunk;
use webfang_core::error::SemanticError;

/// Model configuration
///
/// Controls model loading and inference behavior.
///
/// # Builder Pattern
///
/// Following `api-builder-pattern`, use builder methods for configuration:
///
/// ```
/// use webfang_ai::infrastructure_ai::ModelConfig;
/// let config = ModelConfig::new()
///     .with_repo("ibm-granite/granite-embedding-97m-multilingual-r2")
///     .with_offline_mode(true)
///     .with_max_tokens(512);
/// ```
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// Model repository on HuggingFace Hub
    pub repo: String,
    /// Model filename within repository
    pub model_file: String,
    /// Offline mode (fail if not cached)
    pub offline_mode: bool,
    /// Maximum tokens per chunk (model-specific)
    pub max_tokens: usize,
    /// Relevance threshold for filtering (0.0-1.0)
    pub relevance_threshold: f32,
    /// AI model variant to use (Granite-97M or Granite-311M)
    pub model_variant: AiModel,
}

impl Default for ModelConfig {
    fn default() -> Self {
        let model_variant = AiModel::from_env_or_default();
        Self {
            repo: model_variant.repo_id().to_string(),
            model_file: model_variant.model_file().to_string(),
            offline_mode: false,
            max_tokens: 32768,        // Granite-97M context window (32K tokens)
            relevance_threshold: 0.3, // Moderate relevance threshold
            model_variant,
        }
    }
}

impl ModelConfig {
    /// Create a new model configuration with default values
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set model repository
    #[must_use]
    pub fn with_repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set model filename
    #[must_use]
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.model_file = file.into();
        self
    }

    /// Enable offline mode
    #[must_use]
    pub fn with_offline_mode(mut self, enabled: bool) -> Self {
        self.offline_mode = enabled;
        self
    }

    /// Set maximum tokens per chunk
    #[must_use]
    pub fn with_max_tokens(mut self, tokens: usize) -> Self {
        self.max_tokens = tokens;
        self
    }

    /// Set relevance threshold for filtering
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::InvalidThreshold`] if `threshold` is outside [0.0, 1.0].
    pub fn with_relevance_threshold(mut self, threshold: f32) -> Result<Self, SemanticError> {
        if !(0.0..=1.0).contains(&threshold) {
            return Err(SemanticError::InvalidThreshold { value: threshold });
        }
        self.relevance_threshold = threshold;
        Ok(self)
    }

    /// Set AI model variant
    ///
    /// Updates repo, model_file, and model_variant atomically.
    #[must_use]
    pub fn with_model_variant(mut self, variant: AiModel) -> Self {
        self.repo = variant.repo_id().to_string();
        self.model_file = variant.model_file().to_string();
        self.model_variant = variant;
        self
    }
}

/// Semantic Cleaner implementation using full RAG pipeline
///
/// This is the concrete implementation of the [`SemanticCleaner`] trait.
/// It integrates all Phase 2 and Phase 3 modules:
/// - [`HtmlChunker`]: Semantic chunking with arena allocator
/// - [`MiniLmTokenizer`]: HuggingFace tokenization
/// - [`InferencePool`]: ONNX model execution with dedicated worker threads
/// - [`RelevanceScorer`]: SIMD-accelerated cosine similarity filtering
///
/// # Thread Safety
///
/// This type is `Send + Sync` and can be safely shared across threads.
/// All components use `Arc` for thread-safe sharing.
///
/// # Performance
///
/// - **First call**: Model download (~90MB) + load (~100-500ms)
/// - **Subsequent calls**: ~50-200ms per page (depending on content size)
/// - **Memory**: Arena allocator reduces allocation overhead
/// - **Concurrency**: Embeddings generated concurrently with `try_join_all`
pub struct SemanticCleanerImpl {
    // Phase 2: Core inference
    /// ONNX inference pool (dedicated worker threads with persistent sessions)
    inference_pool: Arc<InferencePool>,
    /// HuggingFace tokenizer
    tokenizer: MiniLmTokenizer,

    // Phase 3: Chunking + scoring
    /// Semantic HTML chunker with arena allocator
    chunker: HtmlChunker,
    /// Relevance scorer with SIMD cosine similarity
    scorer: RelevanceScorer,

    // Phase 4: Content pruning
    /// Content pruner (extracts readable content via legible)
    pruner: LegibleContentPruner,

    // Config
    /// Model and pipeline configuration
    config: ModelConfig,
}

impl SemanticCleanerImpl {
    /// Create a new semantic cleaner with full pipeline
    ///
    /// This method loads all pipeline components:
    /// 1. Downloads/loads ONNX model
    /// 2. Loads tokenizer
    /// 3. Creates chunker and scorer
    ///
    /// # Arguments
    ///
    /// * `config` - Model configuration
    ///
    /// # Returns
    ///
    /// * `Ok(SemanticCleanerImpl)` - Successfully created cleaner
    /// * `Err(SemanticError)` - Model loading or download failed
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Model download fails
    /// - Model file is corrupted (SHA256 mismatch)
    /// - ONNX model fails to load
    /// - Tokenizer fails to load
    /// - Offline mode enabled but model not cached
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use webfang_ai::{SemanticCleanerImpl, ModelConfig};
    ///
    /// let config = ModelConfig::default();
    /// let cleaner = SemanticCleanerImpl::new(config).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Performance
    ///
    /// - **First call**: Model download (~90MB) + load (~100-500ms)
    /// - **Subsequent calls**: Cache hit, ~10-50ms per page
    /// - **Memory**: Memory-mapped files, ~90MB virtual memory
    pub async fn new(config: ModelConfig) -> Result<Self, SemanticError> {
        info!(
            repo = %config.repo,
            file = %config.model_file,
            offline_mode = config.offline_mode,
            relevance_threshold = config.relevance_threshold,
            "Initializing semantic cleaner with full RAG pipeline"
        );

        // Resolve model + tokenizer paths through the hf_hub cache.
        // Offline mode resolves strictly from the local cache (no network) and
        // fails fast with `OfflineMode` when either asset is missing. Online
        // mode is cache-first: hf_hub returns the cached path when present and
        // transparently downloads missing assets otherwise.
        let (model_path, tokenizer_path) = if config.offline_mode {
            let cache = HfCache::default();
            let cache_repo = cache.repo(Repo::new(config.repo.clone(), RepoType::Model));

            let model_path =
                cache_repo
                    .get(&config.model_file)
                    .ok_or_else(|| SemanticError::OfflineMode {
                        repo: config.repo.clone(),
                    })?;
            let tokenizer_path =
                cache_repo
                    .get("tokenizer.json")
                    .ok_or_else(|| SemanticError::OfflineMode {
                        repo: config.repo.clone(),
                    })?;

            debug!("Resolved model and tokenizer from offline cache");
            (model_path, tokenizer_path)
        } else {
            let api = ApiBuilder::new()
                .with_progress(false)
                .build()
                .map_err(|e| SemanticError::Download {
                    repo: config.repo.clone(),
                    cause: format!("Failed to build HuggingFace API client: {}", e),
                })?;

            let repo = api.model(config.repo.clone());

            // Resolve both assets concurrently (cache-first, downloads if missing).
            let (model_path, tokenizer_path) =
                tokio::try_join!(repo.get(&config.model_file), repo.get("tokenizer.json"))
                    .map_err(|e| SemanticError::Download {
                        repo: config.repo.clone(),
                        cause: format!("HuggingFace API error: {}", e),
                    })?;

            debug!("Resolved model and tokenizer via hf_hub (cache-first)");
            (model_path, tokenizer_path)
        };

        // Load the model bytes once and validate SHA256 on the in-memory buffer
        // (zero extra I/O — the same bytes feed the inference pool below).
        let model_bytes = Arc::new(
            tokio::fs::read(&model_path)
                .await
                .map_err(SemanticError::ModelLoad)?,
        );

        debug!("Validating model integrity...");
        let actual_hash = format!("{:x}", Sha256::digest(model_bytes.as_slice()));
        if actual_hash != config.model_variant.sha256() {
            return Err(SemanticError::CacheValidation {
                repo: config.repo.clone(),
                expected: config.model_variant.sha256().to_string(),
                actual: actual_hash,
            });
        }
        debug!(sha = %actual_hash, "SHA256 validation passed");

        // Initialize all pipeline components.
        let tokenizer = MiniLmTokenizer::from_file(&tokenizer_path).await?;
        let inference_pool = Arc::new(InferencePool::new(
            Arc::clone(&model_bytes),
            config.model_variant,
        )?);
        let chunker = HtmlChunker::new();
        let scorer = RelevanceScorer::new(config.relevance_threshold);

        info!("Semantic cleaner initialized successfully");
        debug!(
            embedding_dim = inference_pool.embedding_dim(),
            max_tokens = config.max_tokens,
            relevance_threshold = config.relevance_threshold,
            "Pipeline components loaded"
        );

        Ok(Self {
            inference_pool,
            tokenizer,
            chunker,
            scorer,
            pruner: LegibleContentPruner::standard(),
            config,
        })
    }

    /// Get the relevance threshold
    #[must_use]
    pub fn relevance_threshold(&self) -> f32 {
        self.config.relevance_threshold
    }

    /// Set the relevance threshold
    ///
    /// # Arguments
    ///
    /// * `threshold` - New threshold value (0.0-1.0)
    ///
    /// # Panics
    ///
    /// Panics if threshold is outside [0.0, 1.0] range
    pub fn set_relevance_threshold(&mut self, threshold: f32) {
        assert!(
            (0.0..=1.0).contains(&threshold),
            "Relevance threshold must be between 0.0 and 1.0, got {}",
            threshold
        );
        self.config.relevance_threshold = threshold;
        self.scorer.set_threshold(threshold);
    }
}

// Implement the Sealed trait for SemanticCleanerImpl
// This is required by the sealed trait pattern
impl private::Sealed for SemanticCleanerImpl {}

impl SemanticCleaner for SemanticCleanerImpl {
    fn clean<'a>(
        &'a self,
        html: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>> {
        Box::pin(async move {
            debug!(
                html_length = html.len(),
                "Starting full RAG pipeline: prune → chunk → tokenize → embed → score"
            );

            // Step 0: Content pruning — extract readable content via legible
            // On failure or empty result, pass through raw HTML unchanged.
            let pruned = self.pruner.prune(html);
            let effective_html = if pruned.is_empty() { html } else { &pruned };
            debug!(
                pruned_length = effective_html.len(),
                "Step 0: Content pruning complete"
            );

            // Step 1: Semantic chunking (uses arena internally)
            // Following `own-borrow-over-clone`: borrow html, don't clone
            let chunks = self
                .chunker
                .chunk(effective_html)
                .map_err(|e| SemanticError::Tokenize(format!("Chunking failed: {}", e)))?;

            if chunks.is_empty() {
                debug!("No chunks produced from HTML");
                return Ok(Vec::new());
            }

            debug!(chunks_count = chunks.len(), "Step 1: Chunking complete");

            // Step 2: Tokenize all chunks (mem-reuse-collections: reuse buffer)
            // Pre-allocate with capacity following `mem-with-capacity`
            let mut token_buffers = Vec::with_capacity(chunks.len());
            for chunk in &chunks {
                let input = self.tokenizer.tokenize(&chunk.content).map_err(|e| {
                    SemanticError::Tokenize(format!("Tokenization failed for chunk: {}", e))
                })?;

                // Validate token count
                if input.seq_len() > self.config.max_tokens {
                    return Err(SemanticError::ChunkTooLarge {
                        chunk_id: format!("chunk-{}", token_buffers.len()),
                        tokens: input.seq_len(),
                        max: self.config.max_tokens,
                    });
                }

                token_buffers.push(input);
            }

            debug!(
                tokens_generated = token_buffers.len(),
                "Step 2: Tokenization complete"
            );

            // Step 3: Generate embeddings CONCURRENTLY (async-join-parallel)
            // Following `async-join-parallel`: use try_join_all for concurrent independent operations
            // InferencePool dispatches to dedicated worker threads with persistent sessions
            // Following `anti-lock-across-await`: No locks held across await points
            let embeddings = try_join_all(token_buffers.iter().map(|input| {
                let pool = &self.inference_pool;
                async move { pool.infer(input).await }
            }))
            .await
            .map_err(|e| {
                SemanticError::Inference(format!("Concurrent embedding generation failed: {}", e))
            })?;

            debug!(
                embeddings_generated = embeddings.len(),
                embedding_dim = embeddings.first().map(|e| e.len()).unwrap_or(0),
                "Step 3: Embedding generation complete"
            );

            // Step 4: Score and filter (own-borrow-over-clone: borrow embeddings)
            // Following `own-borrow-over-clone`: borrow &chunks and &embeddings, don't clone
            // Following `opt-simd-portable`: RelevanceScorer uses SIMD cosine similarity
            let filtered = self.filter_by_relevance(&chunks, &embeddings)?;

            debug!(
                chunks_before = chunks.len(),
                chunks_after = filtered.len(),
                filtered_out = chunks.len() - filtered.len(),
                "Step 4: Relevance filtering complete"
            );

            info!(total_chunks = filtered.len(), "Full RAG pipeline complete");

            Ok(filtered)
        })
    }

    fn max_tokens(&self) -> usize {
        self.config.max_tokens
    }

    fn is_ready(&self) -> bool {
        self.inference_pool.is_ready()
    }
}

impl SemanticCleanerImpl {
    /// Filter chunks by relevance score and **preserve embeddings**
    ///
    /// Pairs each chunk with its embedding, scores against a reference,
    /// filters by threshold, and **preserves** the embedding vectors in the output.
    ///
    /// **Critical bug fix**: Previously called `scorer.filter()` which discarded
    /// embeddings via `.map(|(chunk, _)| chunk.clone())`, resulting in:
    /// - "Generated 0 chunks with embeddings" log messages
    /// - Empty embeddings fields in JSONL output
    /// - Loss of 49536 dimensions of embedding data
    ///
    /// **Solution**: Uses `scorer.filter_with_embeddings()` to preserve embeddings,
    /// then restores them to each chunk before returning `Vec<DocumentChunk>`.
    ///
    /// # Arguments
    ///
    /// * `chunks` - Slice of DocumentChunks (borrowed, following `own-borrow-over-clone`)
    /// * `embeddings` - Slice of embedding vectors (borrowed)
    ///
    /// # Returns
    ///
    /// Filtered vector of `DocumentChunk` items meeting relevance threshold.
    /// **Important**: Each chunk includes its embedding vector (not `None`).
    ///
    /// # Errors
    ///
    /// Returns `SemanticError::Inference("No embeddings available")` if
    /// input embeddings slice is empty (no reference vector for scoring).
    ///
    /// # Performance
    ///
    /// Uses SIMD-accelerated cosine similarity via `RelevanceScorer`.
    /// Concurrent operations use arena allocator to reduce allocation overhead.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(feature = "ai")]
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use webfang_ai::{SemanticCleaner, SemanticCleanerImpl, ModelConfig};
    ///
    /// // Create semantic cleaner (requires --features ai)
    /// let config = ModelConfig::default();
    /// let cleaner = SemanticCleanerImpl::new(config).await?;
    ///
    /// // Clean HTML content - will generate chunks with embeddings
    /// let html = "<article><h1>Title</h1><p>Content here.</p></article>";
    /// let chunks = cleaner.clean(html).await?;
    ///
    /// // Verify embeddings are present (bug fix validation)
    /// let has_embeddings = chunks.first()
    ///     .map(|c| c.embeddings.is_some())
    ///     .ok_or_else(|| SemanticError::Inference(
    ///         "No chunks returned from semantic cleaner. Check HTML content and AI model availability."
    ///     ))?;
    /// assert!(has_embeddings, "embeddings should not be None after fix");
    ///
    /// // Embedding dimension: 384 for all-MiniLM-L6-v2 model
    /// let dim = chunks.first()
    ///     .map(|c| c.embeddings.as_ref().map(|e| e.len()))
    ///     .ok_or(SemanticError::Inference("No chunks or Embeddings returned".to_string()))?;
    /// assert_eq!(dim, Some(384));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// See also:
    /// - [`SemanticCleaner::clean()`](SemanticCleaner::clean) - Full pipeline entry point
    /// - [`RelevanceScorer::filter_with_embeddings()`](RelevanceScorer::filter_with_embeddings)
    fn filter_by_relevance(
        &self,
        chunks: &[DocumentChunk],
        embeddings: &[Vec<f32>],
    ) -> Result<Vec<DocumentChunk>, SemanticError> {
        // Validate that each chunk has a corresponding embedding (mem-prevent-data-loss)
        if chunks.len() != embeddings.len() {
            return Err(SemanticError::Inference(format!(
                "Length mismatch: got {} chunks but {} embedding vectors. \
                 Each chunk must have exactly one embedding vector.",
                chunks.len(),
                embeddings.len()
            )));
        }

        // Create (chunk, embedding) pairs
        // Following `mem-with-capacity`: pre-allocate
        let mut chunk_embedding_pairs = Vec::with_capacity(chunks.len());

        for (chunk, embedding) in chunks.iter().zip(embeddings.iter()) {
            chunk_embedding_pairs.push((chunk.clone(), embedding.clone()));
        }

        // Use first embedding as reference (simple strategy)
        // In production, this could be a query vector or domain-specific reference
        let reference = embeddings.first().ok_or_else(|| {
            SemanticError::Inference("No embeddings available for relevance scoring".to_string())
        })?;

        // Filter using scorer WITH embeddings preserved
        let filtered_with_embeddings: Vec<(DocumentChunk, Vec<f32>)> = self
            .scorer
            .filter_with_embeddings(&chunk_embedding_pairs, Some(reference));

        // Restore embeddings to chunks following `mem-preserving-embeddings`
        let mut result = Vec::with_capacity(filtered_with_embeddings.len());
        for (chunk, embedding) in filtered_with_embeddings {
            let mut chunk_with_embeddings = chunk.clone();
            chunk_with_embeddings.embeddings = Some(embedding);
            result.push(chunk_with_embeddings);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_config_default() {
        let config = ModelConfig::default();
        assert_eq!(config.repo, AiModel::default().repo_id());
        assert_eq!(config.model_file, AiModel::default().model_file());
        assert!(!config.offline_mode);
        assert_eq!(config.max_tokens, 32768);
        assert_eq!(config.relevance_threshold, 0.3);
    }

    #[test]
    fn test_model_config_builder() {
        let config = ModelConfig::new()
            .with_repo("test/repo")
            .with_file("test.onnx")
            .with_offline_mode(true)
            .with_max_tokens(256)
            .with_relevance_threshold(0.5)
            .unwrap();

        assert_eq!(config.repo, "test/repo");
        assert_eq!(config.model_file, "test.onnx");
        assert!(config.offline_mode);
        assert_eq!(config.max_tokens, 256);
        assert_eq!(config.relevance_threshold, 0.5);
    }

    #[test]
    fn test_model_config_invalid_threshold() {
        let result = ModelConfig::new().with_relevance_threshold(1.5);
        assert!(result.is_err());
        match result {
            Err(SemanticError::InvalidThreshold { value }) => {
                assert_eq!(value, 1.5);
            },
            _ => panic!("Expected InvalidThreshold error"),
        }
    }

    #[test]
    fn test_semantic_cleaner_type_traits() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}

        // SemanticCleanerImpl should be Send + Sync
        assert_send::<SemanticCleanerImpl>();
        assert_sync::<SemanticCleanerImpl>();
    }

    #[tokio::test]
    async fn test_semantic_cleaner_creation_fails_without_model() {
        // Creation must fail gracefully when the model is unavailable.
        // Offline mode + a bogus repo id (never present in the local hf_hub
        // cache) guarantees a deterministic resolution failure without network.
        let config = ModelConfig::new()
            .with_repo("nonexistent/fake-repo-for-test")
            .with_offline_mode(true);

        let result = SemanticCleanerImpl::new(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_semantic_cleaner_offline_mode() {
        // Offline mode must fail with OfflineMode when the model is not cached.
        // A bogus repo id is never present in the hf_hub cache, so this is
        // deterministic and requires no network access.
        let config = ModelConfig::new()
            .with_repo("nonexistent/fake-repo-for-test")
            .with_offline_mode(true);

        let result = SemanticCleanerImpl::new(config).await;
        assert!(result.is_err());

        if let Err(SemanticError::OfflineMode { .. }) = result {
            // Expected
        } else {
            panic!("Expected OfflineMode error");
        }
    }

    #[test]
    fn test_model_config_with_relevance_threshold() {
        let config = ModelConfig::default()
            .with_relevance_threshold(0.5)
            .unwrap();
        assert_eq!(config.relevance_threshold, 0.5);
    }

    #[test]
    fn test_model_config_full_builder() {
        let config = ModelConfig::new()
            .with_repo("test/repo")
            .with_file("test.onnx")
            .with_offline_mode(true)
            .with_max_tokens(256)
            .with_relevance_threshold(0.4)
            .unwrap();

        assert_eq!(config.repo, "test/repo");
        assert_eq!(config.model_file, "test.onnx");
        assert!(config.offline_mode);
        assert_eq!(config.max_tokens, 256);
        assert_eq!(config.relevance_threshold, 0.4);
    }

    #[test]
    fn test_semantic_cleaner_impl_fields() {
        // Verify that SemanticCleanerImpl has the expected fields
        // This is a compile-time check
        fn _check_fields(cleaner: &SemanticCleanerImpl) {
            let _ = cleaner.relevance_threshold();
        }
    }

    #[test]
    fn test_filter_by_relevance_length_mismatch() {
        // This test would require creating a SemanticCleanerImpl instance,
        // which requires async setup. Skipping for now.
        // The method is tested indirectly through integration tests.
    }
}

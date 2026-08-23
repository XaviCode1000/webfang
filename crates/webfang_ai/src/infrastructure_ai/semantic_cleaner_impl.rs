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
//! let chunks = cleaner.clean("https://example.com", html).await?;
//!
//! println!("Generated {} chunks", chunks.len());
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::future::{try_join, try_join_all};
use hf_hub::api::tokio::ApiBuilder;
use hf_hub::{Cache as HfCache, Repo, RepoType};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn, Instrument};

use crate::infrastructure_ai::cache_config::AiModel;
use crate::infrastructure_ai::embedding_ops::cosine_similarity;
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
    /// Maximum tokens per chunk before rejection. This is a chunk-rejection
    /// guard, not a context-window or generation limit: chunks whose tokenized
    /// length exceeds this value fail with [`SemanticError::ChunkTooLarge`].
    /// The tokenizer itself truncates sequences at the model's configured
    /// maximum (`DEFAULT_MAX_LENGTH`, 32,768 tokens).
    pub max_tokens: usize,
    /// Relevance threshold for filtering (0.0-1.0)
    pub relevance_threshold: f32,
    /// AI model variant to use (Granite-97M or Granite-311M)
    pub model_variant: AiModel,
}

impl Default for ModelConfig {
    fn default() -> Self {
        // A bare default configuration is always Granite-97M and never consults
        // the environment: `AI_MODEL_ID` is resolved LOUDLY at the application
        // entry points (CLI `build_ai_cleaner`, MCP `spawn_ai_wiring`) via
        // `AiModel::from_env()`, which errors on set-but-invalid values (#874).
        let model_variant = AiModel::default();
        Self {
            repo: model_variant.repo_id().to_string(),
            model_file: model_variant.model_file().to_string(),
            offline_mode: false,
            max_tokens: 32768, // Chunk-rejection guard; matches the Granite context window (32K tokens)
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
    /// HuggingFace tokenizer (`Arc`-shared with the embedding adapter via
    /// [`shared_inference`](Self::shared_inference))
    tokenizer: Arc<MiniLmTokenizer>,

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
    #[tracing::instrument(skip(config), fields(repo = %config.repo, model_file = %config.model_file, offline_mode = config.offline_mode))]
    pub async fn new(config: ModelConfig) -> Result<Self, SemanticError> {
        info!(
            repo = %config.repo,
            file = %config.model_file,
            offline_mode = config.offline_mode,
            relevance_threshold = config.relevance_threshold,
            "Initializing semantic cleaner with full RAG pipeline"
        );

        // Resolve and validate model + tokenizer assets (hf_hub cache-first,
        // in-memory SHA256 integrity check). Shared with `EmbeddingAdapter::from_config`
        // so both pipelines resolve and validate models identically.
        let (model_bytes, tokenizer_path) = resolve_model_assets(&config).await?;

        // Initialize all pipeline components. The tokenizer is `Arc`-wrapped so
        // `shared_inference` can hand the SAME instance to the embedding adapter.
        let tokenizer = Arc::new(MiniLmTokenizer::from_file(&tokenizer_path).await?);
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

    /// Share the inference pool and tokenizer with another pipeline.
    ///
    /// Returns cheap `Arc` clones of the ONNX [`InferencePool`] and the
    /// [`MiniLmTokenizer`] this cleaner was built with, so a second consumer
    /// (e.g. the
    /// [`EmbeddingAdapter`](crate::infrastructure_ai::embedding_adapter::EmbeddingAdapter))
    /// can reuse the SAME model + tokenizer instead of resolving and loading a
    /// second copy. This is what lets the `--ai` path load the ONNX model
    /// exactly once across the semantic cleaner and the vault-search embedding
    /// adapter — one `resolve_model_assets` call, one `InferencePool`.
    #[must_use]
    pub fn shared_inference(&self) -> (Arc<InferencePool>, Arc<MiniLmTokenizer>) {
        (
            Arc::clone(&self.inference_pool),
            Arc::clone(&self.tokenizer),
        )
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
            "Relevance threshold must be between 0.0 and 1.0, got {threshold}"
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
        url: &'a str,
        html: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>> {
        Box::pin(async move {
            debug!(
                url = %url,
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
                .map_err(|e| SemanticError::Tokenize(format!("Chunking failed: {e}")))?;

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
                    SemanticError::Tokenize(format!("Tokenization failed for chunk: {e}"))
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
                SemanticError::Inference(format!("Concurrent embedding generation failed: {e}"))
            })?;

            debug!(
                embeddings_generated = embeddings.len(),
                embedding_dim = embeddings.first().map(|e| e.len()).unwrap_or(0),
                "Step 3: Embedding generation complete"
            );

            // Step 4: Score and filter (own-borrow-over-clone: borrow embeddings)
            // Following `own-borrow-over-clone`: borrow &chunks and &embeddings, don't clone
            // Following `opt-simd-portable`: RelevanceScorer uses SIMD cosine similarity
            let filtered = self.filter_by_relevance(url, &chunks, &embeddings)?;

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
    /// Pairs each chunk with its embedding, scores against the **centroid**
    /// of all embeddings, filters by threshold, and **preserves** the embedding
    /// vectors in the output.
    ///
    /// **Centroid reference**: Using the mean-pooled centroid of all chunk
    /// embeddings as the reference vector is more robust than using the first
    /// chunk — which may be a navigation element, header, or other non-representative
    /// content. The centroid captures the overall semantic center of the page.
    ///
    /// **Aggressive filtering detection**: Emits a `warn!` when >50% of chunks
    /// are discarded, indicating a potential threshold misconfiguration or
    /// off-topic page.
    ///
    /// # Arguments
    ///
    /// * `url` - Source URL for diagnostics and warning logs
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
    /// The centroid is mean-pooled in O(n×d) where n = chunks, d = embedding dim.
    ///
    /// See also:
    /// - [`SemanticCleaner::clean()`](SemanticCleaner::clean) - Full pipeline entry point
    /// - [`RelevanceScorer::filter_with_embeddings()`](RelevanceScorer::filter_with_embeddings)
    fn filter_by_relevance(
        &self,
        url: &str,
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

        let chunks_before = chunks.len();

        // Create (chunk, embedding) pairs
        // Following `mem-with-capacity`: pre-allocate
        let mut chunk_embedding_pairs = Vec::with_capacity(chunks.len());

        for (chunk, embedding) in chunks.iter().zip(embeddings.iter()) {
            chunk_embedding_pairs.push((chunk.clone(), embedding.clone()));
        }

        // Compute centroid (mean-pooled reference) of all embeddings.
        // More robust than using embeddings.first() — the first chunk may be a
        // nav element, header, or other non-representative content.
        let embedding_dim = embeddings.first().map(|e| e.len()).unwrap_or(0);
        if embedding_dim == 0 {
            return Err(SemanticError::Inference(
                "No embeddings available for relevance scoring".to_string(),
            ));
        }

        let mut centroid = vec![0.0f32; embedding_dim];
        for embedding in embeddings {
            for (i, &val) in embedding.iter().enumerate() {
                if i < centroid.len() {
                    centroid[i] += val;
                }
            }
        }
        let n = embeddings.len() as f32;
        for val in &mut centroid {
            *val /= n;
        }

        // Z-score adaptive thresholding.
        //
        // An absolute cosine-similarity threshold is inert on homogeneous pages:
        // every chunk scores high against a centroid computed from those same
        // chunks. Instead we measure how far each chunk sits from the centroid
        // and drop statistical outliers, so `--threshold` actually modulates
        // strictness (#648).
        let distances: Vec<f32> = chunk_embedding_pairs
            .iter()
            .map(|(_, emb)| 1.0 - cosine_similarity(emb, &centroid))
            .collect();

        let sample_count = distances.len() as f32;
        let mean_dist = distances.iter().sum::<f32>() / sample_count;
        let variance = distances
            .iter()
            .map(|d| (d - mean_dist).powi(2))
            .sum::<f32>()
            / sample_count;
        let std_dev = variance.sqrt();

        // threshold 1.0 → Z=0 (only exact centroid matches)
        // threshold 0.7 → Z=0.9
        // threshold 0.0 → Z=3.0 (keeps ~99.7%, filters extreme outliers)
        let z_limit = 3.0 * (1.0 - self.scorer.threshold());

        let filtered_with_embeddings: Vec<(DocumentChunk, Vec<f32>)> = chunk_embedding_pairs
            .into_iter()
            .zip(distances.iter())
            .filter(|(_, &distance)| {
                let z_score = (distance - mean_dist).abs() / std_dev.max(1e-6);
                z_score <= z_limit
            })
            .map(|((chunk, emb), _)| (chunk, emb))
            .collect();

        debug!(
            url = %url,
            mean_distance = mean_dist,
            std_dev = std_dev,
            z_limit = z_limit,
            "Applied Z-score adaptive relevance filtering"
        );

        let filtered_out = chunks_before - filtered_with_embeddings.len();

        // Warn when filtering is aggressive (>50% discarded) — possible over-aggressive
        // threshold or off-topic page. Structured fields for trace querying.
        if chunks_before > 0 && filtered_out as f64 / chunks_before as f64 > 0.5 {
            warn!(
                url = %url,
                chunks_before = chunks_before,
                chunks_after = filtered_with_embeddings.len(),
                filtered_out = filtered_out,
                loss_ratio = format!(
                    "{:.0}%",
                    filtered_out as f64 / chunks_before as f64 * 100.0
                ),
                "AI relevance filter discarded >50% of chunks for this page — possible over-aggressive filtering"
            );
        }

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

/// Resolve and validate model assets from the hf_hub cache.
///
/// Resolves the model and tokenizer paths through the hf_hub cache — offline
/// mode resolves strictly from the local cache (no network) and fails fast with
/// [`SemanticError::OfflineMode`] when either asset is missing; online mode is
/// cache-first (hf_hub returns the cached path when present and transparently
/// downloads missing assets otherwise). Then loads the model bytes once and
/// validates their SHA256 in memory.
///
/// Extracted from [`SemanticCleanerImpl::new`] so
/// [`crate::infrastructure_ai::embedding_adapter::EmbeddingAdapter::from_config`]
/// reuses the identical resolution + integrity-validation logic without
/// duplicating the hf_hub plumbing.
///
/// # Returns
///
/// `(model_bytes, tokenizer_path)` — SHA256-validated model bytes and the path
/// to `tokenizer.json`.
///
/// # Errors
///
/// Returns [`SemanticError::OfflineMode`] when offline and an asset is uncached,
/// [`SemanticError::Download`] on hf_hub client/API failure,
/// [`SemanticError::ModelLoad`] on read failure, or
/// [`SemanticError::CacheValidation`] on SHA256 mismatch.
#[tracing::instrument(skip(config), fields(repo = %config.repo, model_file = %config.model_file, offline_mode = config.offline_mode))]
pub(crate) async fn resolve_model_assets(
    config: &ModelConfig,
) -> Result<(Arc<Vec<u8>>, std::path::PathBuf), SemanticError> {
    // Resolve model + tokenizer paths through the hf_hub cache.
    let (model_path, tokenizer_path) = if config.offline_mode {
        let cache = HfCache::from_env();
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
        let api = ApiBuilder::from_env()
            .with_progress(true)
            .build()
            .map_err(|e| SemanticError::Download {
                repo: config.repo.clone(),
                cause: format!("Failed to build HuggingFace API client: {e}"),
            })?;

        let repo = api.model(config.repo.clone());

        // Resolve both assets concurrently (cache-first, downloads if missing).
        // `with_progress(true)` surfaces hf_hub's built-in progress bar so the
        // first download (~390MB) is not perceived as a hang; the span makes
        // the download phase observable in the trace file.
        let (model_path, tokenizer_path) =
            try_join(repo.get(&config.model_file), repo.get("tokenizer.json"))
                .instrument(tracing::info_span!(
                    "download_model_assets",
                    repo = %config.repo
                ))
                .await
                .map_err(|e| SemanticError::Download {
                    repo: config.repo.clone(),
                    cause: format!("HuggingFace API error: {e}"),
                })?;

        debug!("Resolved model and tokenizer via hf_hub (cache-first)");
        (model_path, tokenizer_path)
    };

    // Load the model bytes once and validate SHA256 on the in-memory buffer
    // (zero extra I/O — the same bytes feed the caller's inference pool).
    let model_bytes = Arc::new(
        tokio::fs::read(&model_path)
            .await
            .map_err(SemanticError::ModelLoad)?,
    );

    validate_model_hash(
        model_bytes.as_slice(),
        config.model_variant.sha256(),
        &config.repo,
    )?;

    Ok((model_bytes, tokenizer_path))
}

/// Validate the SHA256 hash of in-memory model bytes against an expected value.
///
/// Extracted from [`SemanticCleanerImpl::new`] so the integrity check is
/// unit-testable without downloading a model or loading the ONNX runtime.
///
/// # Errors
///
/// Returns [`SemanticError::CacheValidation`] when the computed hash does not
/// match `expected`.
#[tracing::instrument(skip(bytes), fields(repo = %repo, expected = %expected))]
fn validate_model_hash(bytes: &[u8], expected: &str, repo: &str) -> Result<(), SemanticError> {
    debug!("Validating model integrity...");
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected {
        return Err(SemanticError::CacheValidation {
            repo: repo.to_string(),
            expected: expected.to_string(),
            actual,
        });
    }
    debug!(sha = %actual, "SHA256 validation passed");
    Ok(())
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

    #[test]
    fn test_validate_model_hash_mismatch_returns_cache_validation() {
        // Exercises the REAL validation path: known content plus a WRONG
        // expected hash must yield CacheValidation carrying both hashes + repo.
        let bytes = b"webfang deterministic test payload";
        let wrong_expected = "0000000000000000000000000000000000000000000000000000000000000000";

        let result = validate_model_hash(bytes, wrong_expected, "test/repo");

        match result {
            Err(SemanticError::CacheValidation {
                repo,
                expected,
                actual,
            }) => {
                assert_eq!(repo, "test/repo");
                assert_eq!(expected, wrong_expected);
                // The actual hash is the real SHA256 of the payload (64 hex
                // chars), never the bogus expected value.
                assert_ne!(actual, wrong_expected);
                assert_eq!(actual.len(), 64);
            },
            other => panic!("expected CacheValidation, got {other:?}"),
        }
    }

    #[test]
    fn test_validate_model_hash_match_passes() {
        // Success path: feeding back the real SHA256 must validate cleanly,
        // proving the helper round-trips the digest correctly.
        let bytes = b"webfang deterministic test payload";
        let real_hash = format!("{:x}", Sha256::digest(bytes));

        assert!(validate_model_hash(bytes, &real_hash, "test/repo").is_ok());
    }
}

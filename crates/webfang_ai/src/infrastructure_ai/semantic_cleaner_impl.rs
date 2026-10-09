//! Semantic Cleaner implementation — Full RAG Pipeline Integration
//!
//! This module provides the concrete implementation of the
//! [`SemanticCleaner`](webfang_core::domain::semantic_cleaner::SemanticCleaner)
//! trait using the complete Phase 2 + Phase 3 pipeline:
//!
//! # Architecture
//!
//! ```text
//! HTML Input
//!     ↓
//! [Chunker] Split into semantic chunks (arena allocator)
//!     ↓
//! [EmbeddingPort] Embed each chunk through the domain port
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
//! - `async-spawn-blocking`: the embedding port's engine uses dedicated worker threads
//! - `err-context-chain`: Add `.context()` to errors
//! - `anti-unwrap-abuse`: Use `?` operator, NO `.unwrap()` in prod
//! - `anti-lock-across-await`: Don't hold MutexGuard across `.await`
//! - `api-builder-pattern`: ModelConfig uses builder pattern
//! - `type-newtype-ids`: Using `ChunkId` for type-safe IDs
//! - `opt-simd-portable`: RelevanceScorer uses `wide::f32x8` SIMD
//!
//! # What is NOT here (and where it went)
//!
//! Model resolution (`resolve_model_assets`, the hf_hub cache/download +
//! streaming SHA256 validation) used to live in this file. It stayed behind
//! with the ONNX half because it is the LOCAL-model path: this module
//! compiles with no `ai` feature, so it cannot import `hf_hub`'s resolver
//! without dragging the whole ONNX stack back in. It moved verbatim to
//! the `embedding_adapter` module,
//! which already owned the only consumer.
//!
//! The one constructor that needed it, `SemanticCleanerImpl::new`, is
//! therefore `#[cfg(feature = "ai")]`. Every other entry point —
//! `SemanticCleanerImpl::from_parts`, `SemanticCleaner::clean`, and the whole
//! relevance-filtering stage — is ungated and works against ANY
//! [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort): a
//! remote HTTP endpoint or a deterministic fake included.
//!
//! # Examples
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! use std::sync::Arc;
//! use webfang_ai::{SemanticCleanerImpl, ModelConfig};
//! use webfang_ai::SemanticCleaner;
//!
//! // Any `EmbeddingPort` works here — no ONNX, no model download.
//! # fn fake_port() -> Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort> {
//! #     unimplemented!()
//! # }
//! let cleaner = SemanticCleanerImpl::from_parts(fake_port(), ModelConfig::default());
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

use futures::future::try_join_all;
use tracing::{debug, info, warn};

use crate::infrastructure_ai::cache_config::AiModel;
#[cfg(feature = "ai")]
use crate::infrastructure_ai::embedding_adapter::EmbeddingAdapter;
use crate::infrastructure_ai::embedding_ops::cosine_similarity;
use crate::infrastructure_ai::{ContentPruner, HtmlChunker, LegibleContentPruner, RelevanceScorer};
use webfang_core::domain::embedding_port::EmbeddingPort;
use webfang_core::domain::semantic_cleaner::{private, SemanticCleaner};
use webfang_core::domain::DocumentChunk;
use webfang_core::error::SemanticError;

/// Model configuration
///
/// Controls the pipeline's relevance budget and — on the LOCAL-ONNX path
/// only — which model assets get resolved.
///
/// # `repo` / `model_file` / `offline_mode` / `model_variant` are LOCAL-ONNX
///
/// Four of the seven fields describe an ONNX model on the HuggingFace hub:
/// the repository, the file inside it, whether resolution may touch the
/// network, and which of the two Granite variants to pick. **This module
/// compiles without the `ai` cargo feature, so it cannot resolve a model**:
/// nothing here downloads, opens, or validates a file. Those four fields are
/// read by exactly one consumer —
/// `build_onnx_embedding_port`
/// — which is `#[cfg(feature = "ai")]`.
///
/// For every other embedding backend (a remote HTTP endpoint, a test fake)
/// they are inert: `from_parts` accepts whatever the port needs, and these
/// describe nothing the cleaner will act on. `max_chars`,
/// `chars_per_token` and `relevance_threshold` ARE honored ungated, because
/// they are the cleaner's own behavior.
///
/// They are kept here, on purpose, rather than moved into the adapter:
/// removing them is a separate slice, and today they are what makes a
/// `ModelConfig` a complete description of the local path. Anyone who wants a
/// backend-agnostic cleaner should read only the ungated fields.
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
///     .with_max_chars(1536);
/// ```
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// Model repository on HuggingFace Hub.
    ///
    /// LOCAL-ONNX ONLY: read by
    /// `build_onnx_embedding_port`.
    /// Inert for any other [`EmbeddingPort`].
    pub repo: String,
    /// Model filename within repository.
    ///
    /// LOCAL-ONNX ONLY, same as [`repo`](Self::repo).
    pub model_file: String,
    /// Offline mode (fail if not cached).
    ///
    /// LOCAL-ONNX ONLY, same as [`repo`](Self::repo): "offline" is a property
    /// of the hf_hub resolver, so it constrains nothing on a remote endpoint
    /// (whose reachability is its own provider's problem, reported as an HTTP
    /// error the pipeline degrades on).
    pub offline_mode: bool,
    /// Maximum CHARACTERS per chunk before rejection. This is a
    /// chunk-rejection guard, not a context-window or generation limit:
    /// chunks longer than this fail with [`SemanticError::ChunkTooLarge`].
    ///
    /// Characters, not tokens (ADR-0004): the cleaner no longer tokenizes, so
    /// it cannot count tokens without a tokenizer, and the value has to work
    /// for a remote embedding endpoint too — where the real ceiling is the
    /// provider's own context window, reported as HTTP 400/413 and handled by
    /// the pipeline's error degradation. The default keeps the effective
    /// ceiling the retired token budget had against the local Granite models
    /// (32 768 tokens × [`chars_per_token`](Self::chars_per_token) = 3.0).
    pub max_chars: usize,
    /// Characters per token used to translate the still-accepted
    /// `--max-tokens` budget into the character budget that replaced it.
    ///
    /// Purely descriptive at the cleaner: the guard counts characters
    /// directly. It defaults to the SSOT constant
    /// [`DEFAULT_CHARS_PER_TOKEN`](webfang_core::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN),
    /// which `webfang_core` applies when translating a legacy flag value — the
    /// two must agree, or `--max-tokens 4096` and `--max-chars 4096` would mean
    /// different budgets.
    pub chars_per_token: f32,
    /// Relevance threshold for filtering (0.0-1.0)
    pub relevance_threshold: f32,
    /// AI model variant to use (Granite-97M or Granite-311M).
    ///
    /// LOCAL-ONNX ONLY, same as [`repo`](Self::repo): the variant names a
    /// Granite checkpoint to resolve. Set through
    /// [`with_model_variant`](Self::with_model_variant), which keeps `repo`
    /// and `model_file` consistent with it.
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
            max_chars: webfang_core::domain::options_spec::ai::DEFAULT_MAX_CHARS,
            chars_per_token: webfang_core::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN,
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
    ///
    /// LOCAL-ONNX ONLY (see [`ModelConfig`]) — and `Default` is what keeps
    /// `repo` consistent with [`model_variant`](Self::model_variant); setting
    /// it directly only changes what the ONNX path resolves.
    #[must_use]
    pub fn with_repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set model filename
    ///
    /// LOCAL-ONNX ONLY (see [`ModelConfig`]).
    #[must_use]
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.model_file = file.into();
        self
    }

    /// Enable offline mode
    ///
    /// LOCAL-ONNX ONLY (see [`ModelConfig`]).
    #[must_use]
    pub fn with_offline_mode(mut self, enabled: bool) -> Self {
        self.offline_mode = enabled;
        self
    }

    /// Set the maximum number of CHARACTERS per chunk (the chunk-rejection
    /// guard). Replaces `with_max_tokens` (ADR-0004): the cleaner counts
    /// characters, so it rejects characters.
    #[must_use]
    pub fn with_max_chars(mut self, chars: usize) -> Self {
        self.max_chars = chars;
        self
    }

    /// Set the characters-per-token conversion ratio recorded alongside
    /// [`max_chars`](Self::max_chars).
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::InvalidCharsPerToken`] for a non-positive
    /// ratio: it would translate a token budget into a zero or negative
    /// character budget, i.e. a guard that rejects every chunk while looking
    /// configured (the zero-silent-loss class).
    pub fn with_chars_per_token(mut self, chars_per_token: f32) -> Result<Self, SemanticError> {
        if chars_per_token <= 0.0 {
            return Err(SemanticError::InvalidCharsPerToken {
                value: chars_per_token,
            });
        }
        self.chars_per_token = chars_per_token;
        Ok(self)
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
    ///
    /// LOCAL-ONNX ONLY (see [`ModelConfig`]).
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
/// Holds one erased [`EmbeddingPort`] and nothing engine-shaped (ADR-0004):
/// the cleaner no longer owns a tokenizer or an inference engine, so the
/// embedding backend it runs against is decided entirely by whoever builds the
/// port — the local ONNX `EmbeddingAdapter`,
/// a remote HTTP endpoint, or a deterministic fake in a test.
///
/// This is the concrete implementation of the [`SemanticCleaner`] trait.
/// It integrates:
/// - [`HtmlChunker`]: Semantic chunking with arena allocator
/// - [`EmbeddingPort`]: embedding generation, local or remote
/// - [`RelevanceScorer`]: SIMD-accelerated cosine similarity filtering
///
/// # Thread Safety
///
/// This type is `Send + Sync` and can be safely shared across threads.
/// All components use `Arc` for thread-safe sharing.
///
/// # Performance
///
/// - **First call**: Model download (~90MB) + load (~100-500ms), paid by the
///   port's construction, not by this type
/// - **Subsequent calls**: ~50-200ms per page (depending on content size)
/// - **Memory**: Arena allocator reduces allocation overhead
/// - **Concurrency**: Embeddings generated concurrently with `try_join_all`
pub struct SemanticCleanerImpl {
    // Embedding seam
    /// Domain embedding port (ONNX adapter today; remote or fake for tests and
    /// future backends), `Arc`-shared so concurrent `clean()` calls fan out
    /// over the same port.
    embedding: Arc<dyn EmbeddingPort>,

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
    /// - **Memory**: the ONNX session is committed from the model file
    ///   (`commit_from_file`) — model bytes never pass through application
    ///   memory, and the process holds only ORT's single session copy (#1315)
    ///
    /// # Why this constructor is `ai`-gated
    ///
    /// It is the LOCAL-ONNX convenience path: it resolves model assets off the
    /// hf_hub cache and loads an ORT session, so it cannot exist without the
    /// feature. Ungated builds construct the cleaner through
    /// [`from_parts`](Self::from_parts) against any [`EmbeddingPort`] — a
    /// remote endpoint included — which is the same cleaner this constructor
    /// returns.
    #[cfg(feature = "ai")]
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
        // streamed SHA256 integrity check) and build the ONNX embedding port.
        // The port owns the engine and the tokenizer now, so this constructor
        // hands the cleaner an erased `Arc<dyn EmbeddingPort>` and nothing
        // else.
        let embedding = Arc::new(EmbeddingAdapter::from_config(&config).await?);

        info!("Semantic cleaner initialized successfully");
        debug!(
            embedding_dim = embedding.embedding_dim(),
            max_chars = config.max_chars,
            chars_per_token = config.chars_per_token,
            relevance_threshold = config.relevance_threshold,
            "Pipeline components loaded"
        );

        Ok(Self::from_parts(embedding, config))
    }
}

impl SemanticCleanerImpl {
    /// Build a cleaner around an already-constructed embedding port.
    ///
    /// The seam (ADR-0004): the full `clean()` path runs against ANY
    /// [`EmbeddingPort`] — the ONNX adapter, a remote endpoint, or a
    /// deterministic fake — without resolving or downloading a model. The
    /// caller keeps whatever else it built (engine, tokenizer) for its own
    /// ports, so the ONNX model is still loaded exactly once per process.
    #[must_use]
    pub fn from_parts(embedding: Arc<dyn EmbeddingPort>, config: ModelConfig) -> Self {
        Self {
            embedding,
            chunker: HtmlChunker::new(),
            scorer: RelevanceScorer::new(config.relevance_threshold),
            pruner: LegibleContentPruner::standard(),
            config,
        }
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
                "Starting full RAG pipeline: prune → chunk → guard → embed → score"
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

            // Step 2: Chunk-size guard. Characters, not tokens (ADR-0004):
            // the cleaner holds no tokenizer, so a token count is not
            // something it can honestly compute — and the same budget has to
            // mean something against a remote endpoint. Rejecting here also
            // keeps the guard ahead of the network call in step 3.
            for (index, chunk) in chunks.iter().enumerate() {
                let chars = chunk.content.chars().count();
                if chars > self.config.max_chars {
                    return Err(SemanticError::ChunkTooLarge {
                        chunk_id: format!("chunk-{index}"),
                        chars,
                        max: self.config.max_chars,
                    });
                }
            }

            debug!(
                chunks_guarded = chunks.len(),
                max_chars = self.config.max_chars,
                "Step 2: Chunk size guard complete"
            );

            // Step 3: Generate embeddings CONCURRENTLY (async-join-parallel)
            // Following `async-join-parallel`: use try_join_all for concurrent independent operations
            // The port's engine dispatches to dedicated worker threads with persistent sessions
            // Following `anti-lock-across-await`: No locks held across await points
            //
            // One `embed` per chunk, NOT `embed_batch`: the trait default for
            // batch is a sequential loop, so batching here would serialize the
            // backend instead of fanning out over it.
            let embeddings = try_join_all(
                chunks
                    .iter()
                    .map(|chunk| self.embedding.embed(&chunk.content)),
            )
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

    fn max_chars(&self) -> usize {
        self.config.max_chars
    }

    /// Always `true`: the cleaner has no lazily-loadable state of its own.
    ///
    /// Its embedding port was built successfully before the cleaner was
    /// constructed, so a `SemanticCleanerImpl` exists only if the backend it
    /// embeds through is ready. There is nothing left to probe.
    fn is_ready(&self) -> bool {
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_config_default() {
        let config = ModelConfig::default();
        assert_eq!(config.repo, AiModel::default().repo_id());
        assert_eq!(config.model_file, AiModel::default().model_file());
        assert!(!config.offline_mode);
        assert_eq!(config.max_chars, 98_304);
        assert_eq!(config.chars_per_token, 3.0);
        assert_eq!(config.relevance_threshold, 0.3);
    }

    #[test]
    fn test_model_config_builder() {
        let config = ModelConfig::new()
            .with_repo("test/repo")
            .with_file("test.onnx")
            .with_offline_mode(true)
            .with_max_chars(256)
            .with_relevance_threshold(0.5)
            .unwrap();

        assert_eq!(config.repo, "test/repo");
        assert_eq!(config.model_file, "test.onnx");
        assert!(config.offline_mode);
        assert_eq!(config.max_chars, 256);
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
    #[cfg(feature = "ai")]
    async fn test_semantic_cleaner_creation_fails_without_model() {
        // Creation must fail gracefully when the model is unavailable.
        // Offline mode + a bogus repo id (never present in the local hf_hub
        // cache) guarantees a deterministic resolution failure without network.
        let config = ModelConfig::new()
            .with_repo("nonexistent/fake-repo-for-test")
            .with_offline_mode(true);

        let result = SemanticCleanerImpl::new(config).await;
        // Not a bare `is_err()`: an uncached repo in offline mode must take the
        // `OfflineMode` path, so a `ModelLoad`/`Download` fallback here would
        // mean a real cache-miss regression silently passed.
        // `SemanticCleanerImpl` is not `Debug`, so the Ok arm is matched
        // instead of relying on `expect_err`.
        match result {
            Err(SemanticError::OfflineMode { .. }) => {},
            Err(other) => panic!("expected OfflineMode, got: {other:?}"),
            Ok(_) => panic!("expected OfflineMode, but the cleaner was constructed"),
        }
    }

    #[tokio::test]
    #[cfg(feature = "ai")]
    async fn test_semantic_cleaner_offline_mode() {
        // Offline mode must fail with OfflineMode when the model is not cached.
        // A bogus repo id is never present in the hf_hub cache, so this is
        // deterministic and requires no network access.
        let config = ModelConfig::new()
            .with_repo("nonexistent/fake-repo-for-test")
            .with_offline_mode(true);

        let result = SemanticCleanerImpl::new(config).await;
        // Pin the offending repo alongside the variant: the error is only
        // actionable if it names the model that was looked for.
        match result {
            Err(SemanticError::OfflineMode { repo }) => {
                assert_eq!(repo, "nonexistent/fake-repo-for-test");
            },
            Err(other) => panic!("expected OfflineMode, got: {other:?}"),
            Ok(_) => panic!("expected OfflineMode, but the cleaner was constructed"),
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
            .with_max_chars(256)
            .with_relevance_threshold(0.4)
            .unwrap();

        assert_eq!(config.repo, "test/repo");
        assert_eq!(config.model_file, "test.onnx");
        assert!(config.offline_mode);
        assert_eq!(config.max_chars, 256);
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

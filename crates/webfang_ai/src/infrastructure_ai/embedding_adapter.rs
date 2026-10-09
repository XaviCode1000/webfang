//! Embedding adapter — bridges an erased [`InferenceEngine`] to the domain [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort).
//!
//! This is the concrete infrastructure implementation of the always-compiled
//! domain port [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort). It wraps an erased ONNX [`InferenceEngine`] (`Arc<dyn
//! InferenceEngine + Send + Sync>` — the single-session [`InferencePool`] or
//! the N-session
//! [`PooledInferenceEngine`](crate::infrastructure_ai::inference_engine::PooledInferenceEngine)
//! alike, #1569) and the
//! HuggingFace [`MiniLmTokenizer`] to turn raw text into fixed-dimension
//! embedding vectors for IBM Granite embedding models (Granite-97M / Granite-311M,
//! unified 384d output), following the Adapter pattern (infrastructure adapts the
//! domain port to the ONNX primitives). Note: `MiniLmTokenizer` is legacy naming
//! from the pre-Granite era; an API rename is tracked as a follow-up refactor.
//!
//! # Architecture
//!
//! ```text
//! &str / &[String]
//!     ↓
//! [MiniLmTokenizer] text → ModelInput (token ids + masks)   // legacy name; see note above
//!     ↓
//! erased [InferenceEngine] ModelInput → Vec<f32>
//!     ↓
//! Vec<f32> / Vec<Vec<f32>>
//! ```
//!
//! # Design decisions
//!
//! - **Batch override**: [`EmbeddingPort::embed_batch`](webfang_core::domain::embedding_port::EmbeddingPort::embed_batch) defaults to calling
//!   [`EmbeddingPort::embed`](webfang_core::domain::embedding_port::EmbeddingPort::embed) per text, re-tokenizing through the port surface
//!   each time. This adapter overrides it to tokenize the whole batch in one
//!   [`MiniLmTokenizer::tokenize_batch`] call, then dispatches the resulting
//!   [`ModelInput`]s to the pool. `InferencePool` has no true batched inference
//!   (each `infer` is one worker request), so the dispatch is sequential — the
//!   win is avoiding per-call re-tokenization overhead, not parallel ONNX.
//! - **Span attachment**: the port methods are synchronous fns returning a
//!   `BoxFuture`, so `#[instrument]` would only span the (trivial) future
//!   construction. Per the observability contract, the span is attached to the
//!   future itself via [`Instrument::instrument`](tracing::Instrument::instrument) so it covers the actual
//!   tokenize + infer work.
//! - **Erased engine (#1569)**: the adapter stores
//!   `Arc<dyn InferenceEngine + Send + Sync>` instead of the concrete
//!   [`InferencePool`], so a Pool-mode cleaner shares its N-session engine with
//!   vault search through one model load — the `Single` path compiles unchanged
//!   via unsized coercion.
//! - **Shared resolution**: `EmbeddingAdapter::from_config` and
//!   `build_onnx_embedding_port` both call `resolve_model_assets`, the same
//!   hf_hub cache/download + SHA256 validation path, so every local-ONNX
//!   composition root resolves models identically. (Written as a code span,
//!   not a link: it is `pub(crate)`, so rustdoc does not document it by
//!   default and a link to it would be a permanently unresolved reference.)
//!
//! # Why model resolution lives here (ADR-0004, slice C)
//!
//! `resolve_model_assets` and `stream_validate_model_hash` used to live in
//! `semantic_cleaner_impl`. That module is now UNGATED — the cleaner itself
//! has no ONNX dependency — so it can no longer own the resolver: keeping
//! the hf_hub download path there would have re-gated the whole cleaner
//! behind `hf_hub` for no reason. This module is the natural home because it
//! is where the local-ONNX path lives, it already imports `hf_hub`, and it is
//! the only caller: `build_onnx_embedding_port` plus
//! [`EmbeddingAdapter::from_config`].
//!
//! # Rust-Skills Applied
//!
//! - `own-arc-shared`: erased `Arc<dyn InferenceEngine + Send + Sync>` /
//!   `Arc<MiniLmTokenizer>` shared ownership
//! - `async-clone-before-await`: references captured before await points
//! - `err-question-mark`: `?` propagation, no `.unwrap()` in production
//! - `obs-instrument-spans`: spans attached to the boxed futures
//! - `mem-with-capacity`: batch result pre-allocation

use std::future::Future;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use futures::future::try_join;
use hf_hub::api::tokio::ApiBuilder;
use hf_hub::{Cache as HfCache, Repo, RepoType};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tracing::{debug, info, Instrument};

use crate::infrastructure_ai::inference_engine::{InferenceEngine, InferencePool};
use crate::infrastructure_ai::semantic_cleaner_impl::ModelConfig;
use crate::infrastructure_ai::tokenizer::MiniLmTokenizer;
use crate::infrastructure_ai::{build_engine, EngineConfig};
use webfang_core::domain::embedding_port::EmbeddingPort;
use webfang_core::error::SemanticError;

/// A boxed future for dyn-compatible async trait methods.
///
/// Mirrors the private alias in [`webfang_core::domain::embedding_port`]; the
/// alias there is module-private, so the adapter declares its own.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The local embedding stack, built once: the erased domain port the cleaner
/// consumes, plus the erased engine and tokenizer the vault-search port and
/// the Tier 2 inspector still need.
pub type OnnxEmbeddingStack = (
    Arc<dyn EmbeddingPort>,
    Arc<dyn InferenceEngine + Send + Sync>,
    Arc<MiniLmTokenizer>,
);

/// Build the local ONNX embedding port AND the engine + tokenizer behind it.
///
/// This is the one composition root for the local embedding stack (ADR-0004):
/// it resolves and validates the model assets once, builds the selected
/// engine, loads the tokenizer, and wraps all three in an
/// [`EmbeddingAdapter`] erased to the domain [`EmbeddingPort`].
///
/// The three-tuple exists so every caller keeps what it needs without asking
/// the cleaner for it: the semantic cleaner takes only the port
/// ([`SemanticCleanerImpl::from_parts`](crate::infrastructure_ai::SemanticCleanerImpl::from_parts)),
/// while the vault-search embedding port and the Tier 2 DOM inspector still
/// want the erased engine and the tokenizer directly. Handing those out here
/// is what keeps the `--ai` path at ONE model load per process.
///
/// `EngineConfig::Single` behaves like the historical single-session pool
/// (graceful degradation included); `Pool { size }` opens `size` sessions.
///
/// # Errors
///
/// Returns [`SemanticError`] when model resolution or download fails, the
/// SHA256 integrity check fails, the tokenizer cannot be loaded, or the engine
/// cannot be built (a pool session that cannot be built fails fast instead of
/// degrading).
#[tracing::instrument(skip(config), fields(repo = %config.repo, offline_mode = config.offline_mode, engine = ?engine_config))]
pub async fn build_onnx_embedding_port(
    config: &ModelConfig,
    engine_config: EngineConfig,
) -> Result<OnnxEmbeddingStack, SemanticError> {
    let (model_path, tokenizer_path) = resolve_model_assets(config).await?;
    let tokenizer = Arc::new(MiniLmTokenizer::from_file(&tokenizer_path).await?);
    let engine = build_engine(&engine_config, model_path, config.model_variant)?;
    debug!(
        dim = engine.embedding_dim(),
        "ONNX embedding port built (engine, tokenizer and port share one model)"
    );
    let port: Arc<dyn EmbeddingPort> = Arc::new(EmbeddingAdapter::new(
        Arc::clone(&engine),
        Arc::clone(&tokenizer),
    ));
    Ok((port, engine, tokenizer))
}

/// Adapter exposing an erased [`InferenceEngine`] through the domain [`EmbeddingPort`].
///
/// Cheap to clone-share via `Arc`: both fields are `Arc`-wrapped. The engine is
/// type-erased (`Arc<dyn InferenceEngine + Send + Sync>`, #1569) so the SAME
/// engine instance backs the semantic cleaner, vault-search embeddings and the
/// Tier 2 inspector regardless of whether it is the single-session
/// [`InferencePool`] or the N-session
/// [`PooledInferenceEngine`](crate::infrastructure_ai::inference_engine::PooledInferenceEngine).
/// Concrete
/// `Arc<InferencePool>` arguments coerce into the erased field at the
/// [`EmbeddingAdapter::new`] call site, so the Single-path constructors keep
/// compiling unchanged. Constructed either directly from existing components
/// ([`EmbeddingAdapter::new`]) or from a [`ModelConfig`] that resolves and
/// validates the model + tokenizer ([`EmbeddingAdapter::from_config`]).
///
/// # Thread Safety
///
/// `Send + Sync` — erased `dyn InferenceEngine + Send + Sync` and
/// `MiniLmTokenizer` are `Send + Sync`, so the adapter can be shared as
/// `Arc<dyn EmbeddingPort>` across the MCP server and any future consumer.
pub struct EmbeddingAdapter {
    /// Erased ONNX inference engine (single-session pool or N-session pool).
    pool: Arc<dyn InferenceEngine + Send + Sync>,
    /// HuggingFace tokenizer (text → `ModelInput`).
    tokenizer: Arc<MiniLmTokenizer>,
}

impl EmbeddingAdapter {
    /// Wrap an existing inference engine and tokenizer.
    ///
    /// The engine is accepted erased; a concrete `Arc<InferencePool>` (the
    /// Single path) coerces implicitly. Use this when the caller already holds
    /// the components; otherwise prefer
    /// [`build_onnx_embedding_port`] (which also resolves the model) or
    /// [`from_config`](Self::from_config).
    #[must_use]
    pub fn new(
        pool: Arc<dyn InferenceEngine + Send + Sync>,
        tokenizer: Arc<MiniLmTokenizer>,
    ) -> Self {
        Self { pool, tokenizer }
    }

    /// Resolve the model + tokenizer from `config` and build the adapter.
    ///
    /// Mirrors `SemanticCleanerImpl::new`: resolves the model and tokenizer
    /// through the hf_hub cache (cache-first online, strict offline), validates
    /// the model SHA256 by streaming the file on disk, then loads the
    /// tokenizer and builds the inference pool.
    ///
    /// Callers: the semantic cleaner's own
    /// [`SemanticCleanerImpl::new`](crate::infrastructure_ai::SemanticCleanerImpl::new)
    /// constructor (the Single path), plus the offline-failure test below. The
    /// `Pool`-capable composition root is [`build_onnx_embedding_port`].
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError`] when model resolution, download, SHA256
    /// validation, tokenizer loading, or pool construction fails.
    #[tracing::instrument(skip(config), fields(repo = %config.repo, offline_mode = config.offline_mode))]
    pub async fn from_config(config: &ModelConfig) -> Result<Self, SemanticError> {
        let (model_path, tokenizer_path) = resolve_model_assets(config).await?;
        let tokenizer = Arc::new(MiniLmTokenizer::from_file(&tokenizer_path).await?);
        let pool: Arc<dyn InferenceEngine + Send + Sync> =
            Arc::new(InferencePool::new(model_path, config.model_variant)?);
        debug!(dim = pool.embedding_dim(), "EmbeddingAdapter initialized");
        Ok(Self { pool, tokenizer })
    }
}

impl EmbeddingPort for EmbeddingAdapter {
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
        let span = tracing::debug_span!("embed", text_len = text.len(), dim = self.embedding_dim());
        Box::pin(
            async move {
                let input = self.tokenizer.tokenize(text)?;
                self.pool.infer(&input).await
            }
            .instrument(span),
        )
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
        let span = tracing::debug_span!(
            "embed_batch",
            count = texts.len(),
            dim = self.embedding_dim()
        );
        Box::pin(
            async move {
                if texts.is_empty() {
                    return Ok(Vec::new());
                }
                // Tokenize the whole batch once (avoids per-call re-tokenization),
                // then dispatch each ModelInput to the pool sequentially — the pool
                // has no batched inference, each infer is a single worker request.
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                let batch = self.tokenizer.tokenize_batch(&refs)?;
                let inputs = batch.to_model_inputs();
                let mut results = Vec::with_capacity(inputs.len());
                for input in &inputs {
                    results.push(self.pool.infer(input).await?);
                }
                Ok(results)
            }
            .instrument(span),
        )
    }

    fn embedding_dim(&self) -> usize {
        self.pool.embedding_dim()
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
/// Moved here from `semantic_cleaner_impl` when the cleaner was ungated
/// (ADR-0004, slice C): that module no longer imports hf_hub, and this is the
/// local-ONNX path's own resolver. Both callers —
/// `build_onnx_embedding_port` and `EmbeddingAdapter::from_config` — are
/// `ai`-gated, which is exactly the scope of the hf_hub dependency it needs.
///
/// # Returns
///
/// `(model_path, tokenizer_path)` — SHA256-validated model bytes and the path
/// to `tokenizer.json`.
///
/// # Errors
///
/// Returns [`SemanticError::OfflineMode`] when offline and an asset is uncached,
/// [`SemanticError::Download`] on hf_hub client/API failure,
/// [`SemanticError::ModelLoad`] when the model file cannot be opened for
/// validation, or [`SemanticError::CacheValidation`] on SHA256 mismatch.
#[tracing::instrument(skip(config), fields(repo = %config.repo, model_file = %config.model_file, offline_mode = config.offline_mode))]
pub(crate) async fn resolve_model_assets(
    config: &ModelConfig,
) -> Result<(PathBuf, PathBuf), SemanticError> {
    // #1316: name the model-resolve operation up front so a slow (cold)
    // download is attributable in the trace file instead of looking like a
    // hang, and time the whole resolution for the summary event below.
    let started = std::time::Instant::now();
    info!(
        repo = %config.repo,
        offline_mode = config.offline_mode,
        "resolving AI model assets"
    );

    // Resolve model + tokenizer paths through the hf_hub cache.
    let (model_path, tokenizer_path, cached) = if config.offline_mode {
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
        (model_path, tokenizer_path, true)
    } else {
        // #1316: cache-only probe (pure fs lookup, no network) BEFORE touching
        // the API, so the cold-download hint can fire before the pull starts.
        let cache = HfCache::from_env();
        let probe = cache.repo(Repo::new(config.repo.clone(), RepoType::Model));
        let cached =
            probe.get(&config.model_file).is_some() && probe.get("tokenizer.json").is_some();

        // #1316: when stderr is piped, hf_hub's indicatif progress bar renders
        // nothing and a multi-minute cold pull looks like a hang. A plain
        // eprintln! reaches the user on the non-TTY path (a TTY already gets
        // the built-in progress bar). User-facing, so Spanish.
        if !cached && !std::io::stderr().is_terminal() {
            eprintln!(
                "Descargando modelo AI (~{} MB, primera vez); puede tardar varios minutos.",
                config.model_variant.approx_download_mb()
            );
        }

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
        (model_path, tokenizer_path, cached)
    };

    // Stream-validate the SHA256 of the model file on disk. The file itself
    // (not a byte copy) feeds the inference pool via `commit_from_file`, so
    // no application-side duplicate of the blob ever exists (#1315).
    stream_validate_model_hash(&model_path, config.model_variant.sha256(), &config.repo).await?;

    // #1316: structured summary for the long-running resolve — emitted for
    // both branches, visible in `--trace-file` JSONL regardless of TTY.
    let bytes = tokio::fs::metadata(&model_path)
        .await
        .map_err(SemanticError::ModelLoad)?
        .len();
    info!(
        repo = %config.repo,
        bytes,
        elapsed_ms = started.elapsed().as_millis() as u64,
        cached,
        "AI model assets resolved"
    );

    Ok((model_path, tokenizer_path))
}

/// Stream-validate the SHA256 of the model file on disk (constant memory:
/// 1 MiB chunks — the integrity check itself never pulls the ~1.2 GB 311m
/// blob into the process).
///
/// Streaming computes the actual digest without ever buffering the whole
/// file, so the "buffer in RAM if the hash fails" alternative is not needed:
/// a mismatch simply yields the computed digest in the
/// [`SemanticError::CacheValidation`] payload.
///
/// # Errors
///
/// Returns [`SemanticError::ModelLoad`] when the file cannot be opened or
/// read, and [`SemanticError::CacheValidation`] when the computed hash does
/// not match `expected`.
#[tracing::instrument(skip(model_path), fields(repo = %repo, expected = %expected))]
async fn stream_validate_model_hash(
    model_path: &Path,
    expected: &str,
    repo: &str,
) -> Result<(), SemanticError> {
    debug!("Validating model integrity (streaming)...");
    let mut file = tokio::fs::File::open(model_path)
        .await
        .map_err(SemanticError::ModelLoad)?;

    const CHUNK_BYTES: usize = 1024 * 1024; // 1 MiB
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut hasher = Sha256::new();
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(SemanticError::ModelLoad)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(SemanticError::CacheValidation {
            repo: repo.to_string(),
            expected: expected.to_string(),
            actual,
        });
    }
    debug!(sha = %actual, "SHA256 validation passed (streamed)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure_ai::ai_test_fixture::in_memory_wordpiece_tokenizer;
    use crate::infrastructure_ai::inference_engine::MockInferenceEngine;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    /// Adapter over a fixed-latency mock engine and an in-memory tokenizer — no
    /// ONNX model, no worker threads, fully deterministic.
    ///
    /// The mock reports the same 384-dim output as `AiModel::Granite97M`, which
    /// is all these tests assert. #1575 switched this fixture off the
    /// unloadable-model `InferencePool` it used to build: the pool spawns a
    /// worker thread per instance purely so a test can read a dimension, and
    /// that setup was a byte-for-byte copy of the one in
    /// `tests/erased_engine_ports_test.rs`. The pool's own configured dimension
    /// stays covered by the `inference_engine` unit tests, and the concrete-pool
    /// path through this adapter stays covered by the integration suite, which
    /// needs the concrete type to prove unsized coercion.
    fn fake_adapter() -> EmbeddingAdapter {
        let engine: Arc<dyn InferenceEngine + Send + Sync> = Arc::new(MockInferenceEngine::new(
            std::time::Duration::from_millis(1),
        ));
        let tokenizer = Arc::new(MiniLmTokenizer::new(in_memory_wordpiece_tokenizer(), 512));
        EmbeddingAdapter::new(engine, tokenizer)
    }

    #[test]
    fn test_embedding_adapter_is_send_sync() {
        assert_send::<EmbeddingAdapter>();
        assert_sync::<EmbeddingAdapter>();
    }

    #[test]
    fn test_embedding_dim_returns_384() {
        let adapter = fake_adapter();
        assert_eq!(
            adapter.embedding_dim(),
            384,
            "the engine's Granite-97M output dim must report 384d"
        );
    }

    #[test]
    fn test_adapter_coerces_to_dyn_embedding_port() {
        // Object-safety proof with a real instance: the adapter must coerce to
        // the erased domain port and delegate embedding_dim through the vtable.
        let adapter = fake_adapter();
        let port: &dyn EmbeddingPort = &adapter;
        assert_eq!(port.embedding_dim(), 384);
    }

    #[tokio::test]
    async fn test_from_config_fails_offline_without_cache() {
        // Offline mode + a bogus repo id (never present in the hf_hub cache)
        // guarantees a deterministic resolution failure without network access,
        // mirroring the SemanticCleanerImpl::new offline tests.
        let config = ModelConfig::new()
            .with_repo("nonexistent/fake-repo-for-test")
            .with_offline_mode(true);
        let result = EmbeddingAdapter::from_config(&config).await;
        assert!(
            result.is_err(),
            "offline resolution of an uncached model must fail"
        );
    }

    // -----------------------------------------------------------------------
    // `stream_validate_model_hash` coverage.
    //
    // These three travelled here verbatim from
    // `semantic_cleaner_impl::tests` when the resolver moved (ADR-0004,
    // slice C). They were NOT reformulated or dropped: the function they
    // cover is still the one the ONNX path calls, and its own unit home is
    // now the file that owns it. `SemanticCleanerImpl::new` — whose tests
    // still exercise the same offline-failure path end to end — is
    // `ai`-gated alongside the resolver, so those two stay put under
    // `#[cfg(feature = "ai")]`.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_stream_validate_model_hash_mismatch_returns_cache_validation() {
        // Exercises the REAL streaming validation path: known file content
        // plus a WRONG expected hash must yield CacheValidation carrying both
        // hashes + repo, with the actual digest computed from disk in chunks.
        let dir = tempfile::tempdir().expect("create temp dir for hash test");
        let model_path = dir.path().join("model.onnx");
        tokio::fs::write(&model_path, b"webfang deterministic test payload")
            .await
            .expect("write temp model file");
        let wrong_expected = "0000000000000000000000000000000000000000000000000000000000000000";

        let result = stream_validate_model_hash(&model_path, wrong_expected, "test/repo").await;

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
                assert_eq!(actual.len(), 64, "SHA256 digest must be 64 hex chars");
            },
            other => panic!("expected CacheValidation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_stream_validate_model_hash_match_passes_across_chunk_boundary() {
        // Success path: feeding back the real SHA256 must validate cleanly.
        // The payload spans several 1 MiB chunks (plus a non-multiple tail)
        // to prove the chunk loop reassembles the full digest and the final
        // partial chunk is not dropped.
        let dir = tempfile::tempdir().expect("create temp dir for hash test");
        let model_path = dir.path().join("model.onnx");
        // 2 MiB + 37 bytes: forces two full chunks and one partial one.
        let payload: Vec<u8> = (0..2 * 1024 * 1024 + 37).map(|i| (i % 251) as u8).collect();
        tokio::fs::write(&model_path, &payload)
            .await
            .expect("write temp model file");
        let real_hash = format!("{:x}", Sha256::digest(&payload));

        assert!(
            stream_validate_model_hash(&model_path, &real_hash, "test/repo")
                .await
                .is_ok(),
            "streamed digest across chunk boundaries must match the single-shot digest"
        );
    }

    #[tokio::test]
    async fn test_stream_validate_model_hash_missing_file_returns_model_load() {
        // Opening a nonexistent path must surface as ModelLoad (io::Error),
        // NOT as a hash mismatch or a panic.
        let dir = tempfile::tempdir().expect("create temp dir for hash test");
        let missing = dir.path().join("does-not-exist.onnx");

        let result = stream_validate_model_hash(&missing, "00", "test/repo").await;

        match result {
            Err(SemanticError::ModelLoad(_)) => {
                // Expected
            },
            other => panic!("expected ModelLoad, got {other:?}"),
        }
    }
}

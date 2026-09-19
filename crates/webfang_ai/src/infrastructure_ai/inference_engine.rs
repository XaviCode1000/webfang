//! Inference engine — ONNX model execution with ort (ONNX Runtime)
//!
//! Handles loading and executing ONNX models for sentence embedding generation:
//! - Session built ONCE from the model FILE via `commit_from_file` — the
//!   application never holds model bytes; ORT keeps the single copy of the
//!   weights in its session representation instead of app buffer + ORT copy,
//!   halving the ~1.2 GB Granite-311M footprint (#1315: 3.2 → 2.05 GiB measured)
//! - Async inference via `spawn_blocking` (`async-spawn-blocking`)
//! - Clone Arc before await (`async-clone-before-await`)
//! - 384-dimensional embedding output for IBM Granite models
//! - **2 required ONNX inputs**: `input_ids` and `attention_mask`
//! - **`token_type_ids` is OPTIONAL**: only sent when the model graph declares it
//!   (ModernBert/Granite never declare it). The input set is resolved from the
//!   graph at worker startup, not hardcoded (#543).
//!
//! # Design Decisions
//!
//! - **One shared session**: the pool builds a single `ort::Session` before spawning
//!   workers and shares it as `Arc<Mutex<Session>>`; that Arc is the ONLY shared
//!   model state. `Session::run` takes `&mut self`
//!   in ort 2.0, so the `Mutex` is required — it costs nothing because the request
//!   channel already serializes work per worker. Building one session per worker
//!   duplicated the whole model graph in RSS on every CPU core (#648).
//! - **384-dim invariant**: Granite-97M is natively 384d; Granite-311M uses Matryoshka
//!   truncation to 384d. No runtime dimension discovery needed.
//! - **spawn_blocking**: CPU-intensive ONNX inference runs in blocking pool to avoid
//!   starving async runtime.
//! - **No locks across await**: Clone Arc before async operations.

use std::future::Future;
use std::num::NonZeroUsize;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use ort::session::{builder::GraphOptimizationLevel, Session};
use tracing::{debug, instrument};

use crate::infrastructure_ai::cache_config::AiModel;
use webfang_core::error::SemanticError;

/// Input data for ONNX model inference
///
/// The Granite/ModernBert embedding models require 2 input tensors:
/// 1. `input_ids` - Token IDs (vocab indices)
/// 2. `attention_mask` - Which tokens are real (1) vs padding (0)
///
/// `token_type_ids` is OPTIONAL and is only sent when the model graph declares
/// it (forward-compat). All vectors must have the same length (sequence length).
/// See [`InputPlan`] for how the actual input set is resolved from the graph.
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

// ---------------------------------------------------------------------------
// InputPlan: resolve the real input set of the ONNX graph once per worker
// ---------------------------------------------------------------------------

/// Resolved ONNX input plan: the subset of inputs the model graph actually
/// declares, in graph declaration order.
///
/// The Granite/ModernBert models only require `input_ids` and `attention_mask`.
/// `token_type_ids` is OPTIONAL and is only fed when the graph declares it.
/// Resolving the plan once at worker startup (instead of using a hardcoded
/// 3-input assumption) is what fixes the `Invalid input name: token_type_ids`
/// failure (#543): we never send an input the graph does not declare.
#[derive(Debug, Clone)]
pub struct InputPlan {
    /// Owned input names in graph declaration order.
    names: Vec<String>,
}

impl InputPlan {
    /// Resolve a plan from the raw input names declared by the model graph.
    ///
    /// # Errors
    ///
    /// Returns `SemanticError::Inference` if a required input (`input_ids` or
    /// `attention_mask`) is missing, or if the graph declares an unsupported
    /// input name (anything other than the three recognized tensors).
    pub fn resolve(names: &[&str]) -> Result<Self, SemanticError> {
        const REQUIRED: [&str; 2] = ["input_ids", "attention_mask"];
        const KNOWN: [&str; 3] = ["input_ids", "attention_mask", "token_type_ids"];

        let present: std::collections::HashSet<&str> = names.iter().copied().collect();
        let missing: Vec<&str> = REQUIRED
            .iter()
            .copied()
            .filter(|r| !present.contains(r))
            .collect();
        if !missing.is_empty() {
            return Err(SemanticError::Inference(format!(
                "missing required model inputs: {}",
                missing.join(", ")
            )));
        }

        for n in names {
            if !KNOWN.contains(n) {
                return Err(SemanticError::Inference(format!("unsupported input: {n}")));
            }
        }

        Ok(Self {
            names: names.iter().map(|n| n.to_string()).collect(),
        })
    }

    /// Build a plan by introspecting a built `ort::Session`.
    ///
    /// # Errors
    ///
    /// Propagates [`InputPlan::resolve`] errors when the graph's inputs do not
    /// satisfy the required/known contract.
    pub fn from_session(session: &Session) -> Result<Self, SemanticError> {
        let owned: Vec<String> = session
            .inputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
        Self::resolve(&borrowed)
    }

    /// Input names in graph declaration order.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

// ---------------------------------------------------------------------------
// InferenceEngine: minimal seam so `SemanticCleanerImpl` can run against the
// real `InferencePool` or the fixed-latency mock (P0-001 paso 0, issue #1456).
// ---------------------------------------------------------------------------

/// Minimal inference seam over one tokenized chunk.
///
/// The production [`InferencePool`] implements this; the paso-0 verification
/// [`MockInferenceEngine`] implements it with a fixed sleep and no real mutex.
/// Object-safe by construction: the async method is spelled as a boxed future
/// (the same pattern as `SemanticCleaner::clean`), because native `async fn`
/// in traits is not dyn-compatible.
///
/// `Send + Sync` is a supertrait bound so engines can be shared as
/// `Arc<E>` / `Arc<dyn InferenceEngine + Send + Sync>` across Tokio workers.
pub trait InferenceEngine: Send + Sync {
    /// Run inference for one tokenized chunk.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Inference`]
    /// when the engine cannot serve the request.
    fn infer<'a>(
        &'a self,
        input: &'a ModelInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SemanticError>> + Send + 'a>>;

    /// Native embedding dimension (384 for all Granite variants).
    fn embedding_dim(&self) -> usize;

    /// Whether the engine can serve inference right now.
    fn is_ready(&self) -> bool;
}

impl<T> InferenceEngine for Arc<T>
where
    T: InferenceEngine + ?Sized,
{
    /// Forward through the `Arc` so erased engines (`Arc<dyn InferenceEngine + Send + Sync>`)
    /// satisfy the same generic seam (`SemanticCleanerImpl<E>`) as concrete ones.
    fn infer<'a>(
        &'a self,
        input: &'a ModelInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SemanticError>> + Send + 'a>> {
        Box::pin(async move { self.as_ref().infer(input).await })
    }

    fn embedding_dim(&self) -> usize {
        self.as_ref().embedding_dim()
    }

    fn is_ready(&self) -> bool {
        self.as_ref().is_ready()
    }
}

// ---------------------------------------------------------------------------
// InferencePool: dedicated worker threads with persistent sessions
// ---------------------------------------------------------------------------

use std::thread;

use tokio::sync::{mpsc, oneshot, Semaphore};
use tracing::{error, info};

/// Internal: a single inference request dispatched to a worker thread.
struct WorkerRequest {
    input: ModelInput,
    reply_tx: oneshot::Sender<Result<Vec<f32>, SemanticError>>,
}

/// One `ort::Session` shared by every worker thread.
///
/// `Session::run` requires `&mut self` in ort 2.0, so the session is guarded by
/// a `Mutex`. Contention is irrelevant: the request channel already serializes
/// the work each worker performs.
type SharedSession = Arc<Mutex<Session>>;

/// Shared request receiver. `tokio::sync::mpsc::Receiver` is `!Sync`, so the
/// worker threads take turns holding it behind a `std::sync::Mutex` and call
/// `blocking_recv` — they are plain OS threads, never the Tokio reactor. The
/// mutex is only held for the receive itself (which returns immediately when
/// a message is buffered), so inference parallelism is unaffected.
type SharedReceiver = Arc<Mutex<mpsc::Receiver<WorkerRequest>>>;

/// Pool of dedicated worker threads for ONNX inference.
///
/// All workers share ONE persistent `ort::Session` built with `intra_threads(1)`
/// and guarded by a `Mutex`. Requests are dispatched via a bounded
/// `tokio::sync::mpsc` channel (#1133 — the async `send` applies backpressure
/// by yielding the task to the reactor instead of parking a Tokio worker on a
/// synchronous crossbeam send); results return through per-request tokio
/// oneshot channels.
///
/// # Thread Safety
///
/// - `Send + Sync`: tokio `Sender` and `JoinHandle` are both Send+Sync
/// - **Not `Clone`** (#1131): a cloned sender would keep the request channel
///   open after the owner drops, so the owner's `Drop` would join forever and
///   the ONNX `Session` plus the `inference-worker-*` threads would leak.
///   Share the pool as `Arc<InferencePool>` — the single owner's `Drop` then
///   always disconnects the channel.
/// - `Drop`: releases the shared session, disconnects the channel (workers
///   exit) and joins all threads
pub struct InferencePool {
    request_tx: mpsc::Sender<WorkerRequest>,
    _worker_handles: Vec<thread::JoinHandle<()>>,
    shared_session: Option<SharedSession>,
    model_variant: AiModel,
    worker_count: usize,
}

impl InferencePool {
    /// Create a new inference pool with dedicated worker threads.
    ///
    /// The `ort::Session` is built ONCE from `model_path` with
    /// `intra_threads(1)` and shared by every worker through
    /// `Arc<Mutex<Session>>`, so the model graph is resident in memory a
    /// single time instead of once per CPU core (#648). The session is
    /// committed via `commit_from_file`: the application never materializes
    /// model bytes, and ORT holds the only copy of the weights in its
    /// session representation (#1315). Spawns `(num_cpus - 1).max(1)` OS threads.
    ///
    /// When the session cannot be built (missing or invalid model file), the
    /// pool is still created: a drainer thread consumes pending requests so
    /// callers get a prompt error instead of blocking forever.
    ///
    /// # Errors
    ///
    /// Returns `SemanticError::Inference` if a thread fails to spawn.
    pub fn new(
        model_path: std::path::PathBuf,
        model_variant: AiModel,
    ) -> Result<Self, SemanticError> {
        // Canonical detector seam (Q2, via core dependency): process-wide "auto".
        let worker_count =
            (webfang_core::domain::budget::detector::system_parallelism().get() - 1).max(1);
        // #1133: bounded tokio mpsc — `infer` applies backpressure with an
        // awaitable `send`, so a full queue parks the TASK on the reactor,
        // never a Tokio worker thread on a synchronous crossbeam send.
        let (request_tx, receiver) = mpsc::channel::<WorkerRequest>(worker_count);
        let receiver: SharedReceiver = Arc::new(Mutex::new(receiver));

        let (shared_session, worker_handles) = match prepare_shared_session(&model_path) {
            Ok((session, plan)) => {
                let session: SharedSession = Arc::new(Mutex::new(session));
                let handles =
                    spawn_workers(&receiver, &session, &plan, model_variant, worker_count)?;
                (Some(session), handles)
            },
            Err(e) => {
                error!(error = %e, "Failed to initialize shared ONNX session");
                (None, spawn_drainer(&receiver)?)
            },
        };

        info!(worker_count, ?model_variant, "InferencePool created");

        Ok(Self {
            request_tx,
            _worker_handles: worker_handles,
            shared_session,
            model_variant,
            worker_count,
        })
    }

    /// Run inference asynchronously by dispatching to a worker thread.
    ///
    /// Thin inherent wrapper over the [`InferenceEngine`] implementation, kept
    /// so existing call sites (`pool.infer(..)`) resolve unchanged: inherent
    /// methods take precedence over trait methods with the same name.
    ///
    /// Sends the request via the bounded channel — under backpressure the
    /// `send` is an await point that yields the task to the reactor (#1133),
    /// so `tokio::time::timeout`/cancellation keep working while the queue is
    /// full — then awaits the oneshot result.
    ///
    /// # Errors
    ///
    /// Returns `SemanticError::Inference` if the channel is closed or the
    /// worker drops the response.
    pub async fn infer(&self, input: &ModelInput) -> Result<Vec<f32>, SemanticError> {
        <Self as InferenceEngine>::infer(self, input).await
    }

    /// Get embedding dimension (384 for all Granite models)
    #[must_use]
    pub fn embedding_dim(&self) -> usize {
        self.model_variant.output_dim()
    }

    /// Get the AI model variant loaded in this pool
    #[must_use]
    pub fn model_variant(&self) -> AiModel {
        self.model_variant
    }

    /// Get the number of worker threads in the pool
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    /// Check if pool is ready for inference
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.worker_count > 0
    }
}

impl InferenceEngine for InferencePool {
    /// Dispatch one request to the worker threads (real ORT session behind
    /// the shared `Mutex`; see the `InferencePool` docs for why it exists).
    #[instrument(skip_all)]
    fn infer<'a>(
        &'a self,
        input: &'a ModelInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SemanticError>> + Send + 'a>> {
        Box::pin(async move {
            let (reply_tx, reply_rx) = oneshot::channel();
            let request = WorkerRequest {
                input: input.clone(),
                reply_tx,
            };

            // #1133: async send on a bounded tokio channel. When all workers are
            // busy the task waits cooperatively (cancellable) for capacity — the
            // executor thread is released, not parked on a blocking send.
            self.request_tx.send(request).await.map_err(|_| {
                SemanticError::Inference("InferencePool channel closed (all workers exited)".into())
            })?;

            // Await result asynchronously — yields to Tokio, no blocking
            reply_rx
                .await
                .map_err(|_| SemanticError::Inference("Worker dropped response channel".into()))?
        })
    }

    fn embedding_dim(&self) -> usize {
        InferencePool::embedding_dim(self)
    }

    fn is_ready(&self) -> bool {
        InferencePool::is_ready(self)
    }
}

// ---------------------------------------------------------------------------
// MockInferenceEngine: fixed-latency verification mock (P0-001 paso 0)
// ---------------------------------------------------------------------------

/// Fixed-latency mock engine for the P0-001 verification (issue #1456).
///
/// `infer` sleeps `fixed_latency` and returns a deterministic 384-dim
/// L2-normalized constant embedding. There is deliberately NO real `Mutex`:
/// concurrent `infer` calls overlap fully, so any residual serialization
/// observed through this mock is attributable to the fan-out/fan-in
/// plumbing (`SemanticCleanerImpl::clean`, `export_flow::clean_all_pages`),
/// not to ORT session contention.
///
/// Calibrate `fixed_latency` against the issue baseline: ~45ms reproduces
/// the measured ≈17.7s for 393 serial chunks (393 × 45ms ≈ 17.7s).
#[derive(Debug, Clone)]
pub struct MockInferenceEngine {
    /// Fixed sleep per `infer` call.
    fixed_latency: Duration,
}

impl MockInferenceEngine {
    /// Unified Granite output dimension shared by every mock embedding.
    const EMBEDDING_DIM: usize = 384;

    /// Create a mock engine that sleeps `fixed_latency` per chunk.
    #[must_use]
    pub fn new(fixed_latency: Duration) -> Self {
        debug!(?fixed_latency, "MockInferenceEngine created");
        Self { fixed_latency }
    }

    /// The fixed sleep applied per `infer` call.
    #[must_use]
    pub fn fixed_latency(&self) -> Duration {
        self.fixed_latency
    }

    /// Deterministic 384-dim L2-normalized constant embedding.
    ///
    /// Every chunk maps to the same unit vector (`1/sqrt(384)` per lane), so
    /// relevance filtering keeps all chunks and benchmark runs are bit-identical.
    #[must_use]
    pub fn deterministic_embedding() -> Vec<f32> {
        let lane = 1.0 / (Self::EMBEDDING_DIM as f32).sqrt();
        vec![lane; Self::EMBEDDING_DIM]
    }
}

impl InferenceEngine for MockInferenceEngine {
    fn infer<'a>(
        &'a self,
        _input: &'a ModelInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SemanticError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::time::sleep(self.fixed_latency).await;
            Ok(Self::deterministic_embedding())
        })
    }

    fn embedding_dim(&self) -> usize {
        Self::EMBEDDING_DIM
    }

    fn is_ready(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// N-session pool (P0-001 remediation, issue #1456).
//
// The single shared session behind `InferencePool` serializes every inference
// on one `Mutex<Session>` (speedup 1→8 = 1.02×). This section adds the
// remediation candidate: N independent `ort::Session`s behind the same
// [`InferenceEngine`] seam, with `intra_threads = (total_cores / N).max(1)`.
//
// Coordination has ZERO additional centralized contention: slot selection is
// a lock-free `AtomicUsize::fetch_add % N` start index plus `try_acquire`
// rotation over per-slot 1-permit semaphores. There is deliberately NO second
// centralized `Mutex<usize>` round-robin — that would reintroduce the
// eliminated pattern (maintainer comment is explicit).
// ---------------------------------------------------------------------------

/// Engine selection: single shared session (today's behavior) or N-session pool.
///
/// `Single` builds exactly one [`InferencePool`] via [`InferencePool::new`]
/// (byte-for-byte today's path: 1 session, `intra_threads(1)`, shared across
/// workers), so rollback is a one-variant change at the single call site that
/// picks this enum. `Pool { size }` builds [`PooledInferenceEngine`] with
/// `size` independent sessions and `intra_threads = (total_cores / size).max(1)`.
///
/// The MEASURE task (later) calibrates N with numbers and owns any flag UX;
/// this enum only names the already-decided configuration. Selection travels
/// exclusively through [`EngineConfig::from_env`] (`WEBFANG_AI_ENGINE`): no CLI
/// args by design — a flag could never reach the MCP daemon, which has no
/// per-run argv and already resolves `AI_MODEL_ID` from the environment (#874).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineConfig {
    /// Today's behavior: one shared session behind [`InferencePool`].
    #[default]
    Single,
    /// N independent sessions; `size` is the MEASURE-calibrated dial.
    Pool {
        /// Session count (the MEASURE-calibrated dial; explicit override).
        size: NonZeroUsize,
    },
}

impl EngineConfig {
    /// Today's behavior (rollback target).
    #[must_use]
    pub fn single() -> Self {
        Self::Single
    }

    /// `Pool` with an explicit session count (MEASURE override).
    #[must_use]
    pub fn pool(size: NonZeroUsize) -> Self {
        Self::Pool { size }
    }

    /// Explicit override when `Some`, otherwise the parallelism-derived default.
    #[must_use]
    pub fn pool_size_or_default(size: Option<NonZeroUsize>) -> NonZeroUsize {
        size.unwrap_or_else(Self::default_pool_size)
    }

    /// Default pool size derived from system parallelism (canonical detector
    /// seam): half the cores clamped to [2, 8]. A starting dial only — the
    /// MEASURE task calibrates N with numbers, never intuition.
    #[must_use]
    pub fn default_pool_size() -> NonZeroUsize {
        let cores = webfang_core::domain::budget::detector::system_parallelism().get();
        NonZeroUsize::new((cores / 2).clamp(2, 8)).unwrap_or(NonZeroUsize::MIN)
    }

    /// Split `total_cores` intra-op threads across `pool_size` sessions.
    ///
    /// `(total_cores / pool_size).max(1)`: the N > cores edge degrades to one
    /// thread per session instead of dividing by zero or idling sessions.
    #[must_use]
    pub fn split_intra_threads(total_cores: usize, pool_size: usize) -> usize {
        total_cores
            .checked_div(pool_size.max(1))
            .unwrap_or(1)
            .max(1)
    }

    /// Environment variable selecting the engine (`single` | `pool:<N>`).
    pub const ENV_VAR: &str = "WEBFANG_AI_ENGINE";

    /// Read the engine selection from [`Self::ENV_VAR`].
    ///
    /// Unset, empty, or whitespace-only means [`EngineConfig::Single`] (today's
    /// default — the user made no choice). A set-but-invalid value is a loud
    /// `Err` in Spanish: it must never silently fall back to `Single` (#874
    /// discipline: a poisoned env var fails startup instead of mismeasuring).
    /// Pure core in `Self::resolve_spec()` so tests stay race-free.
    pub fn from_env() -> Result<Self, String> {
        Self::resolve_spec(std::env::var(Self::ENV_VAR).ok().as_deref())
    }

    /// Pure core of [`Self::from_env`], taking the raw env value so tests stay
    /// race-free under parallel execution (no real env mutation).
    fn resolve_spec(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(Self::Single),
            Some(spec) => spec.parse(),
        }
    }
}

impl FromStr for EngineConfig {
    /// `Err` is a Spanish message naming the variable and the valid values.
    type Err = String;

    /// Parse `single` (case-insensitive, trimmed) or `pool:<N>` with `N >= 1`.
    ///
    /// A bare `pool` (no size) is rejected loudly: guessing N would be the
    /// silent-fallback this seam exists to prevent — the MEASURE decision doc
    /// owns the calibrated value, never intuition.
    fn from_str(s: &str) -> Result<Self, String> {
        let invalid = || {
            format!(
                "Motor AI inválido en WEBFANG_AI_ENGINE: '{s}'. \
                 Valores válidos: 'single', 'pool:<N>' (p. ej. 'pool:4')"
            )
        };
        let lowered = s.trim().to_lowercase();
        if lowered == "single" {
            return Ok(Self::Single);
        }
        if let Some(count) = lowered.strip_prefix("pool:") {
            let parsed: usize = count.trim().parse().map_err(|_| invalid())?;
            let size = NonZeroUsize::new(parsed).ok_or_else(invalid)?;
            return Ok(Self::Pool { size });
        }
        Err(invalid())
    }
}

/// Build the selected engine from a model file.
///
/// `Single` delegates to [`InferencePool::new`] unchanged (today's behavior,
/// including the drainer graceful-degradation contract). `Pool { size }` opens
/// [`PooledInferenceEngine`] with the split thread budget.
///
/// # Errors
///
/// Returns [`SemanticError::Inference`] when the pool/session cannot be built.
pub fn build_engine(
    config: &EngineConfig,
    model_path: std::path::PathBuf,
    variant: AiModel,
) -> Result<Arc<dyn InferenceEngine + Send + Sync>, SemanticError> {
    match *config {
        EngineConfig::Single => {
            let pool = InferencePool::new(model_path, variant)?;
            Ok(Arc::new(pool))
        },
        EngineConfig::Pool { size } => {
            let pooled = PooledInferenceEngine::open(&model_path, variant, size)?;
            Ok(Arc::new(pooled))
        },
    }
}

/// Build one pool session: file-backed weights (`commit_from_file`, so the
/// application never materializes model bytes), `GraphOptimizationLevel::Level3`,
/// the split intra-op budget, and `inter_threads(1)` (the transformer graph is
/// ≈ sequential; inter-op parallelism inside a single session is measured
/// separately with low priority per the issue).
fn build_pool_session(model_path: &Path, intra_threads: usize) -> Result<Session, SemanticError> {
    let mut builder = Session::builder().map_err(|e| {
        SemanticError::Inference(format!(
            "no se pudo crear el constructor de sesión ONNX: {e}"
        ))
    })?;
    builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| {
            SemanticError::Inference(format!("no se pudo fijar el nivel de optimización: {e}"))
        })?;
    builder = builder
        .with_intra_threads(intra_threads.max(1))
        .map_err(|e| SemanticError::Inference(format!("no se pudo fijar intra_threads: {e}")))?;
    builder = builder
        .with_inter_threads(1)
        .map_err(|e| SemanticError::Inference(format!("no se pudo fijar inter_threads: {e}")))?;
    builder.commit_from_file(model_path).map_err(|e| {
        SemanticError::Inference(format!(
            "no se pudo crear la sesión ONNX desde el archivo: {e}"
        ))
    })
}

/// One exclusive ORT session: the pool's unit of parallelism.
///
/// Owns a single `ort::Session` behind its own `Mutex` (per-session, never a
/// centralized lock: `Session::run` needs `&mut self` in ort 2.0, so some guard
/// is unavoidable — the fix is that N guards never contend, not that guards
/// disappear). Driven with `spawn_blocking` (`async-spawn-blocking`) so the
/// Tokio reactor never blocks on ONNX compute.
///
/// Per-chunk isolation: Granite/ModernBert embedding models are feedforward
/// (no KV-cache, no cross-request state); each `run` is a pure function of its
/// inputs plus the frozen weights, so serving one chunk at a time per session
/// is both sufficient and deterministic. Deliberately NOT `Clone` (#1131
/// discipline): share via `Arc`.
pub struct SingleSessionEngine {
    session: Arc<Mutex<Session>>,
    plan: InputPlan,
    variant: AiModel,
}

impl SingleSessionEngine {
    /// Open one session from `model_path` with the given intra-op budget.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Inference`] when the session cannot be built or
    /// the graph's inputs do not satisfy the required/known contract.
    pub fn open(
        model_path: &Path,
        variant: AiModel,
        intra_threads: usize,
    ) -> Result<Self, SemanticError> {
        let session = build_pool_session(model_path, intra_threads)?;
        let plan = InputPlan::from_session(&session)?;
        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            plan,
            variant,
        })
    }
}

impl InferenceEngine for SingleSessionEngine {
    /// Run one inference on the owned session via `spawn_blocking`.
    ///
    /// Reentrant (`&self`): the only shared mutation is the session `Mutex`,
    /// held inside the blocking thread, never across `.await`
    /// (`async-no-lock-await`). A poisoned mutex fails fast with a typed error.
    fn infer<'a>(
        &'a self,
        input: &'a ModelInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SemanticError>> + Send + 'a>> {
        Box::pin(async move {
            let session = Arc::clone(&self.session);
            let plan = self.plan.clone();
            let input = input.clone();
            let variant = self.variant;
            tokio::task::spawn_blocking(move || {
                let mut guard = session.lock().map_err(|_| {
                    SemanticError::Inference(
                        "sesión ONNX envenenada por un pánico previo en worker".to_string(),
                    )
                })?;
                run_session_inference(&mut guard, &input, variant, &plan)
            })
            .await
            .map_err(|e| SemanticError::Inference(format!("worker de inferencia cancelado: {e}")))?
        })
    }

    fn embedding_dim(&self) -> usize {
        self.variant.output_dim()
    }

    fn is_ready(&self) -> bool {
        true
    }
}

/// One pool slot: an engine plus its own 1-permit semaphore.
struct PoolSlot {
    semaphore: Arc<Semaphore>,
    engine: Arc<dyn InferenceEngine + Send + Sync>,
}

/// N-session inference pool behind the [`InferenceEngine`] seam.
///
/// Each slot pairs one `Arc<dyn InferenceEngine + Send + Sync>` (production: a
/// [`SingleSessionEngine`] owning exactly one `ort::Session`) with its own
/// 1-permit semaphore. Selection is a lock-free `AtomicUsize::fetch_add % N`
/// start index plus `try_acquire` rotation — there is deliberately NO second
/// centralized `Mutex<usize>` round-robin (that would reintroduce the
/// eliminated pattern; the maintainer comment is explicit).
///
/// Backpressure: when every slot is busy, `infer` awaits a permit on the
/// selected slot (cancellable, reactor-friendly) instead of failing hard, so
/// the N+1th concurrent request queues instead of erroring. Permits are
/// `OwnedSemaphorePermit` (RAII): released on return AND on panic unwind.
/// `run` stays reentrant: the only shared mutation is the `Relaxed` counter
/// (a scheduling hint, so the weakest ordering is correct per
/// `conc-atomic-ordering`). Deliberately NOT `Clone` (#1131 discipline).
pub struct PooledInferenceEngine {
    slots: Vec<PoolSlot>,
    next: AtomicUsize,
    intra_threads: usize,
}

impl std::fmt::Debug for PooledInferenceEngine {
    /// Slots hold `Arc<dyn InferenceEngine>` (no `Debug` bound on the seam),
    /// so only the sizing fields are reported.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledInferenceEngine")
            .field("pool_size", &self.slots.len())
            .field("intra_threads", &self.intra_threads)
            .finish_non_exhaustive()
    }
}

impl PooledInferenceEngine {
    /// Wrap pre-built engines (one per slot).
    ///
    /// Mock-backed tests use this: NO model download, NO ORT session — pure
    /// coordination logic (parallel acquires, backpressure, permit release).
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Inference`] when `engines` is empty.
    pub fn from_engines(
        engines: Vec<Arc<dyn InferenceEngine + Send + Sync>>,
    ) -> Result<Self, SemanticError> {
        if engines.is_empty() {
            return Err(SemanticError::Inference(
                "el pool de inferencia necesita al menos una sesión".to_string(),
            ));
        }
        let slots = engines
            .into_iter()
            .map(|engine| PoolSlot {
                semaphore: Arc::new(Semaphore::new(1)),
                engine,
            })
            .collect();
        Ok(Self {
            slots,
            next: AtomicUsize::new(0),
            intra_threads: 1,
        })
    }

    /// Open `size` real sessions from `model_path`, splitting the core budget.
    ///
    /// Unlike [`InferencePool::new`], a session-build failure fails fast with a
    /// typed error instead of spawning a drainer: a half-populated pool would
    /// silently misreport its parallelism budget. The drainer
    /// graceful-degradation contract is kept for the `Single` path only.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Inference`] when any session cannot be built.
    pub fn open(
        model_path: &Path,
        variant: AiModel,
        size: NonZeroUsize,
    ) -> Result<Self, SemanticError> {
        let total = webfang_core::domain::budget::detector::system_parallelism().get();
        let intra = EngineConfig::split_intra_threads(total, size.get());
        let mut engines: Vec<Arc<dyn InferenceEngine + Send + Sync>> =
            Vec::with_capacity(size.get());
        for _ in 0..size.get() {
            let single = SingleSessionEngine::open(model_path, variant, intra)?;
            engines.push(Arc::new(single));
        }
        let mut pooled = Self::from_engines(engines)?;
        pooled.intra_threads = intra;
        Ok(pooled)
    }

    /// Number of sessions in the pool.
    #[must_use]
    pub fn pool_size(&self) -> usize {
        self.slots.len()
    }

    /// Intra-op thread budget each session was built with (split formula).
    #[must_use]
    pub fn intra_threads(&self) -> usize {
        self.intra_threads
    }
}

impl InferenceEngine for PooledInferenceEngine {
    /// Run one inference on the first free slot from the rotated start index.
    ///
    /// Fast path takes a free permit without waiting; slow path awaits a permit
    /// on the selected slot (backpressure, cancellable). The held permit is an
    /// `OwnedSemaphorePermit`, not a `MutexGuard`, so holding it across the
    /// inner `.await` is the intended semaphore pattern (`async-no-lock-await`
    /// bans only `Mutex`/`RwLock` across await) and it releases on drop,
    /// including panic unwind.
    fn infer<'a>(
        &'a self,
        input: &'a ModelInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SemanticError>> + Send + 'a>> {
        Box::pin(async move {
            let len = self.slots.len();
            if len == 0 {
                return Err(SemanticError::Inference(
                    "el pool de inferencia no tiene sesiones".to_string(),
                ));
            }
            let start = self.next.fetch_add(1, Ordering::Relaxed) % len;
            for offset in 0..len {
                let slot = &self.slots[(start + offset) % len];
                if let Ok(permit) = Arc::clone(&slot.semaphore).try_acquire_owned() {
                    let result = slot.engine.infer(input).await;
                    drop(permit);
                    return result;
                }
            }
            let slot = &self.slots[start];
            let permit = Arc::clone(&slot.semaphore)
                .acquire_owned()
                .await
                .map_err(|_| {
                    SemanticError::Inference(
                        "el pool de inferencia se cerró durante la espera".to_string(),
                    )
                })?;
            let result = slot.engine.infer(input).await;
            drop(permit);
            result
        })
    }

    fn embedding_dim(&self) -> usize {
        self.slots
            .first()
            .map(|slot| slot.engine.embedding_dim())
            .unwrap_or_else(|| AiModel::default().output_dim())
    }

    fn is_ready(&self) -> bool {
        !self.slots.is_empty()
    }
}

impl Drop for InferencePool {
    fn drop(&mut self) {
        // Drop sender → disconnects channel → all blocking_recv calls return
        // None. Workers exit their loops and terminate. The join below is
        // bounded because `InferencePool` is not `Clone` (#1131): this is the
        // only sender, so the channel cannot outlive the pool.
        let (dummy_tx, _dummy_rx) = mpsc::channel(1);
        drop(std::mem::replace(&mut self.request_tx, dummy_tx));

        // Release the pool's handle on the shared session BEFORE joining, so the
        // only remaining Arc references belong to workers that are already
        // exiting. The session itself is freed when the last worker drops it.
        drop(self.shared_session.take());

        // Join all worker threads
        for (i, handle) in self._worker_handles.drain(..).enumerate() {
            match handle.join() {
                Ok(()) => debug!(worker_id = i, "Worker joined"),
                Err(e) => tracing::warn!(worker_id = i, error = ?e, "Worker panicked"),
            }
        }

        info!(worker_count = self.worker_count, "InferencePool shut down");
    }
}

/// Builds a single-threaded ONNX session from the model file on disk.
///
/// `commit_from_file` hands the path straight to ONNX Runtime: no ~1.2 GB
/// Granite-311M byte buffer ever exists in the application, so peak RSS
/// includes ORT's single session-internal copy instead of two copies
/// (#1315; warm measurement: 2.05 GiB vs 3.2 GiB pre-fix).
fn build_session(model_path: &Path) -> Result<Session, SemanticError> {
    let mut builder = Session::builder().map_err(|e| {
        SemanticError::Inference(format!("Failed to create ONNX session builder: {e}"))
    })?;
    builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| SemanticError::Inference(format!("Failed to set optimization level: {e}")))?;
    builder = builder
        .with_intra_threads(1)
        .map_err(|e| SemanticError::Inference(format!("Failed to set intra threads: {e}")))?;
    builder.commit_from_file(model_path).map_err(|e| {
        SemanticError::Inference(format!("Failed to create ONNX session from file: {e}"))
    })
}

/// Drains the request channel so a failed pool does not block its callers.
fn drain_channel(receiver: &SharedReceiver) {
    while recv_request(receiver).is_some() {}
}

/// Receive the next request, blocking the worker thread — never the Tokio
/// reactor (workers are plain OS threads, so `blocking_recv` is legal here).
/// Returns `None` when the channel is closed or the receiver lock was
/// poisoned by a panicking worker.
fn recv_request(receiver: &SharedReceiver) -> Option<WorkerRequest> {
    let mut rx = receiver.lock().ok()?;
    rx.blocking_recv()
}

/// Build the single shared session and resolve its input plan once.
fn prepare_shared_session(model_path: &Path) -> Result<(Session, InputPlan), SemanticError> {
    let session = build_session(model_path)?;
    let plan = InputPlan::from_session(&session)?;
    Ok((session, plan))
}

/// Spawn the worker threads that share the single session.
fn spawn_workers(
    receiver: &SharedReceiver,
    session: &SharedSession,
    plan: &InputPlan,
    variant: AiModel,
    worker_count: usize,
) -> Result<Vec<thread::JoinHandle<()>>, SemanticError> {
    let mut handles = Vec::with_capacity(worker_count);

    for worker_id in 0..worker_count {
        let receiver = Arc::clone(receiver);
        let session = Arc::clone(session);
        let plan = plan.clone();

        let handle = thread::Builder::new()
            .name(format!("inference-worker-{worker_id}"))
            .spawn(move || {
                worker_main(&receiver, &session, variant, &plan, worker_id);
            })
            .map_err(|e| {
                SemanticError::Inference(format!("failed to spawn worker {worker_id}: {e}"))
            })?;

        handles.push(handle);
    }

    Ok(handles)
}

/// Spawn a single drainer thread used when the shared session cannot be built.
///
/// Without it, callers would block forever on a bounded channel nobody reads.
fn spawn_drainer(receiver: &SharedReceiver) -> Result<Vec<thread::JoinHandle<()>>, SemanticError> {
    let receiver = Arc::clone(receiver);
    let handle = thread::Builder::new()
        .name("inference-drainer".to_string())
        .spawn(move || {
            drain_channel(&receiver);
        })
        .map_err(|e| SemanticError::Inference(format!("failed to spawn drainer thread: {e}")))?;

    Ok(vec![handle])
}

/// Entry point for one inference worker thread.
///
/// Serves requests from the channel until it disconnects, locking the shared
/// session for the duration of each inference call.
fn worker_main(
    receiver: &SharedReceiver,
    session: &SharedSession,
    variant: AiModel,
    plan: &InputPlan,
    worker_id: usize,
) {
    debug!(worker_id, "Worker ready, waiting for requests");

    while let Some(request) = recv_request(receiver) {
        // A poisoned mutex means another worker panicked mid-inference: the
        // shared session is no longer trustworthy, so every request fails fast
        // instead of panicking this thread too. The crate denies `expect_used`.
        let result = match session.lock() {
            Ok(mut guard) => run_session_inference(&mut guard, &request.input, variant, plan),
            Err(_) => {
                error!(
                    worker_id,
                    "Shared ONNX session mutex poisoned by a previous worker panic"
                );
                Err(SemanticError::Inference(
                    "shared ONNX session poisoned by a previous worker panic".to_string(),
                ))
            },
        };
        let _ = request.reply_tx.send(result);
    }

    debug!(worker_id, "Worker exiting (channel disconnected)");
}

// ---------------------------------------------------------------------------
// Shared inference logic
// ---------------------------------------------------------------------------

/// Run inference on a pre-built session (synchronous).
///
/// Used by `InferencePool` workers that own persistent sessions.
/// Handles tensor creation, session execution, mean pooling, and L2 normalization.
///
/// Inputs are built by iterating `plan.names` (the model graph's real input
/// set), so an undeclared `token_type_ids` is simply never sent (#543).
fn run_session_inference(
    session: &mut Session,
    input: &ModelInput,
    model_variant: AiModel,
    plan: &InputPlan,
) -> Result<Vec<f32>, SemanticError> {
    let seq_len = input.seq_len();
    let model_native_dim = model_variant.embedding_dim();
    let model_output_dim = model_variant.output_dim();

    // Build named input tensors from the resolved plan, in graph order.
    let mut named_inputs: Vec<(
        std::borrow::Cow<'_, str>,
        ort::session::SessionInputValue<'_>,
    )> = Vec::with_capacity(plan.names.len());

    for name in plan.names() {
        let array = match name.as_str() {
            "input_ids" => {
                ndarray::Array2::<i64>::from_shape_vec((1, seq_len), input.input_ids.clone())
                    .map_err(|e| {
                        SemanticError::Inference(format!("failed to create input_ids array: {e}"))
                    })?
            },
            "attention_mask" => {
                ndarray::Array2::<i64>::from_shape_vec((1, seq_len), input.attention_mask.clone())
                    .map_err(|e| {
                    SemanticError::Inference(format!("failed to create attention_mask array: {e}"))
                })?
            },
            "token_type_ids" => {
                ndarray::Array2::<i64>::from_shape_vec((1, seq_len), input.token_type_ids.clone())
                    .map_err(|e| {
                    SemanticError::Inference(format!("failed to create token_type_ids array: {e}"))
                })?
            },
            other => {
                return Err(SemanticError::Inference(format!(
                    "unsupported input: {other}"
                )));
            },
        };

        let tensor = ort::value::Tensor::from_array(array).map_err(|e| {
            SemanticError::Inference(format!("failed to create {name} tensor: {e}"))
        })?;

        named_inputs.push((std::borrow::Cow::Borrowed(name.as_str()), tensor.into()));
    }

    // Run inference with the name->value map resolved from the graph.
    let outputs = session
        .run(named_inputs)
        .map_err(|e| SemanticError::Inference(format!("model execution failed: {e}")))?;

    // Extract last_hidden_state output
    let (_shape, raw_data): (_, &[f32]) = outputs["last_hidden_state"]
        .try_extract_tensor::<f32>()
        .map_err(|e| {
            SemanticError::Inference(format!("failed to extract last_hidden_state: {e}"))
        })?;

    // Convert to Vec<f32>
    let embedding_flat: Vec<f32> = raw_data.to_vec();

    // Apply Mean Pooling on the native embedding dimension
    use crate::infrastructure_ai::embedding_ops::{l2_normalize_safe, mean_pool};
    let pooled = mean_pool(
        &embedding_flat,
        seq_len,
        model_native_dim,
        &input.attention_mask,
    );

    // Matryoshka truncation: for 311M, slice native 768d down to first 384 elements
    let truncated: Vec<f32> = pooled.iter().take(model_output_dim).copied().collect();

    let embedding = l2_normalize_safe(&truncated);

    Ok(embedding)
}

/// Batched inference over N right-padded inputs in a single `Session::run`.
///
/// Prototype probe for issue #1456 (P0-001 batch smoke run): right-pads N
/// inputs to S_max with per-row `attention_mask` (0 = pad), executes ONE
/// session run with shape `(N, S_max)`, applies per-row mean pooling over
/// the `(N, S_max, H)` output honoring each row's mask, then the existing
/// Matryoshka `take(384)` + L2-normalize per row. Output order matches input
/// order (determinism hard-constraint: no reordering, no cross-row fusion).
///
/// The single-chunk `run_session_inference()` path is untouched; this is the
/// surgical batched companion behind the existing [`InferenceEngine`] seam.
/// Single session + `intra_threads(1)` are unchanged: this probe isolates
/// batching, not the pool.
///
/// # Errors
///
/// Returns [`SemanticError::Inference`]
/// when the batch is empty, any input is an empty sequence, tensor
/// construction fails, the model execution fails, or the
/// `last_hidden_state` output has an unexpected length.
pub fn run_batched_inference(
    session: &mut Session,
    plan: &InputPlan,
    inputs: &[ModelInput],
    model_variant: AiModel,
) -> Result<Vec<Vec<f32>>, SemanticError> {
    if inputs.is_empty() {
        return Err(SemanticError::Inference(
            "batched inference requires at least one input".to_string(),
        ));
    }
    let batch_size = inputs.len();
    let seq_max = inputs.iter().map(ModelInput::seq_len).max().unwrap_or(0);
    if seq_max == 0 {
        return Err(SemanticError::Inference(
            "batched inference requires non-empty sequences".to_string(),
        ));
    }
    let model_native_dim = model_variant.embedding_dim();
    let model_output_dim = model_variant.output_dim();

    // Right-pad every row to S_max (input_ids/type pad 0, mask pad 0).
    let mut ids_flat: Vec<i64> = Vec::with_capacity(batch_size * seq_max);
    let mut mask_flat: Vec<i64> = Vec::with_capacity(batch_size * seq_max);
    let mut type_flat: Vec<i64> = Vec::with_capacity(batch_size * seq_max);
    for input in inputs {
        let pad = seq_max.saturating_sub(input.seq_len());
        ids_flat.extend_from_slice(&input.input_ids);
        ids_flat.extend(std::iter::repeat_n(0i64, pad));
        mask_flat.extend_from_slice(&input.attention_mask);
        mask_flat.extend(std::iter::repeat_n(0i64, pad));
        type_flat.extend_from_slice(&input.token_type_ids);
        type_flat.extend(std::iter::repeat_n(0i64, pad));
    }

    // Build named input tensors from the resolved plan, in graph order — the
    // same contract as the single path, with shape (N, S_max).
    let mut named_inputs: Vec<(
        std::borrow::Cow<'_, str>,
        ort::session::SessionInputValue<'_>,
    )> = Vec::with_capacity(plan.names().len());

    for name in plan.names() {
        let array = match name.as_str() {
            "input_ids" => {
                ndarray::Array2::<i64>::from_shape_vec((batch_size, seq_max), ids_flat.clone())
                    .map_err(|e| {
                        SemanticError::Inference(format!(
                            "failed to create batched input_ids array: {e}"
                        ))
                    })?
            },
            "attention_mask" => {
                ndarray::Array2::<i64>::from_shape_vec((batch_size, seq_max), mask_flat.clone())
                    .map_err(|e| {
                        SemanticError::Inference(format!(
                            "failed to create batched attention_mask array: {e}"
                        ))
                    })?
            },
            "token_type_ids" => {
                ndarray::Array2::<i64>::from_shape_vec((batch_size, seq_max), type_flat.clone())
                    .map_err(|e| {
                        SemanticError::Inference(format!(
                            "failed to create batched token_type_ids array: {e}"
                        ))
                    })?
            },
            other => {
                return Err(SemanticError::Inference(format!(
                    "unsupported input: {other}"
                )));
            },
        };

        let tensor = ort::value::Tensor::from_array(array).map_err(|e| {
            SemanticError::Inference(format!("failed to create batched {name} tensor: {e}"))
        })?;

        named_inputs.push((std::borrow::Cow::Borrowed(name.as_str()), tensor.into()));
    }

    let outputs = session
        .run(named_inputs)
        .map_err(|e| SemanticError::Inference(format!("batched model execution failed: {e}")))?;

    let (_shape, raw_data): (_, &[f32]) = outputs["last_hidden_state"]
        .try_extract_tensor::<f32>()
        .map_err(|e| {
            SemanticError::Inference(format!("failed to extract batched last_hidden_state: {e}"))
        })?;

    let expected = batch_size * seq_max * model_native_dim;
    if raw_data.len() != expected {
        return Err(SemanticError::Inference(format!(
            "unexpected batched output length: got {}, expected {expected} (N={batch_size}, S={seq_max}, H={model_native_dim})",
            raw_data.len()
        )));
    }
    let embedding_flat: Vec<f32> = raw_data.to_vec();

    // Per-row mean pool honoring each row's mask, then Matryoshka + L2 per row.
    use crate::infrastructure_ai::embedding_ops::{l2_normalize_safe, mean_pool_batched};
    let rows = mean_pool_batched(
        &embedding_flat,
        batch_size,
        seq_max,
        model_native_dim,
        &mask_flat,
    );
    let embeddings: Vec<Vec<f32>> = rows
        .iter()
        .map(|pooled| {
            let truncated: Vec<f32> = pooled.iter().take(model_output_dim).copied().collect();
            l2_normalize_safe(&truncated)
        })
        .collect();

    Ok(embeddings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure_ai::cache_config::AiModel;

    /// A model path that can never build a session (deterministic, no fs
    /// writes): `InferencePool::new` must still construct the pool and spawn
    /// the drainer, exercising the graceful-degradation contract (#1315).
    const FAKE_MODEL_PATH: &str = "/nonexistent/webfang-fake-model.onnx";

    // --- EngineConfig::from_env tests (P0-001 MEASURE, issue #1456) ---

    /// Unset / empty / whitespace-only env means `Single`: the user made no
    /// choice, so the production default applies silently (only set-but-invalid
    /// is loud). Pure `resolve_spec`, so no env mutation under parallel tests.
    #[test]
    fn test_engine_config_unset_or_blank_means_single() {
        assert_eq!(EngineConfig::resolve_spec(None), Ok(EngineConfig::Single));
        assert_eq!(
            EngineConfig::resolve_spec(Some("")),
            Ok(EngineConfig::Single)
        );
        assert_eq!(
            EngineConfig::resolve_spec(Some("   \t")),
            Ok(EngineConfig::Single)
        );
    }

    /// `single` (trimmed, case-insensitive) selects today's behavior.
    #[test]
    fn test_engine_config_single_parses() {
        assert_eq!(
            EngineConfig::resolve_spec(Some("single")),
            Ok(EngineConfig::Single)
        );
        assert_eq!(
            EngineConfig::resolve_spec(Some("  Single ")),
            Ok(EngineConfig::Single)
        );
    }

    /// `pool:<N>` selects N sessions; `N >= 1` enforced via `NonZeroUsize`.
    #[test]
    fn test_engine_config_pool_sizes_parse() {
        for n in [1usize, 2, 4, 8, 15] {
            let size = NonZeroUsize::new(n).expect("test sizes are non-zero");
            assert_eq!(
                EngineConfig::resolve_spec(Some(&format!("pool:{n}"))),
                Ok(EngineConfig::Pool { size })
            );
        }
    }

    /// Set-but-invalid is a loud Spanish error naming the variable and the
    /// valid values — never a silent fallback to `Single` (#874 discipline).
    /// A bare `pool` (no size) is rejected: guessing N would be silent fallback.
    #[test]
    fn test_engine_config_invalid_is_loud_spanish_error() {
        for bad in [
            "pool", "pool:0", "pool:-2", "pool:abc", "pool:", "turbo", "8",
        ] {
            let err = EngineConfig::resolve_spec(Some(bad))
                .expect_err(&format!("{bad:?} must be rejected"));
            assert!(
                err.contains(EngineConfig::ENV_VAR),
                "error must name the env var, got: {err}"
            );
            assert!(
                err.contains("'single'") && err.contains("'pool:<N>'"),
                "error must list valid values, got: {err}"
            );
        }
    }

    /// `from_env` agrees with the pure core for whatever the ambient
    /// environment holds (shape check only — value cases live above, since env
    /// mutation is racy under parallel test execution).
    #[test]
    fn test_engine_config_from_env_never_panics() {
        let raw = std::env::var(EngineConfig::ENV_VAR).ok();
        assert_eq!(
            EngineConfig::from_env(),
            EngineConfig::resolve_spec(raw.as_deref())
        );
    }

    // --- InferencePool tests ---

    /// Test that InferencePool type exists and compiles
    #[test]
    fn test_inference_pool_type_exists() {
        fn _assert_type_exists(_pool: InferencePool) {}
    }

    /// Test that InferencePool is Send + Sync (thread-safe)
    #[test]
    fn test_inference_pool_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}

        assert_send::<InferencePool>();
        assert_sync::<InferencePool>();
    }

    /// Paso 0 (#1456): the mock engine is `Send + Sync` and object-safe, so it
    /// can replace the pool behind `Arc<E>` / `Arc<dyn InferenceEngine + Send + Sync>`.
    #[test]
    fn test_mock_engine_is_send_sync_and_object_safe() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}

        assert_send::<MockInferenceEngine>();
        assert_sync::<MockInferenceEngine>();

        let engine = MockInferenceEngine::new(std::time::Duration::from_millis(1));
        let erased: &dyn super::InferenceEngine = &engine;
        assert_eq!(erased.embedding_dim(), 384);
        assert!(erased.is_ready());
    }

    /// Paso 0 (#1456): every mock inference returns the same 384-dim unit
    /// vector, so benchmark runs are deterministic and relevance filtering
    /// keeps all chunks.
    #[tokio::test]
    async fn test_mock_engine_returns_deterministic_unit_embedding() {
        use super::InferenceEngine;

        let engine = MockInferenceEngine::new(std::time::Duration::from_millis(1));
        let first = ModelInput::from_tokens(vec![101, 5, 6, 102]);
        let second = ModelInput::from_tokens(vec![101, 7, 8, 9, 102]);

        let a = engine.infer(&first).await.expect("mock infer must succeed");
        let b = engine
            .infer(&second)
            .await
            .expect("mock infer must succeed");

        assert_eq!(a.len(), 384, "mock embedding must be 384-dim");
        assert_eq!(a, b, "mock embedding must be input-independent");
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "mock embedding must be L2-normalized, got norm {norm}"
        );
    }

    /// Paso 0 (#1456): concurrent mock infers overlap (no real mutex): N
    /// parallel 50ms infers finish in well under N × 50ms.
    #[tokio::test]
    async fn test_mock_engine_infers_overlap() {
        use super::InferenceEngine;

        let engine = MockInferenceEngine::new(std::time::Duration::from_millis(50));
        let input = ModelInput::from_tokens(vec![101, 5, 102]);
        let started = std::time::Instant::now();
        let (r1, r2, r3, r4) = tokio::join!(
            engine.infer(&input),
            engine.infer(&input),
            engine.infer(&input),
            engine.infer(&input)
        );
        let elapsed = started.elapsed();
        for r in [r1, r2, r3, r4] {
            r.expect("mock infer must succeed");
        }
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "4 parallel 50ms mock infers must overlap (no mutex); took {elapsed:?}"
        );
    }

    /// #1131 — `InferencePool` must NOT be `Clone`.
    ///
    /// A cloned sender keeps the request channel open after the owner drops,
    /// so the owner's `Drop` joins forever and the ONNX `Session` plus the
    /// `inference-worker-*` threads leak. The pool is shared exclusively via
    /// `Arc<InferencePool>` (main.rs, ai_wiring, adapters).
    ///
    /// Compile-time probe via autoref specialization: the inherent
    /// `Probe::<T>::is_clone` wins over the blanket trait fallback exactly when
    /// `T: Clone`. The `ModelInput` positive control guards the probe itself
    /// from silently degrading to "always false".
    #[test]
    fn test_inference_pool_is_not_clone() {
        struct Yes;
        struct No;
        trait Answer {
            fn answer(&self) -> bool;
        }
        impl Answer for Yes {
            fn answer(&self) -> bool {
                true
            }
        }
        impl Answer for No {
            fn answer(&self) -> bool {
                false
            }
        }

        struct Probe<T>(std::marker::PhantomData<T>);
        #[allow(dead_code)]
        impl<T: Clone> Probe<T> {
            fn is_clone(&self) -> Yes {
                Yes
            }
        }
        trait NotCloneFallback {
            fn is_clone(&self) -> No {
                No
            }
        }
        impl<T> NotCloneFallback for T {}

        assert!(
            Probe::<ModelInput>(std::marker::PhantomData)
                .is_clone()
                .answer(),
            "probe control failed: ModelInput is Clone, so the probe is broken"
        );
        assert!(
            !Probe::<InferencePool>(std::marker::PhantomData)
                .is_clone()
                .answer(),
            "#1131: InferencePool must not be Clone — a cloned sender keeps \
             the channel open and hangs the owner's Drop join forever"
        );
    }

    /// #1131 — `Drop` returns in bounded time: without `Clone` the pool owns
    /// the only sender, so dropping it disconnects the channel, every worker
    /// exits its `blocking_recv` loop, and each `join()` completes. When the
    /// join returns, the workers' `Arc<Session>` clones are gone and the pool
    /// already released its own handle, so the ONNX `Session` is freed.
    ///
    /// Pre-fix this hung: the repro (run on the base commit) showed the owner
    /// blocked >5s while a clone was alive, completing only once the clone
    /// dropped. Now no clone can exist, so the drop is bounded by construction.
    #[test]
    fn test_inference_pool_drop_returns_in_bounded_time() {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let owner = thread::spawn(move || {
            let pool = InferencePool::new(
                std::path::PathBuf::from(FAKE_MODEL_PATH),
                AiModel::Granite97M,
            )
            .expect("pool creation must succeed even with an unloadable model file");
            drop(pool);
            let _ = done_tx.send(());
        });

        done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect(
                "Drop must complete in bounded time: the channel closes and every worker is joined",
            );
        owner.join().expect("owner thread must not panic");
    }

    /// Test InferencePool::new with an unloadable model path
    ///
    /// Uses a model path that cannot build a session — workers will fail to
    /// build sessions but the pool itself should still be created. The
    /// workers drain the channel and exit cleanly.
    #[test]
    fn test_inference_pool_creation() {
        let pool = InferencePool::new(
            std::path::PathBuf::from(FAKE_MODEL_PATH),
            AiModel::Granite97M,
        )
        .expect("Pool should create even with an unloadable model file");

        assert_eq!(pool.model_variant(), AiModel::Granite97M);
        assert_eq!(pool.worker_count(), (num_cpus::get() - 1).max(1));
        assert_eq!(pool.embedding_dim(), 384);
    }

    /// Test that dropping the pool causes all workers to exit cleanly
    #[test]
    fn test_inference_pool_graceful_shutdown() {
        let pool = InferencePool::new(
            std::path::PathBuf::from(FAKE_MODEL_PATH),
            AiModel::Granite97M,
        )
        .expect("Pool should create");

        let worker_count = pool.worker_count();
        drop(pool);

        // If we get here without hanging, workers exited cleanly
        assert!(worker_count > 0);
    }

    /// Test that infer() returns an error when channel has no workers
    ///
    /// Creates a pool with an unloadable model path. Workers fail to build
    /// sessions, drain the channel, and exit. The pool is then dropped (clean
    /// shutdown). This validates the full lifecycle: creation → worker
    /// failure → shutdown.
    #[test]
    fn test_inference_pool_worker_failure_lifecycle() {
        let pool = InferencePool::new(
            std::path::PathBuf::from(FAKE_MODEL_PATH),
            AiModel::Granite97M,
        )
        .expect("Pool should create");

        // Workers fail to build sessions with the missing file, drain
        // channel, and exit. Give workers time to fail and exit.
        thread::sleep(std::time::Duration::from_millis(100));

        // Drop the pool — workers should already be exited, join succeeds
        drop(pool);
        // If we reach here without hanging, shutdown was clean
    }

    /// #1133 — under backpressure `infer` must yield to the reactor, not park
    /// the worker thread.
    ///
    /// Builds a pool-shaped sender over a FULL bounded channel with no worker
    /// draining it, on a current-thread runtime. The old synchronous crossbeam
    /// `send` parked the only thread, so the timer could never fire (the test
    /// hangs on the pre-fix code). The async `tokio::sync::mpsc` send parks the
    /// TASK cooperatively, the timer fires, and `infer` is still pending.
    #[tokio::test]
    async fn test_infer_backpressure_yields_to_executor() {
        let (tx, rx) = mpsc::channel::<WorkerRequest>(1);
        // Fill the channel so the next send must wait for capacity.
        let (hold_tx, _hold_rx) = oneshot::channel();
        tx.try_send(WorkerRequest {
            input: ModelInput::from_tokens(vec![101, 2]),
            reply_tx: hold_tx,
        })
        .expect("first send fills capacity 1");

        let pool = InferencePool {
            request_tx: tx,
            _worker_handles: Vec::new(),
            shared_session: None,
            model_variant: AiModel::Granite97M,
            worker_count: 1,
        };
        let input = ModelInput::from_tokens(vec![101, 2]);

        let outcome =
            tokio::time::timeout(std::time::Duration::from_millis(50), pool.infer(&input)).await;

        assert!(
            outcome.is_err(),
            "infer must stay pending (cancellable) while the queue is full — \
             a blocking send would have parked this thread and hung the test"
        );
        drop(rx); // keep the receiver alive until the assertion has run
    }

    /// #1133 — when every receiver is gone, `infer` fails promptly with the
    /// typed closed-channel error instead of blocking.
    #[tokio::test]
    async fn test_infer_closed_channel_errors_promptly() {
        let (tx, rx) = mpsc::channel::<WorkerRequest>(1);
        drop(rx);
        let pool = InferencePool {
            request_tx: tx,
            _worker_handles: Vec::new(),
            shared_session: None,
            model_variant: AiModel::Granite97M,
            worker_count: 1,
        };
        let input = ModelInput::from_tokens(vec![101, 2]);
        let err = pool
            .infer(&input)
            .await
            .expect_err("closed channel must error");
        assert!(
            err.to_string().contains("channel closed"),
            "error must name the closed channel, got: {err}"
        );
    }

    // --- ModelInput tests ---

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
            "L2 norm should be 1.0, got {norm}"
        );
    }

    // --- InputPlan tests (pure, no ONNX model required) ---

    /// #543 exact contract: a 2-input ModernBert graph (no token_type_ids)
    /// resolves successfully and omits token_type_ids.
    #[test]
    fn test_input_plan_resolve_two_inputs_omits_token_type_ids() {
        let plan = InputPlan::resolve(&["input_ids", "attention_mask"]);
        let plan = plan.expect("2-input graph should resolve");
        assert_eq!(plan.names(), &["input_ids", "attention_mask"]);
    }

    /// Forward-compat: a graph that declares all 3 inputs still resolves.
    #[test]
    fn test_input_plan_resolve_three_inputs_ok() {
        let plan = InputPlan::resolve(&["input_ids", "attention_mask", "token_type_ids"]);
        let plan = plan.expect("3-input graph should resolve");
        assert_eq!(
            plan.names(),
            &["input_ids", "attention_mask", "token_type_ids"]
        );
    }

    /// Missing required input `input_ids` is an error naming the missing input.
    #[test]
    fn test_input_plan_resolve_missing_input_ids_errors() {
        let err =
            InputPlan::resolve(&["attention_mask"]).expect_err("missing input_ids must error");
        let msg = err.to_string();
        assert!(
            msg.contains("input_ids"),
            "error should name missing input: {msg}"
        );
        assert!(
            msg.contains("missing required"),
            "error should report missing required: {msg}"
        );
    }

    /// An unsupported input name is rejected.
    #[test]
    fn test_input_plan_resolve_unsupported_input_errors() {
        let err = InputPlan::resolve(&["input_ids", "attention_mask", "position_ids"])
            .expect_err("unsupported input must error");
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported"),
            "error should report unsupported: {msg}"
        );
        assert!(
            msg.contains("position_ids"),
            "error should name offending input: {msg}"
        );
    }

    /// Inputs are matched by name, so graph order does not matter.
    #[test]
    fn test_input_plan_resolve_order_inverted_ok() {
        let plan = InputPlan::resolve(&["attention_mask", "input_ids"]);
        let plan = plan.expect("reordered 2-input graph should resolve");
        assert_eq!(plan.names(), &["attention_mask", "input_ids"]);
    }
}

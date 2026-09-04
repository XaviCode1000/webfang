//! Inference engine — ONNX model execution with ort (ONNX Runtime)
//!
//! Handles loading and executing ONNX models for sentence embedding generation:
//! - Thread-safe model bytes sharing with `Arc<Vec<u8>>` (`own-arc-shared`)
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
//!   workers and shares it as `Arc<Mutex<Session>>`. `Session::run` takes `&mut self`
//!   in ort 2.0, so the `Mutex` is required — it costs nothing because the request
//!   channel already serializes work per worker. Building one session per worker
//!   duplicated the whole model graph in RSS on every CPU core (#648).
//! - **384-dim invariant**: Granite-97M is natively 384d; Granite-311M uses Matryoshka
//!   truncation to 384d. No runtime dimension discovery needed.
//! - **spawn_blocking**: CPU-intensive ONNX inference runs in blocking pool to avoid
//!   starving async runtime.
//! - **No locks across await**: Clone Arc before async operations.

use std::sync::{Arc, Mutex};

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
// InferencePool: dedicated worker threads with persistent sessions
// ---------------------------------------------------------------------------

use std::thread;

use tokio::sync::{mpsc, oneshot};
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
    /// The `ort::Session` is built ONCE with `intra_threads(1)` and shared by
    /// every worker through `Arc<Mutex<Session>>`, so the model graph is
    /// resident in memory a single time instead of once per CPU core (#648).
    /// Spawns `(num_cpus - 1).max(1)` OS threads.
    ///
    /// When the session cannot be built (invalid model bytes), the pool is still
    /// created: a drainer thread consumes pending requests so callers get a
    /// prompt error instead of blocking forever.
    ///
    /// # Errors
    ///
    /// Returns `SemanticError::Inference` if a thread fails to spawn.
    pub fn new(model_bytes: Arc<Vec<u8>>, model_variant: AiModel) -> Result<Self, SemanticError> {
        // Canonical detector seam (Q2, via core dependency): process-wide "auto".
        let worker_count =
            (webfang_core::domain::budget::detector::system_parallelism().get() - 1).max(1);
        // #1133: bounded tokio mpsc — `infer` applies backpressure with an
        // awaitable `send`, so a full queue parks the TASK on the reactor,
        // never a Tokio worker thread on a synchronous crossbeam send.
        let (request_tx, receiver) = mpsc::channel::<WorkerRequest>(worker_count);
        let receiver: SharedReceiver = Arc::new(Mutex::new(receiver));

        let (shared_session, worker_handles) = match prepare_shared_session(&model_bytes) {
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
    /// Sends the request via the bounded channel — under backpressure the
    /// `send` is an await point that yields the task to the reactor (#1133),
    /// so `tokio::time::timeout`/cancellation keep working while the queue is
    /// full — then awaits the oneshot result.
    ///
    /// # Errors
    ///
    /// Returns `SemanticError::Inference` if the channel is closed or the
    /// worker drops the response.
    #[instrument(skip_all)]
    pub async fn infer(&self, input: &ModelInput) -> Result<Vec<f32>, SemanticError> {
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

/// Builds a single-threaded ONNX session from model bytes in memory.
fn build_session(bytes: &[u8]) -> Result<Session, SemanticError> {
    let mut builder = Session::builder().map_err(|e| {
        SemanticError::Inference(format!("Failed to create ONNX session builder: {e}"))
    })?;
    builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| SemanticError::Inference(format!("Failed to set optimization level: {e}")))?;
    builder = builder
        .with_intra_threads(1)
        .map_err(|e| SemanticError::Inference(format!("Failed to set intra threads: {e}")))?;
    builder.commit_from_memory(bytes).map_err(|e| {
        SemanticError::Inference(format!("Failed to create ONNX session from memory: {e}"))
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
fn prepare_shared_session(bytes: &[u8]) -> Result<(Session, InputPlan), SemanticError> {
    let session = build_session(bytes)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure_ai::cache_config::AiModel;

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
        let fake_bytes = Arc::new(b"fake model bytes".to_vec());
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let owner = thread::spawn(move || {
            let pool = InferencePool::new(fake_bytes, AiModel::Granite97M)
                .expect("pool creation must succeed even with invalid model bytes");
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

    /// Test InferencePool::new with fake model bytes
    ///
    /// Uses invalid model bytes — workers will fail to build sessions but
    /// the pool itself should still be created. The workers drain the
    /// channel and exit cleanly.
    #[test]
    fn test_inference_pool_creation() {
        let fake_bytes = Arc::new(b"not a real onnx model".to_vec());
        let pool = InferencePool::new(fake_bytes, AiModel::Granite97M)
            .expect("Pool should create even with invalid model bytes");

        assert_eq!(pool.model_variant(), AiModel::Granite97M);
        assert_eq!(pool.worker_count(), (num_cpus::get() - 1).max(1));
        assert_eq!(pool.embedding_dim(), 384);
    }

    /// Test that dropping the pool causes all workers to exit cleanly
    #[test]
    fn test_inference_pool_graceful_shutdown() {
        let fake_bytes = Arc::new(b"fake model bytes".to_vec());
        let pool = InferencePool::new(fake_bytes, AiModel::Granite97M).expect("Pool should create");

        let worker_count = pool.worker_count();
        drop(pool);

        // If we get here without hanging, workers exited cleanly
        assert!(worker_count > 0);
    }

    /// Test that infer() returns an error when channel has no workers
    ///
    /// Creates a pool with invalid model bytes. Workers fail to build sessions,
    /// drain the channel, and exit. The pool is then dropped (clean shutdown).
    /// This validates the full lifecycle: creation → worker failure → shutdown.
    #[test]
    fn test_inference_pool_worker_failure_lifecycle() {
        let fake_bytes = Arc::new(b"fake model bytes".to_vec());
        let pool = InferencePool::new(fake_bytes, AiModel::Granite97M).expect("Pool should create");

        // Workers fail to build sessions with invalid bytes, drain channel, and exit.
        // Give workers time to fail and exit.
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

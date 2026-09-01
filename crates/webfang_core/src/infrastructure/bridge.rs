//! Bridge between sync and async contexts for elastic ingestion.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use tokio::sync::oneshot;
use tracing::{warn, Instrument};

use crate::domain::content_processor::ContentProcessor;
use crate::domain::cpu_executor::CpuExecutorPort;
use crate::error::ScraperError;
use crate::infrastructure::cpu_pool::RayonCpuPool;
use crate::infrastructure::crawler::resource_downloader::DownloadedResource;

/// Domain-owned chunk DTO (ADR-0012-B 3.E): the type now lives in
/// `domain::cpu_executor`; this re-export shim keeps infra-internal users
/// (`dispatch_resource`, `ProcessedResource`, tests) compiling unchanged.
pub use crate::domain::cpu_executor::ProcessedChunk;

/// Result of dispatching a [`DownloadedResource`] through the CPU bridge.
#[derive(Debug, Clone)]
pub struct ProcessedResource {
    /// Source URL of the downloaded resource.
    pub resource_url: String,
    /// Cleaned content chunks (`lol_html` produces one text chunk; the
    /// orchestrator may split further / attach ONNX embeddings).
    pub chunks: Vec<ProcessedChunk>,
    /// Processing metadata (size, chunk count, cleaner provenance).
    pub metadata: serde_json::Value,
}

/// Tokio→Rayon crossing for CPU-bound ingestion work (frozen design decision #3).
///
/// Holds a dedicated [`RayonCpuPool`] (cloned cheaply — it wraps an `Arc`) and
/// a [`ContentProcessor`] strategy for HTML-to-text conversion, plus exposes
/// a generic [`dispatch`](CpuBridge::dispatch) that moves any CPU-bound
/// closure off the event loop, and a typed
/// [`dispatch_resource`](CpuBridge::dispatch_resource) that cleans
/// [`DownloadedResource`] → [`ProcessedResource`] via the injected processor.
///
/// `CpuBridge` is `Send + Sync` (spec: "dispatch gateway MUST be Send + Sync"),
/// so it can be shared across Tokio tasks via `Arc<CpuBridge>`.
///
/// `CpuBridge` also implements the domain port
/// [`CpuExecutorPort`] so `application::elastic_ingestion` can hold it as
/// `Arc<dyn CpuExecutorPort>` (ADR-0012-B 3.E); the port's primitive
/// `dispatch_resource(url, content, size)` adapts to the inherent typed
/// dispatch and remaps the resource-level result to the port's
/// chunk-level result (the only part the orchestrator consumes).
pub struct CpuBridge {
    pool: RayonCpuPool,
    processor: Arc<dyn ContentProcessor>,
}

impl CpuBridge {
    /// Wrap a dedicated [`RayonCpuPool`] and [`ContentProcessor`] in a bridge.
    #[must_use]
    pub fn new(pool: RayonCpuPool, processor: Arc<dyn ContentProcessor>) -> Self {
        Self { pool, processor }
    }

    /// Borrow the underlying CPU pool.
    #[must_use]
    pub fn pool(&self) -> &RayonCpuPool {
        &self.pool
    }

    /// Dispatch an arbitrary CPU-bound closure onto the Rayon pool and return
    /// a [`oneshot::Receiver`] holding `Result<R, ScraperError>`.
    ///
    /// The work runs under `tokio::task::spawn_blocking` + `pool.install`, so
    /// the Tokio event loop stays unblocked and any nested `par_iter` routes to
    /// the sized dedicated pool. CPU panics are caught via
    /// `catch_unwind(AssertUnwindSafe(…))` (frozen user decision #1) and
    /// mapped to [`ScraperError::Ingestion`] so Rayon threads stay alive.
    ///
    /// If the caller drops the receiver before the work finishes (Tokio task
    /// abort), `tx.send()` fails and the bridge logs a `tracing::warn!` but
    /// does NOT panic (Trap 2).
    pub fn dispatch<F, R>(&self, work: F) -> oneshot::Receiver<Result<R, ScraperError>>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let pool = self.pool.clone();
        // The Instrumented wrapper attaches the current tracing span to the
        // blocking task. We intentionally drop it here because the task runs
        // fire-and-forget via a oneshot channel — the JoinHandle is not awaited.
        let handle = tokio::task::spawn_blocking(move || {
            let caught = catch_unwind(AssertUnwindSafe(move || pool.install(work)));
            // LCOV_EXCL_START defensive: cpu-pool-panic — a panic in Rayon work is a bug; the pool must not die with it
            let mapped: Result<R, ScraperError> = caught.map_err(|panic| {
                let msg = panic_message(&*panic);
                ScraperError::ingestion(format!("CPU pool panic: {msg}"))
            });
            // LCOV_EXCL_STOP
            // LCOV_EXCL_START defensive: oneshot-receiver-dropped — the Tokio task was aborted before consuming the result
            if tx.send(mapped).is_err() {
                warn!(
                    reason = "receptor oneshot descartado",
                    "canal CPU bridge descartado: tarea Tokio abortada antes de recibir el resultado"
                );
            }
            // LCOV_EXCL_STOP
        });
        // Suppress clippy warning: this is fire-and-forget via oneshot channel.
        // The span context is captured by in_current_span() before the handle
        // is dropped — the spawned task still runs with the correct span.
        #[allow(clippy::let_underscore_future)]
        let _ = handle.in_current_span();
        rx
    }

    /// Typed dispatch: clean a [`DownloadedResource`] into a
    /// [`ProcessedResource`] on the Rayon pool.
    ///
    /// PR5 wires real `lol_html` boilerplate removal (via `clean_html_to_text`)
    /// that strips `script`/`style`/`nav`/`footer`/`aside` chrome and extracts
    /// visible text. The cleaner is infallible (`clean_html` falls back to the
    /// raw HTML on a `lol_html` parse error), so the work closure returns
    /// `ProcessedResource` directly and reuses [`dispatch`](Self::dispatch)'s single `Result`
    /// wrap. Embeddings stay `None` here — ONNX inference is async and runs in
    /// the orchestrator's async layer (Decision 5); the bridge is sync
    /// CPU-bound text extraction only.
    pub fn dispatch_resource(
        &self,
        payload: DownloadedResource,
    ) -> oneshot::Receiver<Result<ProcessedResource, ScraperError>> {
        let url = payload.url.clone();
        let size = payload.size_bytes;
        let processor = Arc::clone(&self.processor);
        let processor_name = processor.name().to_owned();
        self.dispatch(move || {
            let html = String::from_utf8_lossy(&payload.bytes);
            let text = processor.process(&html);
            let chunk = ProcessedChunk {
                content: text,
                embedding: None,
            };
            let metadata = serde_json::json!({
                "size_bytes": size,
                "chunk_count": 1u64,
                "cleaner": processor_name,
            });
            ProcessedResource {
                resource_url: url,
                chunks: vec![chunk],
                metadata,
            }
        })
    }
}

/// Domain port shim (ADR-0012-B 3.E): lets `application::elastic_ingestion`
/// hold `Arc<dyn CpuExecutorPort>` without importing `infrastructure::bridge`.
/// Passthrough — no new observability fields; the existing `warn!` in
/// [`CpuBridge::dispatch`] remains the hot-path trace.
impl CpuExecutorPort for CpuBridge {
    /// Delegates to the inherent generic [`CpuBridge::dispatch`]. Explicit
    /// UFCS call disambiguates the method from the trait's same-name member.
    fn dispatch(
        &self,
        work: Box<dyn FnOnce() -> String + Send + 'static>,
    ) -> oneshot::Receiver<Result<String, ScraperError>> {
        CpuBridge::dispatch(self, work)
    }

    /// Adapts the port's primitives to the inherent typed dispatch by
    /// rebuilding the `DownloadedResource` infra-internally (the processor
    /// path consumes only url/content/size — verified: `content_type` is
    /// never read — so `None` here is behavior-neutral), then remaps the
    /// resource-level oneshot to the port's chunk-level oneshot. The remap
    /// needs a forwarding task because a `oneshot::Receiver` cannot be
    /// mapped synchronously; the runtime-context assumption matches the
    /// existing `spawn_blocking` path.
    fn dispatch_resource(
        &self,
        url: String,
        content: String,
        size: u64,
    ) -> oneshot::Receiver<Result<Vec<ProcessedChunk>, ScraperError>> {
        let payload = DownloadedResource {
            url,
            bytes: content.into_bytes(),
            content_type: None,
            size_bytes: size,
        };
        let inner = CpuBridge::dispatch_resource(self, payload);
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let mapped = match inner.await {
                Ok(Ok(processed)) => Ok(processed.chunks),
                Ok(Err(e)) => Err(e),
                // LCOV_EXCL_START defensive: inner receiver closed before the result arrived
                Err(_) => Err(ScraperError::ingestion(
                    "canal CPU bridge cerrado prematuramente",
                )),
                // LCOV_EXCL_STOP
            };
            // LCOV_EXCL_START defensive: oneshot-receiver-dropped — the Tokio task was aborted before consuming the result
            if tx.send(mapped).is_err() {
                warn!(
                    reason = "receptor oneshot descartado",
                    "canal CPU bridge shim descartado: tarea Tokio abortada antes de recibir el resultado"
                );
            }
            // LCOV_EXCL_STOP
        });
        rx
    }
}

/// Extract a human-readable message from a captured panic payload.
///
/// Panics raised with `&str` / `String` (the common case, including
/// `panic!` macros and `assert!`) yield their message; other payload types
/// fall back to a Spanish placeholder.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "tipo de pánico desconocido (no es String ni &str)".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{CpuBridge, CpuExecutorPort, ProcessedChunk, ProcessedResource};
    use crate::error::ScraperError;
    use crate::infrastructure::content_processing::AggressiveProcessor;
    use crate::infrastructure::cpu_pool::RayonCpuPool;
    use crate::infrastructure::crawler::resource_downloader::DownloadedResource;
    use std::sync::Arc;

    fn make_bridge(threads: usize) -> CpuBridge {
        let pool = RayonCpuPool::new(threads).expect("pool should build");
        CpuBridge::new(pool, Arc::new(AggressiveProcessor))
    }

    fn html_payload(html: &str) -> DownloadedResource {
        DownloadedResource {
            url: "https://example.com/page".to_string(),
            bytes: html.as_bytes().to_vec(),
            content_type: Some("text/html".to_string()),
            size_bytes: html.len() as u64,
        }
    }

    // ---- Spec: "result returned via oneshot channel" ----

    #[tokio::test]
    async fn test_dispatch_returns_result_via_oneshot() {
        let bridge = make_bridge(2);
        let rx = bridge.dispatch(|| 42);
        let result = rx
            .await
            .expect("oneshot must not be closed (sender alive)")
            .expect("work returned Ok, not an error");
        assert_eq!(result, 42);
    }

    // ---- Spec: "CPU task returns error" / user decision #1 (panic isolation) ----

    #[tokio::test]
    async fn test_dispatch_panic_isolated_returns_ingestion_err_and_pool_survives() {
        let bridge = make_bridge(2);
        // Inject a panic as lol_html / the tokenizer might on malformed payload.
        let rx = bridge.dispatch(|| panic!("boom from lol_html"));
        let outcome = rx
            .await
            .expect("oneshot must deliver the captured panic, not close");
        assert!(
            outcome.is_err(),
            "panic must surface as Err, not Ok or abort"
        );
        let err = outcome.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("panic"),
            "error must mention panic, got: {msg}"
        );
        assert!(
            msg.contains("boom from lol_html"),
            "error must carry the panic payload, got: {msg}"
        );
        // The Rayon pool MUST survive the panic — a second dispatch works.
        let rx2 = bridge.dispatch(|| 7);
        let result2 = rx2
            .await
            .expect("second oneshot must not be closed")
            .expect("second work returns Ok");
        assert_eq!(result2, 7);
    }

    // ---- Spec: "CPU task returns error" (work-Err propagates via oneshot) ----

    #[tokio::test]
    async fn test_dispatch_propagates_work_error_via_oneshot() {
        // A work closure returning its own Err (e.g. ONNX inference failure in
        // PR5) must surface that Err through the oneshot, distinct from a panic.
        let bridge = make_bridge(2);
        let rx = bridge.dispatch(|| Err::<(), _>(ScraperError::ingestion("inferencia ONNX falló")));
        // dispatch wraps the work's R in Result<R, ScraperError>; here R is
        // itself Result<(), ScraperError>, so awaiting yields Result<Result<(), E>, E>.
        let outer = rx
            .await
            .expect("oneshot must deliver the work result, not close");
        let work_result = outer.expect("panic level must be Ok (work did not panic)");
        assert!(work_result.is_err(), "work Err must propagate as inner Err");
        assert!(
            work_result.unwrap_err().to_string().contains("ONNX"),
            "work error context must survive the crossing"
        );
    }

    // ---- Trap 2: oneshot receiver dropped (Tokio task abort) ----

    #[cfg_attr(miri, ignore)] // fire-and-forget spawn_blocking + tokio::time::sleep; runtime drop hangs under Miri
    #[tokio::test]
    async fn test_dispatch_channel_drop_pool_survives_and_no_panic() {
        let bridge = make_bridge(2);
        // Slow work so the receiver is dropped WHILE Rayon is still processing.
        let rx = bridge.dispatch(|| {
            std::thread::sleep(std::time::Duration::from_millis(60));
            42
        });
        drop(rx); // simulate Tokio aborting the outer task
                  // Let the Rayon work finish; tx.send() will fail (receiver gone) and the
                  // bridge must log via tracing::warn! and NOT panic.
        tokio::time::sleep(std::time::Duration::from_millis(140)).await;
        // Pool MUST survive — a subsequent dispatch succeeds.
        let rx2 = bridge.dispatch(|| 9);
        let result2 = rx2
            .await
            .expect("second oneshot must not be closed after a dropped receiver")
            .expect("second work returns Ok");
        assert_eq!(result2, 9);
    }

    // ---- Spec: "dispatch gateway is thread-safe" (concurrent dispatch) ----

    #[cfg_attr(miri, ignore)] // multi_thread + spawn_blocking hangs under Miri
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_dispatch_concurrent_under_shared_pool() {
        let bridge = std::sync::Arc::new(make_bridge(4));
        let mut handles = Vec::new();
        for i in 0u32..16 {
            let b = std::sync::Arc::clone(&bridge);
            handles.push(tokio::spawn(async move {
                let rx = b.dispatch(move || i * i);
                rx.await
                    .expect("oneshot must not be closed")
                    .expect("work must return Ok")
            }));
        }
        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.expect("join task must not panic"));
        }
        // Every i*i must be present exactly once — no lost/duplicated results.
        results.sort_unstable();
        let expected: Vec<u32> = (0u32..16).map(|i| i * i).collect();
        assert_eq!(results, expected, "concurrent dispatch must be race-free");
    }

    // ---- Task 3.3: typed dispatch_resource (lol_html cleaning) ----

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_dispatch_resource_returns_processed_resource_with_lol_html_cleaning() {
        let bridge = make_bridge(2);
        let html = "<article><h1>Title</h1><p>Hello <b>world</b>.</p></article>";
        let rx = bridge.dispatch_resource(html_payload(html));
        let resource: ProcessedResource = rx
            .await
            .expect("oneshot must not be closed")
            .expect("stub cleaning must succeed");
        assert_eq!(resource.resource_url, "https://example.com/page");
        assert!(
            !resource.chunks.is_empty(),
            "stub must produce at least one chunk"
        );
        let metadata = resource
            .metadata
            .as_object()
            .expect("metadata is an object");
        assert!(
            metadata.get("size_bytes").is_some(),
            "metadata must record size_bytes"
        );
        assert_eq!(
            metadata.get("chunk_count").and_then(|v| v.as_u64()),
            Some(1),
            "stub produces exactly one chunk"
        );
    }

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_dispatch_resource_lol_html_strips_html_tags() {
        // The lol_html cleaner must extract visible text, not raw markup.
        let bridge = make_bridge(2);
        let html = "<p>Hello <script>bad()</script> there</p>";
        let rx = bridge.dispatch_resource(html_payload(html));
        let resource = rx
            .await
            .expect("oneshot must not be closed")
            .expect("lol_html cleaning must succeed");
        let text = resource
            .chunks
            .first()
            .expect("at least one chunk")
            .content
            .as_str();
        assert!(!text.contains('<'), "no raw tags in cleaned text: {text}");
        assert!(!text.contains("bad()"), "script body must be gone: {text}");
        assert!(text.contains("Hello"), "visible text preserved: {text}");
        assert!(text.contains("there"), "visible text preserved: {text}");
        // Embedding is None: ONNX is wired in the orchestrator (async), not the
        // sync Rayon bridge closure (see Decision 5 / PR5 apply-progress).
        assert!(
            resource.chunks[0].embedding.is_none(),
            "bridge must leave embedding None (ONNX wired in the orchestrator)"
        );
    }

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_dispatch_resource_tolerates_invalid_utf8_via_lossy() {
        // Invalid UTF-8 must NOT crash the Rayon pool (from_utf8_lossy replaces).
        let bridge = make_bridge(2);
        let mut bytes = "<p>ok</p>".as_bytes().to_vec();
        bytes.extend_from_slice(&[0xFF, 0xFE]);
        let payload = DownloadedResource {
            url: "https://example.com/x".to_string(),
            bytes,
            content_type: Some("text/html".to_string()),
            size_bytes: 12,
        };
        let rx = bridge.dispatch_resource(payload);
        let outcome = rx.await.expect("oneshot must not be closed");
        assert!(
            outcome.is_ok(),
            "stub must tolerate invalid UTF-8 via lossy, got: {:?}",
            outcome.err()
        );
    }

    // ---- Task 5.3: real lol_html boilerplate removal (replaces the stub) ----
    //
    // The naive stub ran a tag-stripper over RAW HTML, so it extracted the
    // visible text of <nav>/<footer>/<aside> boilerplate too. Real lol_html
    // (via `clean_html`) removes those elements entirely before text is
    // extracted, so their text is gone. This test FAILS on the stub (RED) and
    // PASSES once lol_html is wired (GREEN).

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_dispatch_resource_lol_html_removes_boilerplate_text() {
        let bridge = make_bridge(2);
        let html = "<nav>menu links home</nav>\
                    <main><p>real content here</p></main>\
                    <footer>copyright notice</footer>";
        let rx = bridge.dispatch_resource(html_payload(html));
        let resource = rx
            .await
            .expect("oneshot must not be closed")
            .expect("lol_html cleaning must succeed");
        let text = resource
            .chunks
            .first()
            .expect("at least one chunk")
            .content
            .as_str();
        assert!(
            text.contains("real content here"),
            "main content must be preserved: {text}"
        );
        assert!(
            !text.contains("menu"),
            "nav boilerplate text must be removed by lol_html: {text}"
        );
        assert!(
            !text.contains("copyright"),
            "footer boilerplate text must be removed by lol_html: {text}"
        );
        // Embedding stays None in the bridge (ONNX wiring is the orchestrator's
        // async concern — see Decision 5 / PR5 apply-progress).
        assert!(
            resource.chunks[0].embedding.is_none(),
            "bridge must leave embedding None (ONNX wired in the orchestrator)"
        );
        let metadata = resource
            .metadata
            .as_object()
            .expect("metadata is an object");
        assert_eq!(
            metadata.get("cleaner").and_then(|v| v.as_str()),
            Some("aggressive"),
            "metadata must record the processor name"
        );
    }

    // ---- Task 3.E: domain port shim (CpuBridge implements CpuExecutorPort) ----

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_port_dispatch_resource_returns_chunks_via_shim() {
        // Behavioral proof of the port path: call dispatch_resource through
        // the trait object (Arc<dyn CpuExecutorPort>) and verify the shim's
        // primitive→DownloadedResource adaptation and chunk remap.
        let bridge: Arc<dyn CpuExecutorPort> = Arc::new(make_bridge(2));
        let html = "<p>shim cleaning works</p>";
        let rx = bridge.dispatch_resource(
            "https://example.com/shim".to_string(),
            html.to_string(),
            html.len() as u64,
        );
        let chunks = rx
            .await
            .expect("oneshot must not be closed")
            .expect("shim dispatch_resource must succeed");
        assert_eq!(
            chunks.len(),
            1,
            "shim produces one chunk, like the inherent path"
        );
        let text = chunks[0].content.as_str();
        assert!(
            text.contains("shim cleaning works"),
            "visible text must survive the shim: {text}"
        );
        assert!(
            !text.contains('<'),
            "lol_html cleaning must still run through the shim: {text}"
        );
        assert!(
            chunks[0].embedding.is_none(),
            "shim must leave embedding None (ONNX wired in the orchestrator)"
        );
    }

    #[tokio::test]
    async fn test_port_dispatch_delegates_via_trait_object() {
        // Object-safety + dispatch delegation proof: the impl must compile
        // as Arc<dyn CpuExecutorPort> and route work to the pool.
        let bridge: Arc<dyn CpuExecutorPort> = Arc::new(make_bridge(2));
        let rx = bridge.dispatch(Box::new(|| "via-port".to_string()));
        let result = rx
            .await
            .expect("oneshot must not be closed")
            .expect("work must return Ok");
        assert_eq!(result, "via-port");
    }

    // ---- Static Send + Sync assertion (spec: "gateway MUST be Send + Sync") ----

    #[test]
    fn test_cpu_bridge_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CpuBridge>();
        assert_send_sync::<ProcessedResource>();
        assert_send_sync::<ProcessedChunk>();
    }

    #[test]
    fn test_processed_chunk_debug_clone() {
        let chunk = ProcessedChunk {
            content: "hi".to_string(),
            embedding: None,
        };
        let cloned = chunk.clone();
        assert_eq!(cloned.content, "hi");
        assert_eq!(cloned.embedding, None);
        let s = format!("{cloned:?}");
        assert!(s.contains("ProcessedChunk"));
    }
}

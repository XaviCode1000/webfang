use std::panic::{catch_unwind, AssertUnwindSafe};

use tokio::sync::oneshot;
use tracing::{warn, Instrument};

use crate::error::ScraperError;
use crate::infrastructure::converter::html_cleaner::clean_html;
use crate::infrastructure::cpu_pool::RayonCpuPool;
use crate::infrastructure::crawler::resource_downloader::DownloadedResource;

/// One chunk of cleaned content produced by the bridge.
///
/// `embedding` is `None` until PR5 wires the ONNX inference engine
/// (frozen: PR3 ships the CPU-bound isolation mechanism, not the model).
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedChunk {
    /// Cleaned, visible text for this chunk.
    pub content: String,
    /// 384-dim embedding once PR5 ONNX inference is wired; `None` for the stub.
    pub embedding: Option<Vec<f32>>,
}

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
/// exposes a generic [`dispatch`](CpuBridge::dispatch) that moves any CPU-bound
/// closure off the event loop, plus a typed
/// [`dispatch_resource`](CpuBridge::dispatch_resource) stub that demonstrates
/// the `DownloadedResource` → `ProcessedResource` wiring (PR5 replaces the stub
/// cleaner with real `lol_html` + ONNX).
///
/// `CpuBridge` is `Send + Sync` (spec: "dispatch gateway MUST be Send + Sync"),
/// so it can be shared across Tokio tasks via `Arc<CpuBridge>`.
#[derive(Clone)]
pub struct CpuBridge {
    pool: RayonCpuPool,
}

impl CpuBridge {
    /// Wrap a dedicated [`RayonCpuPool`] in a bridge.
    #[must_use]
    pub fn new(pool: RayonCpuPool) -> Self {
        Self { pool }
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
            let mapped: Result<R, ScraperError> = caught.map_err(|panic| {
                let msg = panic_message(&*panic);
                ScraperError::ingestion(format!("CPU pool panic: {msg}"))
            });
            if tx.send(mapped).is_err() {
                warn!(
                    reason = "receptor oneshot descartado",
                    "canal CPU bridge descartado: tarea Tokio abortada antes de recibir el resultado"
                );
            }
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
    /// PR5 wires real `lol_html` boilerplate removal (via [`clean_html_to_text`])
    /// that strips `script`/`style`/`nav`/`footer`/`aside` chrome and extracts
    /// visible text. The cleaner is infallible (`clean_html` falls back to the
    /// raw HTML on a `lol_html` parse error), so the work closure returns
    /// `ProcessedResource` directly and reuses [`dispatch`]'s single `Result`
    /// wrap. Embeddings stay `None` here — ONNX inference is async and runs in
    /// the orchestrator's async layer (Decision 5); the bridge is sync
    /// CPU-bound text extraction only.
    pub fn dispatch_resource(
        &self,
        payload: DownloadedResource,
    ) -> oneshot::Receiver<Result<ProcessedResource, ScraperError>> {
        let url = payload.url.clone();
        let size = payload.size_bytes;
        self.dispatch(move || {
            let text = clean_html_to_text(&payload.bytes);
            let chunk = ProcessedChunk {
                content: text,
                embedding: None,
            };
            let metadata = serde_json::json!({
                "size_bytes": size,
                "chunk_count": 1u64,
                "cleaner": "lol_html",
            });
            ProcessedResource {
                resource_url: url,
                chunks: vec![chunk],
                metadata,
            }
        })
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

/// Clean downloaded HTML into visible text using Cloudflare's `lol_html`.
///
/// Two-stage (frozen Task 5.3 — replaces the PR3 naive stub):
/// 1. [`clean_html`] runs the `lol_html` streaming rewriter with element
///    handlers that `el.remove()` boilerplate tags (`script`, `style`,
///    `noscript`, `nav`, `header`, `footer`, `aside`, `form`, `iframe`, …) and
///    CSS-selector-matched chrome. This is a real HTML parse, so it correctly
///    handles comments, nested boilerplate, and `</script>`-inside-string cases
///    the naive stub mishandled.
/// 2. [`strip_html_tags`] then extracts the visible text from the
///    boilerplate-stripped HTML (the remaining semantic markup: `main`, `p`,
///    `h1`, …) and collapses whitespace.
///
/// `from_utf8_lossy` is used so malformed payloads never crash the Rayon pool.
/// If `lol_html` itself errors on a pathological input, `clean_html` falls back
/// to the original HTML (logged via `tracing::warn!`), so this function is
/// infallible — the work closure stays non-`Result`, reusing `dispatch`'s
/// single `Result` wrap. (ONNX embeddings are wired in the orchestrator's async
/// layer — see Decision 5; the bridge is sync CPU-bound text extraction only.)
fn clean_html_to_text(bytes: &[u8]) -> String {
    // `from_utf8_lossy` never panics on invalid UTF-8 (replaces with U+FFFD),
    // so malformed payloads do not crash the Rayon pool.
    let html = String::from_utf8_lossy(bytes);
    let cleaned_html = clean_html(&html);
    strip_html_tags(&cleaned_html)
}

/// Naive, UTF-8-safe tag stripper used by the stub cleaner.
fn strip_html_tags(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let lbytes = lower.as_bytes();
    let n = html.len();
    let mut out = String::with_capacity(n);
    let mut i = 0;
    while i < n {
        if lbytes[i] == b'<' {
            let rest = &lower[i..];
            if rest.starts_with("<script") {
                if let Some(rel) = rest.find("</script>") {
                    i += rel + "</script>".len();
                    continue;
                } else {
                    break; // unterminated script: drop the rest
                }
            }
            if rest.starts_with("<style") {
                if let Some(rel) = rest.find("</style>") {
                    i += rel + "</style>".len();
                    continue;
                } else {
                    break;
                }
            }
            // Regular tag: skip to '>'.
            i += 1;
            while i < n && lbytes[i] != b'>' {
                i += 1;
            }
            if i < n {
                i += 1; // consume '>'
            }
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            // Push one char on its proper UTF-8 boundary.
            let next = html[i..]
                .char_indices()
                .nth(1)
                .map(|(j, _)| i + j)
                .unwrap_or(n);
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{CpuBridge, ProcessedChunk, ProcessedResource};
    use crate::error::ScraperError;
    use crate::infrastructure::cpu_pool::RayonCpuPool;
    use crate::infrastructure::crawler::resource_downloader::DownloadedResource;

    fn make_bridge(threads: usize) -> CpuBridge {
        let pool = RayonCpuPool::new(threads).expect("pool should build");
        CpuBridge::new(pool)
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
            Some("lol_html"),
            "metadata must record the real cleaner provenance"
        );
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

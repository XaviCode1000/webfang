//! CPU executor port — domain gateway for Tokio→Rayon crossing.
//!
//! Defines the [`CpuExecutorPort`] trait (`dispatch` + `dispatch_resource`)
//! and the [`ProcessedChunk`] DTO so `application::elastic_ingestion` depends
//! on this port instead of `infrastructure::bridge` (ADR-0012-B 3.E).
//!
//! # Third-party types in `domain/` — accepted deliberately
//!
//! This module puts one third-party type into the domain layer:
//! [`tokio::sync::oneshot::Receiver`] (the response half of both port
//! methods' request-reply contract). It follows the precedent disclosed by
//! the [`downloader_factory`](crate::domain::downloader_factory) and
//! [`ssrf_guard`](crate::domain::ssrf_guard) modules.
//!
//! This is a known, accepted leak, not an oversight: the ADR-0010 intra-crate
//! direction gate (`scripts/check_intra_crate_direction.sh`) only inspects
//! `crate::<layer>::…` paths, so it cannot see third-party leakage at all. A
//! join-handle alternative would force `Future` into the trait; `oneshot` is
//! the minimal async seam for the Tokio→Rayon crossing (ADR-0012-B §1.4).
//! Do not read a green gate as "the domain layer is framework-free".

use crate::error::ScraperError;
use tokio::sync::oneshot;

/// One chunk of cleaned content produced by the CPU executor.
///
/// Domain-owned DTO (ADR-0012-B 3.E); `infrastructure::bridge` re-exports it
/// so infra-internal consumers keep compiling unchanged.
///
/// `embedding` is `None` until the orchestrator's async ONNX layer wires
/// inference (Decision 5); the sync CPU bridge ships text extraction only.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedChunk {
    /// Cleaned, visible text for this chunk.
    pub content: String,
    /// 384-dim embedding once ONNX inference is wired; `None` until then.
    pub embedding: Option<Vec<f32>>,
}

/// Domain port for dispatching CPU-bound closures off the Tokio event loop.
///
/// Dyn-compatible: `dispatch` takes a boxed closure that returns a `String`
/// (the common output of `clean_html` / text conversion). For generic work
/// callers can use the concrete `CpuBridge` directly; the port exists so
/// `application::elastic_ingestion` does not import `infrastructure::bridge`.
pub trait CpuExecutorPort: Send + Sync {
    /// Dispatch a CPU-bound closure and return a oneshot holding the result.
    fn dispatch(
        &self,
        work: Box<dyn FnOnce() -> String + Send + 'static>,
    ) -> oneshot::Receiver<Result<String, ScraperError>>;

    /// Dispatch a downloaded resource — reduced to the primitives the CPU
    /// cleaner actually consumes: URL, already-decoded text content, and
    /// payload size — and return a oneshot holding the cleaned chunks.
    ///
    /// The primitive payload is the minimal set verified against the bridge's
    /// processor path: neither `content_type` nor raw bytes are read there
    /// (`String::from_utf8_lossy` is a boundary concern, applied before
    /// dispatch). Resource-level metadata other than the chunks stays
    /// infra-internal.
    fn dispatch_resource(
        &self,
        url: String,
        content: String,
        size: u64,
    ) -> oneshot::Receiver<Result<Vec<ProcessedChunk>, ScraperError>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct FakeCpu;

    impl CpuExecutorPort for FakeCpu {
        fn dispatch(
            &self,
            work: Box<dyn FnOnce() -> String + Send + 'static>,
        ) -> oneshot::Receiver<Result<String, ScraperError>> {
            let (tx, rx) = oneshot::channel();
            let out = work();
            let _ = tx.send(Ok(out));
            rx
        }

        fn dispatch_resource(
            &self,
            url: String,
            content: String,
            size: u64,
        ) -> oneshot::Receiver<Result<Vec<ProcessedChunk>, ScraperError>> {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Ok(vec![ProcessedChunk {
                content: format!("{url}:{content}:{size}"),
                embedding: None,
            }]));
            rx
        }
    }

    #[tokio::test]
    async fn dispatch_returns_string_via_oneshot() {
        let cpu = FakeCpu;
        let rx = cpu.dispatch(Box::new(|| "hello".to_string()));
        let res = rx.await.expect("oneshot").expect("ok");
        assert_eq!(res, "hello");

        let rx2 = cpu.dispatch(Box::new(|| "world".to_string()));
        assert_eq!(rx2.await.unwrap().unwrap(), "world");
    }

    #[tokio::test]
    async fn dispatch_different_closures() {
        let cpu = FakeCpu;
        let rx = cpu.dispatch(Box::new(|| format!("{}-{}", "a", "b")));
        assert_eq!(rx.await.unwrap().unwrap(), "a-b");
    }

    #[tokio::test]
    async fn dispatch_resource_returns_processed_chunks_via_oneshot() {
        let cpu = FakeCpu;
        let rx = cpu.dispatch_resource(
            "https://example.com".to_string(),
            "clean text".to_string(),
            42,
        );
        let chunks = rx.await.expect("oneshot must not be closed").expect("ok");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "https://example.com:clean text:42");
        assert!(chunks[0].embedding.is_none());
    }

    #[test]
    fn cpu_executor_is_object_safe() {
        fn assert_dyn(_: &dyn CpuExecutorPort) {}
        assert_dyn(&FakeCpu);
        let _: Arc<dyn CpuExecutorPort> = Arc::new(FakeCpu);
    }
}

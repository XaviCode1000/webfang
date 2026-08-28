//! CPU executor port — domain gateway for Tokio→Rayon crossing.

use crate::error::ScraperError;
use tokio::sync::oneshot;

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

    #[test]
    fn cpu_executor_is_object_safe() {
        fn assert_dyn(_: &dyn CpuExecutorPort) {}
        assert_dyn(&FakeCpu);
        let _: Arc<dyn CpuExecutorPort> = Arc::new(FakeCpu);
    }
}

//! Progress observer implementations for decoupled progress reporting.
//!
//! Provides concrete implementations of the `ProgressObserver` trait
//! (defined in `domain::ports`) that handle the channel/quiet logic
//! internally, so callers only need a single one-liner per event.

use crate::domain::entities::progress::{ScrapeError, ScrapeProgress, ScrapeStatus};
pub use crate::domain::ports::ProgressObserver;

/// Live observer that forwards events through an optional `UnboundedSender`.
///
/// When `tx` is `Some`, events are sent through the channel.
/// When `tx` is `None` (non-TUI mode), events are written to stderr.
/// When `quiet` is `true`, all methods become no-ops.
pub struct LiveProgressObserver {
    tx: Option<tokio::sync::mpsc::UnboundedSender<ScrapeProgress>>,
    quiet: bool,
}

impl LiveProgressObserver {
    /// Create a new live observer.
    ///
    /// If `tx` is `None` and `quiet` is `false`, events are written to stderr.
    /// If `quiet` is `true`, all events are suppressed.
    pub fn new(
        tx: Option<tokio::sync::mpsc::UnboundedSender<ScrapeProgress>>,
        quiet: bool,
    ) -> Self {
        Self { tx, quiet }
    }
}

impl ProgressObserver for LiveProgressObserver {
    fn on_page_started<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx.send(ScrapeProgress::Started {
                    url: url.to_string(),
                });
            } else {
                eprintln!("Status: {url} [Started]");
            }
        })
    }

    fn on_status_changed<'a>(
        &'a self,
        url: &'a str,
        status: ScrapeStatus,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx.send(ScrapeProgress::StatusChanged {
                    url: url.to_string(),
                    status,
                });
            } else {
                eprintln!("Status: {url} [{status:?}]");
            }
        })
    }

    fn on_page_completed<'a>(
        &'a self,
        url: &'a str,
        chars: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx.send(ScrapeProgress::Completed {
                    url: url.to_string(),
                    chars,
                });
            } else {
                eprintln!("Status: {url} [Completed, {chars} chars]");
            }
        })
    }

    fn on_page_failed<'a>(
        &'a self,
        url: &'a str,
        error: &'a ScrapeError,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx.send(ScrapeProgress::Failed {
                    url: url.to_string(),
                    error: error.clone(),
                });
            } else {
                eprintln!("Status: {url} [Failed: {error}]");
            }
        })
    }

    fn on_finished<'a>(
        &'a self,
        total: usize,
        successful: usize,
        failed: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx.send(ScrapeProgress::Finished {
                    total,
                    successful,
                    failed,
                });
            } else {
                eprintln!("Finished: {total} total, {successful} succeeded, {failed} failed");
            }
        })
    }

    fn on_robots_blocked<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx.send(ScrapeProgress::Failed {
                    url: url.to_string(),
                    error: ScrapeError::Other("blocked by robots.txt".into()),
                });
            } else {
                eprintln!("Status: {url} [Blocked by robots.txt]");
            }
        })
    }
}

/// No-op observer for dry-run/quiet mode.
pub struct NoopObserver;

impl ProgressObserver for NoopObserver {
    fn on_page_started<'a>(
        &'a self,
        _url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_status_changed<'a>(
        &'a self,
        _url: &'a str,
        _status: ScrapeStatus,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_page_completed<'a>(
        &'a self,
        _url: &'a str,
        _chars: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_page_failed<'a>(
        &'a self,
        _url: &'a str,
        _error: &'a ScrapeError,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_finished<'a>(
        &'a self,
        _total: usize,
        _successful: usize,
        _failed: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_robots_blocked<'a>(
        &'a self,
        _url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_observer_sends_started_when_not_quiet() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let observer = LiveProgressObserver::new(Some(tx), false);

        observer.on_page_started("https://example.com").await;

        let msg = rx.recv().await.expect("should receive message");
        assert!(
            matches!(msg, ScrapeProgress::Started { ref url } if url == "https://example.com"),
            "expected Started event"
        );
    }

    #[tokio::test]
    async fn live_observer_suppresses_when_quiet() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let observer = LiveProgressObserver::new(Some(tx), true);

        observer.on_page_started("https://example.com").await;
        observer.on_page_completed("https://example.com", 100).await;
        observer
            .on_page_failed("https://example.com", &ScrapeError::Other("test".into()))
            .await;
        observer.on_finished(1, 0, 1).await;

        assert!(
            rx.try_recv().is_err(),
            "quiet mode should suppress all events"
        );
    }

    #[tokio::test]
    async fn live_observer_noop_when_no_tx() {
        let observer = LiveProgressObserver::new(None, false);

        observer.on_page_started("https://example.com").await;
        observer.on_page_completed("https://example.com", 100).await;
        observer
            .on_page_failed("https://example.com", &ScrapeError::Other("test".into()))
            .await;
        observer.on_finished(1, 0, 1).await;
        observer
            .on_robots_blocked("https://example.com/robots")
            .await;
    }

    #[tokio::test]
    async fn live_observer_sends_completed_with_chars() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let observer = LiveProgressObserver::new(Some(tx), false);

        observer.on_page_completed("https://example.com", 42).await;

        let msg = rx.recv().await.expect("should receive message");
        assert!(
            matches!(msg, ScrapeProgress::Completed { ref url, chars } if url == "https://example.com" && chars == 42),
            "expected Completed event with chars"
        );
    }

    #[tokio::test]
    async fn live_observer_sends_finished_counts() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let observer = LiveProgressObserver::new(Some(tx), false);

        observer.on_finished(10, 8, 2).await;

        let msg = rx.recv().await.expect("should receive message");
        assert!(
            matches!(
                msg,
                ScrapeProgress::Finished {
                    total: 10,
                    successful: 8,
                    failed: 2
                }
            ),
            "expected Finished with correct counts"
        );
    }

    #[tokio::test]
    async fn live_observer_sends_robots_blocked_as_failed() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let observer = LiveProgressObserver::new(Some(tx), false);

        observer
            .on_robots_blocked("https://example.com/blocked")
            .await;

        let msg = rx.recv().await.expect("should receive message");
        assert!(
            matches!(msg, ScrapeProgress::Failed { ref url, ref error } if url == "https://example.com/blocked" && matches!(error, ScrapeError::Other(s) if s == "blocked by robots.txt")),
            "expected Failed event for robots block"
        );
    }

    #[tokio::test]
    async fn noop_observer_is_silent() {
        let observer = NoopObserver;

        observer.on_page_started("https://example.com").await;
        observer.on_page_completed("https://example.com", 100).await;
        observer
            .on_page_failed("https://example.com", &ScrapeError::Other("test".into()))
            .await;
        observer.on_finished(1, 0, 1).await;
        observer
            .on_robots_blocked("https://example.com/robots")
            .await;
    }
}

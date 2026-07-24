//! Progress observer trait for decoupled progress reporting.
//!
//! Provides a trait abstraction over the raw `mpsc::Sender<ScrapeProgress>` channel,
//! eliminating boilerplate `if !quiet { if let Some(tx) = ... }` patterns.

use std::future::Future;
use std::pin::Pin;

use crate::application::progress_types::{ScrapeError, ScrapeProgress};

/// Trait for observing scraping progress events.
///
/// Implementations handle the quiet/channel logic internally, so callers
/// only need a single one-liner per event.
///
/// Methods are desugared to `Pin<Box<dyn Future<…> + Send + '_>>` so the
/// trait is dyn-compatible without the `async_trait` crate, matching the
/// pattern in `domain::repository::VectorRepository`.
pub trait ProgressObserver: Send + Sync {
    /// A page scrape has started.
    fn on_page_started<'a>(&'a self, url: &'a str)
        -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// A page scrape completed successfully.
    fn on_page_completed<'a>(
        &'a self,
        url: &'a str,
        chars: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// A page scrape failed.
    fn on_page_failed<'a>(
        &'a self,
        url: &'a str,
        error: &'a ScrapeError,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// All URLs have been processed.
    fn on_finished<'a>(
        &'a self,
        total: usize,
        successful: usize,
        failed: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// A URL was blocked by robots.txt.
    fn on_robots_blocked<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Live observer that forwards events through an optional `mpsc::Sender`.
///
/// Respects the `quiet` flag — when `true`, no events are emitted.
pub struct LiveProgressObserver {
    tx: Option<tokio::sync::mpsc::Sender<ScrapeProgress>>,
    quiet: bool,
}

impl LiveProgressObserver {
    /// Create a new live observer.
    ///
    /// If `tx` is `None` or `quiet` is `true`, all methods become no-ops.
    pub fn new(tx: Option<tokio::sync::mpsc::Sender<ScrapeProgress>>, quiet: bool) -> Self {
        Self { tx, quiet }
    }
}

impl ProgressObserver for LiveProgressObserver {
    fn on_page_started<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx
                    .send(ScrapeProgress::Started {
                        url: url.to_string(),
                    })
                    .await;
            }
        })
    }

    fn on_page_completed<'a>(
        &'a self,
        url: &'a str,
        chars: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx
                    .send(ScrapeProgress::Completed {
                        url: url.to_string(),
                        chars,
                    })
                    .await;
            }
        })
    }

    fn on_page_failed<'a>(
        &'a self,
        url: &'a str,
        error: &'a ScrapeError,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx
                    .send(ScrapeProgress::Failed {
                        url: url.to_string(),
                        error: error.clone(),
                    })
                    .await;
            }
        })
    }

    fn on_finished<'a>(
        &'a self,
        total: usize,
        successful: usize,
        failed: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx
                    .send(ScrapeProgress::Finished {
                        total,
                        successful,
                        failed,
                    })
                    .await;
            }
        })
    }

    fn on_robots_blocked<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.quiet {
                return;
            }
            if let Some(ref tx) = self.tx {
                let _ = tx
                    .send(ScrapeProgress::Failed {
                        url: url.to_string(),
                        error: ScrapeError::Other("blocked by robots.txt".into()),
                    })
                    .await;
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
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_page_completed<'a>(
        &'a self,
        _url: &'a str,
        _chars: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_page_failed<'a>(
        &'a self,
        _url: &'a str,
        _error: &'a ScrapeError,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_finished<'a>(
        &'a self,
        _total: usize,
        _successful: usize,
        _failed: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn on_robots_blocked<'a>(
        &'a self,
        _url: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_observer_sends_started_when_not_quiet() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
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
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
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
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
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
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
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
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
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

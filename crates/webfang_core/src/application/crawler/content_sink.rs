//! Page-body capture during a crawl.
//!
//! The crawl [`Engine`](super::engine::Engine) discards page bodies after link
//! extraction: [`CrawlResult`](crate::domain::CrawlResult) is metadata only
//! (URLs, counters). Batch mode therefore had no content to export, so
//! `--batch` reported success while writing zero files (#631) and silently
//! ignored `--elastic` / `--output-vectors` / `--resume` (#637).
//!
//! A [`CrawlContentSink`] lets a caller observe every fetched body without a
//! second HTTP round-trip. The CLI collects the bodies, converts them to
//! [`ScrapedContent`](crate::domain::ScrapedContent) via
//! [`extract_content`](super::discovery::extract_content), and then runs the
//! exact same export / vector-ingestion pipeline as single-page mode.

use std::sync::Mutex;

/// A page body captured mid-crawl, before extraction.
///
/// Serializable so a disk-backed sink
/// ([`BoundedFileSink`](super::bounded_sink::BoundedFileSink)) can spool it as
/// one JSONL record per page instead of buffering the whole batch in RAM.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CapturedPage {
    /// Absolute URL the body was fetched from.
    pub url: String,
    /// Raw response body as received from the fetch layer.
    pub html: String,
}

/// Receives every page body fetched by the crawl engine.
///
/// Implementations MUST be cheap and non-blocking: `capture` runs inline on the
/// per-page worker task, so it must never block the async runtime and never
/// hold a lock across an `.await` (it is synchronous by design).
pub trait CrawlContentSink: Send + Sync {
    /// Record the body fetched for `url`.
    fn capture(&self, url: &str, html: &str);
}

/// Thread-safe in-memory [`CrawlContentSink`].
///
/// Batch mode runs several engines concurrently against one shared sink, so
/// the backing buffer is behind a [`Mutex`]. The critical section is a single
/// `Vec::push` with no `.await` inside, so it cannot block the runtime.
#[derive(Debug, Default)]
pub struct InMemoryContentSink {
    pages: Mutex<Vec<CapturedPage>>,
}

impl InMemoryContentSink {
    /// Create an empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drain every captured page, leaving the sink empty.
    ///
    /// A poisoned mutex (a worker panicked mid-`capture`) degrades to the
    /// recovered buffer rather than panicking: losing the crawl's content on a
    /// single worker panic would reintroduce the silent data loss of #631.
    #[must_use]
    pub fn take_pages(&self) -> Vec<CapturedPage> {
        match self.pages.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }

    /// Number of pages captured so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pages
            .lock()
            .map_or_else(|poisoned| poisoned.into_inner().len(), |guard| guard.len())
    }

    /// Whether no page has been captured yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl CrawlContentSink for InMemoryContentSink {
    fn capture(&self, url: &str, html: &str) {
        let page = CapturedPage {
            url: url.to_string(),
            html: html.to_string(),
        };
        match self.pages.lock() {
            Ok(mut guard) => guard.push(page),
            Err(poisoned) => poisoned.into_inner().push(page),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn new_sink_is_empty() {
        let sink = InMemoryContentSink::new();
        assert!(sink.is_empty());
        assert_eq!(sink.len(), 0);
    }

    #[test]
    fn capture_records_url_and_body() {
        let sink = InMemoryContentSink::new();
        sink.capture("https://example.com/", "<html>hi</html>");

        let pages = sink.take_pages();
        assert_eq!(
            pages,
            vec![CapturedPage {
                url: "https://example.com/".to_string(),
                html: "<html>hi</html>".to_string(),
            }]
        );
    }

    #[test]
    fn take_pages_drains_the_buffer() {
        let sink = InMemoryContentSink::new();
        sink.capture("https://example.com/a", "<p>a</p>");
        sink.capture("https://example.com/b", "<p>b</p>");

        assert_eq!(sink.take_pages().len(), 2);
        assert!(sink.take_pages().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_captures_are_all_recorded() {
        let sink = Arc::new(InMemoryContentSink::new());
        let mut tasks = tokio::task::JoinSet::new();

        for i in 0..32 {
            let sink = Arc::clone(&sink);
            tasks.spawn(async move {
                sink.capture(&format!("https://example.com/{i}"), "<p>body</p>");
            });
        }
        while tasks.join_next().await.is_some() {}

        assert_eq!(sink.take_pages().len(), 32);
    }
}

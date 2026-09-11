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

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// A capture sink carries no observable state at the trait-object level, so
/// there is nothing better to print. Same shape as
/// `impl fmt::Debug for dyn DownloaderFactory`: lets option structs holding
/// an `Arc<dyn CrawlContentSink>` (like `EngineOptions`) derive `Debug`.
impl fmt::Debug for dyn CrawlContentSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dyn CrawlContentSink")
    }
}

/// Default byte budget for [`InMemoryContentSink`] (8 MiB).
///
/// Discovery capture is a throughput path: real pages average tens of KiB, so
/// 8 MiB holds hundreds of pages while keeping the resident set bounded.
/// Callers that need less (tests) or more use
/// [`InMemoryContentSink::with_max_bytes`].
pub const DEFAULT_CAPTURE_BYTE_CAP: usize = 8 * 1024 * 1024;

/// Thread-safe in-memory [`CrawlContentSink`].
///
/// Batch mode runs several engines concurrently against one shared sink, so
/// the backing buffer is behind a [`Mutex`]. The critical section is a single
/// check-and-push with no `.await` inside, so it cannot block the runtime.
///
/// The buffer is byte-BOUNDED (F-05, #1229, ruling c): once admitting another
/// page would exceed `max_bytes`, the sink latches a sticky capped flag,
/// emits one English `tracing::warn!` (numeric fields only — never URL data),
/// and discards every further page. The consumer then falls back to a normal
/// re-fetch for uncaptured URLs: correctness is preserved, memory is proven
/// bounded. [`BoundedFileSink`](super::bounded_sink::BoundedFileSink) needs no
/// change — the [`CrawlContentSink`] trait itself is untouched.
#[derive(Debug)]
pub struct InMemoryContentSink {
    state: Mutex<CaptureState>,
    /// Byte budget; admitting a page beyond it trips the cap.
    max_bytes: usize,
    /// Sticky trip flag: set once the cap trips, read without locking.
    capped: AtomicBool,
}

/// Captured pages plus their byte accounting, guarded by one lock so the
/// admit-check and the push are atomic with respect to other workers.
#[derive(Debug, Default)]
struct CaptureState {
    pages: Vec<CapturedPage>,
    /// Sum of `url.len() + html.len()` over held pages.
    bytes_held: usize,
}

impl InMemoryContentSink {
    /// Create an empty sink with the default byte budget.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty sink with an explicit byte budget.
    ///
    /// A zero budget captures nothing (every admit trips the cap); the
    /// default budget is [`DEFAULT_CAPTURE_BYTE_CAP`].
    #[must_use]
    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            state: Mutex::new(CaptureState::default()),
            max_bytes,
            capped: AtomicBool::new(false),
        }
    }

    /// Whether the byte cap has tripped (further captures are discarded).
    #[must_use]
    pub fn cap_exceeded(&self) -> bool {
        self.capped.load(Ordering::Relaxed)
    }

    /// Bytes currently held (`url.len() + html.len()` over held pages).
    #[must_use]
    pub fn bytes_held(&self) -> usize {
        self.state.lock().map_or_else(
            |poisoned| poisoned.into_inner().bytes_held,
            |state| state.bytes_held,
        )
    }

    /// Drain every captured page, leaving the sink empty.
    ///
    /// Byte accounting resets with the drain; the sticky cap flag does NOT —
    /// a tripped sink stays tripped so the one warn keeps its meaning.
    ///
    /// A poisoned mutex (a worker panicked mid-`capture`) degrades to the
    /// recovered buffer rather than panicking: losing the crawl's content on a
    /// single worker panic would reintroduce the silent data loss of #631.
    #[must_use]
    pub fn take_pages(&self) -> Vec<CapturedPage> {
        match self.state.lock() {
            Ok(mut state) => {
                state.bytes_held = 0;
                std::mem::take(&mut state.pages)
            },
            Err(poisoned) => {
                let state = &mut *poisoned.into_inner();
                state.bytes_held = 0;
                std::mem::take(&mut state.pages)
            },
        }
    }

    /// Number of pages captured so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.lock().map_or_else(
            |poisoned| poisoned.into_inner().pages.len(),
            |state| state.pages.len(),
        )
    }

    /// Whether no page has been captured yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for InMemoryContentSink {
    fn default() -> Self {
        Self::with_max_bytes(DEFAULT_CAPTURE_BYTE_CAP)
    }
}

impl CrawlContentSink for InMemoryContentSink {
    fn capture(&self, url: &str, html: &str) {
        // Fast path: a tripped sink discards without locking.
        if self.capped.load(Ordering::Relaxed) {
            return;
        }
        let incoming = url.len().saturating_add(html.len());
        // The admit-check and the push share one critical section (no `.await`
        // inside) so concurrent workers cannot overshoot the cap. A poisoned
        // mutex degrades to the recovered buffer, as `take_pages` does.
        let guard = self.state.lock();
        let mut state = match guard {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Re-check under the lock: a worker may have tripped the cap while
        // this one waited for the guard.
        if self.capped.load(Ordering::Relaxed) {
            return;
        }
        if state.bytes_held.saturating_add(incoming) > self.max_bytes {
            self.capped.store(true, Ordering::Relaxed);
            let held = state.bytes_held;
            let pages = state.pages.len();
            drop(state);
            // English log, numeric structured fields only — the URL is
            // sensitive data and must never land in a log field.
            tracing::warn!(
                cap_bytes = self.max_bytes,
                held_bytes = held,
                captured_pages = pages,
                incoming_bytes = incoming,
                "content capture byte cap exceeded — stopping capture, uncaptured pages fall back to re-fetch"
            );
            return;
        }
        state.bytes_held = state.bytes_held.saturating_add(incoming);
        state.pages.push(CapturedPage {
            url: url.to_string(),
            html: html.to_string(),
        });
    }
}

/// Convert one [`CapturedPage`] into [`ScrapedContent`] through the exact
/// extraction pipeline the CLI batch/export phases use: Readability → text
/// fallback → binary detection (via [`extract_content`]).
///
/// This is the SINGLE page→content conversion path shared by CLI and MCP
/// (#1290, P6-2/F-16): both surfaces feeding the same captured page here
/// produce the same enriched DTO, and therefore the same `WebfangMetadata`
/// JSONL records (checksum, `word_count`, timestamps, `metadata_version`).
/// Asset downloader and adaptive-selector engine are `None`, matching the
/// batch crawl behavior — one fetch per URL, equivalent to `--no-images` /
/// `--no-documents`.
///
/// Errors come back paired with the offending URL instead of aborting the
/// batch: callers collect per-page failures exactly as `extract_batch_content`
/// does today, so a single bad page never loses the run's good records.
///
/// [`extract_content`]: crate::application::extraction::extract_content
/// [`ScrapedContent`]: crate::domain::ScrapedContent
pub async fn extract_page_content(
    page: &CapturedPage,
    config: &crate::domain::config::ScraperConfig,
    page_correlation: &crate::domain::CorrelationId,
) -> Result<crate::domain::ScrapedContent, (String, crate::error::ScraperError)> {
    let url = match url::Url::parse(&page.url) {
        Ok(u) => u,
        Err(e) => {
            return Err((
                page.url.clone(),
                crate::error::ScraperError::invalid_url(format!(
                    "No se pudo parsear la URL capturada: {e}",
                )),
            ));
        },
    };
    match crate::application::extraction::extract_content(
        &page.html,
        &url,
        config,
        None,
        None,
        page_correlation,
    )
    .await
    {
        Ok(content) => Ok(content),
        Err(e) => Err((page.url.clone(), e)),
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
    fn default_cap_is_eight_mib() {
        assert_eq!(DEFAULT_CAPTURE_BYTE_CAP, 8 * 1024 * 1024);
    }

    #[test]
    fn capture_stops_at_byte_cap_and_latches() {
        let sink = InMemoryContentSink::with_max_bytes(10);
        sink.capture("u", "12345"); // 1 + 5 = 6 bytes: admitted
        assert_eq!(sink.len(), 1);
        assert!(!sink.cap_exceeded());

        sink.capture("v", "123456789"); // 6 + 10 > 10: trips the cap
        assert!(sink.cap_exceeded());
        assert_eq!(sink.len(), 1, "tripped capture must be discarded");

        sink.capture("w", "x"); // post-trip: discarded without locking
        assert_eq!(sink.len(), 1);
    }

    #[test]
    fn take_pages_drains_and_resets_byte_accounting() {
        let sink = InMemoryContentSink::with_max_bytes(100);
        sink.capture("https://example.com/", "<p>hi</p>");
        assert!(sink.bytes_held() > 0);

        let pages = sink.take_pages();
        assert_eq!(pages.len(), 1);
        assert_eq!(sink.bytes_held(), 0);
        assert!(sink.is_empty());
    }

    #[test]
    fn zero_budget_captures_nothing() {
        let sink = InMemoryContentSink::with_max_bytes(0);
        sink.capture("https://example.com/", "<p>hi</p>");
        assert!(sink.is_empty());
        assert!(sink.cap_exceeded());
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

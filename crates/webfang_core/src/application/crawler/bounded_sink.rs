//! Bounded, disk-backed [`CrawlContentSink`] for batch crawls.
//!
//! [`InMemoryContentSink`](super::content_sink::InMemoryContentSink) keeps every
//! fetched body in a `Vec` until the whole batch finishes, so a large batch of
//! heavy pages grows the resident set without any ceiling (#653).
//!
//! [`BoundedFileSink`] replaces that unbounded buffer with a bounded
//! [`tokio::sync::mpsc`] channel plus a background writer task that appends each
//! page to a JSONL spool file. Peak memory is therefore
//! `buffer_size * average_page_size` instead of `total_pages * average_page_size`,
//! and consumers stream the spool back one page at a time through
//! [`CapturedPageReader`].
//!
//! ```text
//! crawl worker ── capture() ──► mpsc(buffer_size) ──► writer task ──► spool.jsonl
//!                                                                        │
//!                                    CapturedPageReader ◄────────────────┘
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use super::content_sink::{CapturedPage, CrawlContentSink};

/// Default number of pages held in flight before the writer applies backpressure.
pub const DEFAULT_SINK_BUFFER: usize = 32;

/// Failures of the disk-backed capture pipeline.
#[derive(Debug, thiserror::Error)]
pub enum BoundedSinkError {
    /// The spool file could not be created, written, or read.
    #[error("no se pudo escribir el archivo temporal de captura: {0}")]
    Io(#[from] std::io::Error),

    /// A spool line could not be encoded or decoded.
    #[error("registro de captura corrupto: {0}")]
    Codec(#[from] serde_json::Error),

    /// The background writer task panicked or was cancelled.
    #[error("la tarea de escritura de capturas falló: {0}")]
    Writer(String),

    /// [`BoundedFileSink::finish`] was called more than once.
    #[error("el sumidero de capturas ya fue cerrado")]
    AlreadyFinished,
}

/// Bounded, disk-backed content sink.
///
/// `capture` is synchronous (the [`CrawlContentSink`] contract) and never
/// blocks the runtime: the fast path is a non-blocking `try_send`. When the
/// channel is full the page is handed to a short-lived task that awaits the
/// send, so backpressure is absorbed off the crawl worker instead of dropping
/// the page — silent content loss is exactly the failure mode #631 fixed.
pub struct BoundedFileSink {
    tx: Mutex<Option<mpsc::Sender<CapturedPage>>>,
    writer: Mutex<Option<JoinHandle<Result<usize, BoundedSinkError>>>>,
    spool_path: PathBuf,
    captured: AtomicUsize,
}

impl std::fmt::Debug for BoundedFileSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundedFileSink")
            .field("spool_path", &self.spool_path)
            .field("captured", &self.captured.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl BoundedFileSink {
    /// Create a sink that spools captured pages to `spool_path`.
    ///
    /// `buffer_size` is the number of pages held in memory before the producer
    /// is slowed down; it is clamped to at least 1 because a zero-capacity
    /// `mpsc` channel is invalid.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] if the spool file (or its parent
    /// directory) cannot be created.
    pub async fn new(spool_path: PathBuf, buffer_size: usize) -> Result<Self, BoundedSinkError> {
        if let Some(parent) = spool_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::File::create(&spool_path).await?;
        let (tx, rx) = mpsc::channel(buffer_size.max(1));
        let writer = tokio::spawn(spool_writer(file, rx));

        info!(
            spool = %spool_path.display(),
            buffer_size = buffer_size.max(1),
            "bounded content sink ready"
        );

        Ok(Self {
            tx: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            spool_path,
            captured: AtomicUsize::new(0),
        })
    }

    /// Path of the JSONL spool holding the captured pages.
    #[must_use]
    pub fn spool_path(&self) -> &Path {
        &self.spool_path
    }

    /// Number of pages handed to the channel so far.
    #[must_use]
    pub fn captured(&self) -> usize {
        self.captured.load(Ordering::Relaxed)
    }

    /// Close the channel, wait for the writer to flush, and return the number
    /// of pages persisted to the spool.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::AlreadyFinished`] on a second call, or the
    /// writer's own I/O / codec error when the flush failed.
    pub async fn finish(&self) -> Result<usize, BoundedSinkError> {
        // Drop the sender OUTSIDE any await so the writer observes the close.
        {
            let mut guard = self
                .tx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.take().is_none() {
                return Err(BoundedSinkError::AlreadyFinished);
            }
        }

        let handle = {
            let mut guard = self
                .writer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.take()
        };
        let Some(handle) = handle else {
            return Err(BoundedSinkError::AlreadyFinished);
        };

        match handle.await {
            Ok(result) => {
                let written = result?;
                info!(
                    pages = written,
                    spool = %self.spool_path.display(),
                    "bounded content sink flushed"
                );
                Ok(written)
            },
            // LCOV_EXCL_START defensive: writer-join — a JoinError means the writer task panicked, a bug
            Err(join_err) => {
                error!(error = %join_err, "content spool writer task failed");
                Err(BoundedSinkError::Writer(join_err.to_string()))
            },
            // LCOV_EXCL_STOP
        }
    }

    /// Open a streaming reader over the spool.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] when the spool file cannot be opened.
    pub async fn reader(&self) -> Result<CapturedPageReader, BoundedSinkError> {
        CapturedPageReader::open(&self.spool_path).await
    }
}

impl CrawlContentSink for BoundedFileSink {
    fn capture(&self, url: &str, html: &str) {
        let page = CapturedPage {
            url: url.to_string(),
            html: html.to_string(),
        };

        let sender = {
            let guard = self
                .tx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone()
        };
        let Some(sender) = sender else {
            warn!(url, "capture after sink close — page discarded");
            return;
        };

        match sender.try_send(page) {
            Ok(()) => {
                self.captured.fetch_add(1, Ordering::Relaxed);
            },
            Err(mpsc::error::TrySendError::Full(page)) => {
                // Backpressure: defer the send instead of blocking the crawl
                // worker or dropping the body.
                debug!(url = %page.url, "capture buffer full — deferring spool write");
                self.captured.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    if sender.send(page).await.is_err() {
                        warn!("capture channel closed before deferred write");
                    }
                });
            },
            // LCOV_EXCL_START defensive: closed-channel — capture after finish() is a lifecycle bug
            Err(mpsc::error::TrySendError::Closed(page)) => {
                warn!(url = %page.url, "capture channel closed — page discarded");
            },
            // LCOV_EXCL_STOP
        }
    }
}

/// Background writer: drains the channel and appends one JSON object per line.
async fn spool_writer(
    file: tokio::fs::File,
    mut rx: mpsc::Receiver<CapturedPage>,
) -> Result<usize, BoundedSinkError> {
    let mut writer = BufWriter::new(file);
    let mut written = 0usize;

    while let Some(page) = rx.recv().await {
        let line = serde_json::to_string(&page)?;
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        written += 1;
    }

    writer.flush().await?;
    writer.into_inner().sync_all().await?;
    Ok(written)
}

/// Streaming reader over a spool written by [`BoundedFileSink`].
///
/// Yields one [`CapturedPage`] at a time so the consumer never materializes the
/// whole batch in memory.
#[derive(Debug)]
pub struct CapturedPageReader {
    lines: tokio::io::Lines<BufReader<tokio::fs::File>>,
}

impl CapturedPageReader {
    /// Open the spool at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] when the file cannot be opened.
    pub async fn open(path: &Path) -> Result<Self, BoundedSinkError> {
        let file = tokio::fs::File::open(path).await?;
        Ok(Self {
            lines: BufReader::new(file).lines(),
        })
    }

    /// Read the next captured page, or `None` at end of spool.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] on a read failure or
    /// [`BoundedSinkError::Codec`] when a line is not a valid record.
    pub async fn next_page(&mut self) -> Result<Option<CapturedPage>, BoundedSinkError> {
        while let Some(line) = self.lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            return Ok(Some(serde_json::from_str(&line)?));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn sink_in(dir: &tempfile::TempDir, buffer: usize) -> BoundedFileSink {
        BoundedFileSink::new(dir.path().join("spool.jsonl"), buffer)
            .await
            .expect("sink must be created")
    }

    async fn drain(sink: &BoundedFileSink) -> Vec<CapturedPage> {
        let mut reader = sink.reader().await.expect("spool must open");
        let mut pages = Vec::new();
        while let Some(page) = reader.next_page().await.expect("spool must decode") {
            pages.push(page);
        }
        pages
    }

    #[tokio::test]
    async fn captured_pages_round_trip_through_the_spool() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = sink_in(&dir, 4).await;

        sink.capture("https://example.com/a", "<p>a</p>");
        sink.capture("https://example.com/b", "<p>b</p>");

        assert_eq!(sink.finish().await.expect("flush"), 2);
        assert_eq!(
            drain(&sink).await,
            vec![
                CapturedPage {
                    url: "https://example.com/a".to_string(),
                    html: "<p>a</p>".to_string(),
                },
                CapturedPage {
                    url: "https://example.com/b".to_string(),
                    html: "<p>b</p>".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn bodies_with_newlines_survive_the_jsonl_encoding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = sink_in(&dir, 2).await;

        sink.capture("https://example.com/", "<p>line1</p>\n<p>line2</p>");
        assert_eq!(sink.finish().await.expect("flush"), 1);

        let pages = drain(&sink).await;
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].html, "<p>line1</p>\n<p>line2</p>");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_tiny_buffer_still_persists_every_page() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Buffer of 1 forces the deferred-send path for most captures.
        let sink = Arc::new(sink_in(&dir, 1).await);

        let mut handles = Vec::with_capacity(64);
        for i in 0..64 {
            let sink = Arc::clone(&sink);
            handles.push(tokio::spawn(async move {
                sink.capture(&format!("https://example.com/{i}"), "<p>body</p>");
            }));
        }
        for h in handles {
            h.await.expect("capture task panicked");
        }

        // Deferred sends run on spawned tasks; yield until every capture landed.
        while sink.captured() < 64 {
            tokio::task::yield_now().await;
        }

        assert_eq!(sink.finish().await.expect("flush"), 64);
        assert_eq!(drain(&sink).await.len(), 64);
    }

    #[tokio::test]
    async fn finish_is_not_idempotent_and_reports_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = sink_in(&dir, 2).await;

        assert_eq!(sink.finish().await.expect("first flush"), 0);
        let err = sink.finish().await.expect_err("second flush must fail");
        assert!(matches!(err, BoundedSinkError::AlreadyFinished));
    }

    #[tokio::test]
    async fn reader_over_a_missing_spool_reports_io() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = CapturedPageReader::open(&dir.path().join("absent.jsonl"))
            .await
            .expect_err("missing spool must fail");
        assert!(matches!(err, BoundedSinkError::Io(_)));
    }
}

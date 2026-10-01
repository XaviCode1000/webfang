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
//! crawl worker ── capture() ──► mpsc(buffer_size) ─┐
//!                                                 ├─► writer task ──► spool.jsonl
//! crawl worker ── capture() ──► Backlog(max_bytes) ─┘        │
//!                                            CapturedPageReader ◄┘
//! ```
//!
//! # Where boundedness lives (#1616)
//!
//! [`CrawlContentSink::capture`] is **synchronous by contract** — it runs inline
//! on the per-page crawl worker and must not block the runtime. A synchronous
//! producer therefore cannot simply `await` its way through backpressure, and
//! the previous implementation spawned **one deferred task per full
//! `try_send`** to do it. That made the channel bound a lie: 32 bounded the
//! queue, never the number of live tasks, so a spool that could not keep up grew
//! one task — and one retained page body — per capture, forever. The resulting
//! `send().await` was also the only unguarded await in the module, so a writer
//! that stopped draining parked that task indefinitely (audit P0.2 / CC-D3).
//!
//! Both bounds now live **here, in the sink**, and nowhere else:
//!
//! | bound | owned by | limits |
//! |---|---|---|
//! | `buffer_size` | [`mpsc`] channel capacity | pages waiting to be spooled |
//! | `DEFAULT_MAX_BACKLOG_BYTES` | `backlog` | page bytes a full channel defers to |
//!
//! A full channel routes the page into the sink's own byte-bounded
//! `backlog`, which the writer drains alongside the channel. The producer
//! spawns **no task at all**, so the number of in-flight producer tasks is
//! *constant* — zero — no matter how far the writer falls behind, and there is
//! no per-send await left to park. When even the backlog ceiling is reached the
//! page is refused, counted, and reported once (see [`BoundedFileSink::dropped`]),
//! which is the same bounded-and-observed tradeoff
//! [`InMemoryContentSink`](super::content_sink::InMemoryContentSink) already
//! makes for its byte cap: correctness is preserved, memory is proven bounded.

#![deny(clippy::await_holding_lock)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn, Instrument};

use super::content_sink::{CapturedPage, CrawlContentSink};

/// Default number of pages held in flight before the writer applies backpressure.
pub const DEFAULT_SINK_BUFFER: usize = 32;

/// Byte ceiling for pages a full channel defers to the sink's `backlog`.
///
/// This is the second half of the sink's memory bound (#1616). Real pages
/// average tens of KiB, so 8 MiB holds a few hundred pages of slack while
/// still putting a hard, measured ceiling on a spool that cannot keep up — the
/// same order of magnitude, and the same reasoning, as
/// [`InMemoryContentSink`](super::content_sink::DEFAULT_CAPTURE_BYTE_CAP).
///
/// Deliberately **not** derived from the crawl budget tier the way the channel
/// capacity is (`build_batch_sink`, #2.5c): that tier sizes *concurrency*, while
/// this is a *memory* ceiling, which is a property of the machine rather than of
/// the crawl. Keeping it a sink-internal constant is what keeps the bound from
/// being re-decided at every call site.
pub const DEFAULT_MAX_BACKLOG_BYTES: usize = 8 * 1024 * 1024;

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
/// blocks the runtime: the fast path is a non-blocking `try_send`, and a full
/// channel hands the page to the sink's own byte-bounded `backlog` instead of
/// to a task. Backpressure is therefore absorbed without dropping the body
/// (silent content loss is the failure mode #631 fixed) and without letting the
/// producer spawn work it never reclaims (#1616 P0.2).
pub struct BoundedFileSink {
    tx: Mutex<Option<mpsc::Sender<CapturedPage>>>,
    writer: Mutex<Option<JoinHandle<Result<usize, BoundedSinkError>>>>,
    backlog: Arc<Backlog>,
    cancel: CancellationToken,
    spool_path: PathBuf,
    captured: AtomicUsize,
    dropped: AtomicUsize,
    buffer: usize,
}

impl std::fmt::Debug for BoundedFileSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundedFileSink")
            .field("spool_path", &self.spool_path)
            .field("captured", &self.captured.load(Ordering::Relaxed))
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl BoundedFileSink {
    /// Create a sink that spools captured pages to `spool_path`.
    ///
    /// `buffer_size` is the number of pages held in memory before the producer
    /// is slowed down; it is clamped to at least 1 because a zero-capacity
    /// `mpsc` channel is invalid. Deferred pages are bounded separately by
    /// [`DEFAULT_MAX_BACKLOG_BYTES`].
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] if the spool file (or its parent
    /// directory) cannot be created.
    pub async fn new(spool_path: PathBuf, buffer_size: usize) -> Result<Self, BoundedSinkError> {
        Self::with_backlog_bytes(spool_path, buffer_size, DEFAULT_MAX_BACKLOG_BYTES).await
    }

    /// Create a sink with an explicit byte ceiling for the deferred backlog.
    ///
    /// The seam that makes the bound testable and tunable: a spool too slow to
    /// drain holds at most `max_backlog_bytes` of page bodies beyond the
    /// channel, whatever the producer rate. A ceiling of 0 means a full channel
    /// refuses pages outright.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] if the spool file (or its parent
    /// directory) cannot be created.
    pub async fn with_backlog_bytes(
        spool_path: PathBuf,
        buffer_size: usize,
        max_backlog_bytes: usize,
    ) -> Result<Self, BoundedSinkError> {
        if let Some(parent) = spool_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::File::create(&spool_path).await?;
        let buffer = buffer_size.max(1);
        let (tx, rx) = mpsc::channel(buffer);
        let backlog = Arc::new(Backlog::new(max_backlog_bytes));
        let cancel = CancellationToken::new();
        let writer = tokio::spawn(
            spool_writer(file, rx, Arc::clone(&backlog), cancel.clone()).in_current_span(),
        );

        info!(
            spool = %spool_path.display(),
            buffer_size = buffer,
            max_backlog_bytes,
            "bounded content sink ready"
        );

        Ok(Self {
            tx: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            backlog,
            cancel,
            spool_path,
            captured: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            buffer,
        })
    }

    /// Path of the JSONL spool holding the captured pages.
    #[must_use]
    pub fn spool_path(&self) -> &Path {
        &self.spool_path
    }

    /// Pages accepted for spooling, whether they took the channel fast path or
    /// the backlog.
    ///
    /// Excludes pages refused at the backlog ceiling (see
    /// [`dropped`](Self::dropped)), so on a successful flush
    /// `captured() == written + dropped()`.
    #[must_use]
    pub fn captured(&self) -> usize {
        self.captured.load(Ordering::Relaxed)
    }

    /// Pages refused because the backlog byte ceiling was already reached.
    ///
    /// Non-zero means the spool could not keep up with the crawl *and* the
    /// sink's memory ceiling was hit: those page bodies are not in the spool and
    /// the run exported fewer pages than were fetched. A tripped backlog latches
    /// and reports exactly one `warn!`, so this counter is how a caller learns
    /// the loss was bounded and how large it was.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Request that the background writer stop and flush what it holds.
    ///
    /// Unparks a writer idle between pages; it cannot interrupt a write that is
    /// already blocked on the filesystem. [`finish`](Self::finish) always drains
    /// to the end of the spool, so this is for shutdown, not for completing a
    /// run.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether [`cancel`](Self::cancel) has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
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
        // Nothing else holds a clone: `capture` clones per call and releases it
        // before returning, so this single drop really does close the channel.
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
                let dropped = self.dropped.load(Ordering::Relaxed);
                info!(
                    pages = written,
                    dropped_pages = dropped,
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
    /// Record `html`, spooling it to disk without blocking or spawning.
    ///
    /// Three outcomes, in order of preference: the page goes straight into the
    /// channel; the channel is full, so it goes into the byte-bounded
    /// `backlog`; or the backlog ceiling is reached too, so the page is
    /// refused, counted, and reported once. No outcome spawns a task, so the
    /// producer's in-flight work is constant no matter how far the writer falls
    /// behind.
    ///
    /// The `#[instrument]` span declares the per-capture identity at creation
    /// time — FileTraceLayer snapshots fields in `on_new_span` (#501), so the
    /// outcome is reported as its own event rather than as a later field write.
    /// It carries no URL: `capture` runs once per page inside a `crawl_page`
    /// span that already owns the URL, and page URLs are sensitive data that
    /// must not be duplicated into every log line.
    #[instrument(
        name = "crawler.capture",
        skip_all,
        level = "debug",
        fields(sink_buffer = self.buffer, html_bytes = html.len())
    )]
    fn capture(&self, url: &str, html: &str) {
        let page = CapturedPage {
            url: url.to_string(),
            html: html.to_string(),
        };

        let Some(sender) = self.current_sender() else {
            warn!(url, "capture after sink close — page discarded");
            return;
        };

        match sender.try_send(page) {
            Ok(()) => {
                self.captured.fetch_add(1, Ordering::Relaxed);
            },
            Err(mpsc::error::TrySendError::Full(page)) => self.defer(page),
            // LCOV_EXCL_START defensive: closed-channel — capture after finish() is a lifecycle bug
            Err(mpsc::error::TrySendError::Closed(page)) => {
                warn!(url = %page.url, "capture channel closed — page discarded");
            },
            // LCOV_EXCL_STOP
        }
    }
}

impl BoundedFileSink {
    /// The live sender, or `None` once [`finish`](Self::finish) has closed the
    /// sink.
    ///
    /// The clone is deliberately short-lived: it exists for this one `try_send`
    /// and is dropped at the end of `capture`, so `finish` closing the only
    /// sender really does close the channel.
    fn current_sender(&self) -> Option<mpsc::Sender<CapturedPage>> {
        let guard = self
            .tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clone()
    }

    /// Route a page the full channel could not take into the byte-bounded
    /// backlog — or refuse and count it when even that is full.
    fn defer(&self, page: CapturedPage) {
        debug!(url = %page.url, "capture buffer full — deferring to the bounded backlog");
        match self.backlog.admit(page) {
            Admit::Queued => {
                self.captured.fetch_add(1, Ordering::Relaxed);
            },
            Admit::Saturated { first } => self.record_refused(first),
        }
    }

    /// Count a page refused at the backlog ceiling, reporting the trip once.
    fn record_refused(&self, first: bool) {
        // LCOV_EXCL_START defensive: saturated — requires a spool that cannot
        // drain; the ceiling itself is covered in the unit tests below
        self.dropped.fetch_add(1, Ordering::Relaxed);
        if first {
            let (held_pages, held_bytes) = self.backlog.occupancy();
            warn!(
                held_pages,
                held_bytes,
                max_bytes = self.backlog.max_bytes(),
                dropped_pages = self.dropped.load(Ordering::Relaxed),
                "capture backlog byte cap exceeded — further page bodies are not spooled"
            );
        }
        // LCOV_EXCL_STOP
    }
}

/// Result of offering a page to the `backlog`.
enum Admit {
    /// Accepted; the writer has been woken.
    Queued,
    /// The byte ceiling is reached. `first` is true only on the transition, so
    /// the sink can report the trip exactly once instead of once per page.
    Saturated { first: bool },
}

/// Pages a full channel defers to, drained by the writer task.
///
/// This is the second bound of the pair that replaces #1616's unbounded
/// producer: the channel bounds pages waiting to be spooled, and `max_bytes`
/// bounds the bodies held behind a full channel. A `Mutex` is the right tool
/// because every critical section is a synchronous check-and-push with no
/// `.await` inside — the writer takes the pages out and releases the guard
/// before writing.
#[derive(Debug)]
struct Backlog {
    inner: Mutex<BacklogState>,
    /// Wakes the writer after an admit. `Notify` stores a permit, so a push
    /// that races the writer's drain cannot be lost.
    notify: Notify,
    max_bytes: usize,
}

/// Deferred pages plus their byte accounting, guarded by one lock so the
/// admit-check and the push are atomic with respect to other workers.
#[derive(Debug, Default)]
struct BacklogState {
    pages: VecDeque<CapturedPage>,
    /// Sum of `url.len() + html.len()` over held pages.
    bytes: usize,
    /// Sticky: the byte ceiling has tripped, so later admits are refused.
    saturated: bool,
}

impl Backlog {
    fn new(max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(BacklogState::default()),
            notify: Notify::new(),
            max_bytes,
        }
    }

    /// Byte ceiling for held pages.
    fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Offer a page, or refuse it if the ceiling is already reached.
    fn admit(&self, page: CapturedPage) -> Admit {
        let incoming = page.url.len().saturating_add(page.html.len());
        // A poisoned mutex (a worker panicked mid-admit) degrades to the
        // recovered backlog rather than panicking, as the in-memory sink does.
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.bytes.saturating_add(incoming) > self.max_bytes {
            let first = !state.saturated;
            state.saturated = true;
            return Admit::Saturated { first };
        }
        state.bytes = state.bytes.saturating_add(incoming);
        state.pages.push_back(page);
        drop(state);
        // Signal AFTER the push is visible under the lock, so a writer that
        // wakes cannot miss the page; and a writer that is already parked gets
        // the stored permit.
        self.notify.notify_one();
        Admit::Queued
    }

    /// Take every deferred page, leaving the backlog empty.
    fn drain(&self) -> Vec<CapturedPage> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.bytes = 0;
        std::mem::take(&mut state.pages).into()
    }

    /// Whether no page is waiting in the backlog.
    fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pages
            .is_empty()
    }

    /// `(held_pages, held_bytes)` currently in the backlog.
    fn occupancy(&self) -> (usize, usize) {
        let state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.pages.len(), state.bytes)
    }

    /// Wake the writer without changing the backlog.
    async fn notified(&self) {
        self.notify.notified().await;
    }
}

/// Background writer: drains both hand-off halves and appends one JSON object
/// per line.
///
/// The loop is drain-then-wait: it empties the channel and the backlog without
/// blocking, then parks on whichever half gets a page. That ordering is what
/// makes shutdown correct — a page can never sit in the backlog while the writer
/// has already concluded the stream is over, because the exit condition is
/// re-checked after every drain. Every await is cancel-safe (a `mpsc::recv` and a
/// `Notify` future are both safe to drop, and neither holds a lock), which is
/// what #1616 CC-D3 was missing.
async fn spool_writer(
    file: tokio::fs::File,
    mut rx: mpsc::Receiver<CapturedPage>,
    backlog: Arc<Backlog>,
    cancel: CancellationToken,
) -> Result<usize, BoundedSinkError> {
    let mut writer = BufWriter::new(file);
    let mut written = 0usize;

    loop {
        while let Ok(page) = rx.try_recv() {
            write_page(&mut writer, &page, &mut written).await?;
        }
        for page in backlog.drain() {
            write_page(&mut writer, &page, &mut written).await?;
        }

        // Only a closed channel can end the stream, and only once it is closed,
        // EMPTY, and the backlog it fed is empty. `is_closed()` alone is not
        // enough: closing makes the channel refuse new sends, it does not
        // discard what is already buffered. A page that arrives between the
        // drain above and this check is still queued, and breaking here would
        // drop it with the receiver — a silent loss of exactly the kind #631
        // exists to prevent.
        //
        // Ordering is what makes this race-free rather than merely hopeful:
        // once `is_closed()` is true every sender is gone, so no further `send`
        // or `admit` can run and both emptiness checks are stable. Checking
        // `is_closed()` first is therefore sufficient to freeze the channel
        // before reading the two lengths.
        if rx.is_closed() && rx.is_empty() && backlog.is_empty() {
            break;
        }

        tokio::select! {
            biased;
            next = rx.recv() => {
                if let Some(page) = next {
                    write_page(&mut writer, &page, &mut written).await?;
                }
                // `None`: every sender is gone. Loop to drain the backlog and
                // re-check the exit condition rather than breaking here.
            },
            _ = backlog.notified() => {}
            _ = cancel.cancelled() => {
                let pending = backlog.occupancy().0;
                warn!(written, pending_pages = pending, "capture spool write cancelled — flushing what was persisted");
                break;
            },
        }
    }

    writer.flush().await?;
    writer.into_inner().sync_all().await?;
    Ok(written)
}

/// Append one page as a single JSONL line and count it.
async fn write_page(
    writer: &mut BufWriter<tokio::fs::File>,
    page: &CapturedPage,
    written: &mut usize,
) -> Result<(), BoundedSinkError> {
    let line = serde_json::to_string(page)?;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    *written += 1;
    Ok(())
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
    use std::time::Duration;

    async fn sink_in(dir: &tempfile::TempDir, buffer: usize) -> BoundedFileSink {
        BoundedFileSink::new(dir.path().join("spool.jsonl"), buffer)
            .await
            .expect("sink must be created")
    }

    async fn sink_with_backlog(
        dir: &tempfile::TempDir,
        buffer: usize,
        max_backlog_bytes: usize,
    ) -> BoundedFileSink {
        BoundedFileSink::with_backlog_bytes(
            dir.path().join("spool.jsonl"),
            buffer,
            max_backlog_bytes,
        )
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
        // Buffer of 1 routes nearly every capture through the backlog.
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

        // `finish` waits for the writer to drain BOTH the channel and the
        // backlog, so there is nothing left to wait on here — the deferred pages
        // are already accounted for and nothing is racing.
        assert_eq!(sink.finish().await.expect("flush"), 64);
        assert_eq!(sink.dropped(), 0, "the default backlog absorbs the burst");
        assert_eq!(drain(&sink).await.len(), 64);
    }

    // ===== #1616 P0.2: the producer must not scale with the writer's lag =====

    /// The P0.2 invariant, proved without touching a clock.
    ///
    /// A `current_thread` runtime cannot run the writer until this task yields,
    /// so the capture loop below is a **deterministic stalled writer**: every
    /// page meets a full channel. What the old code did here was spawn one task
    /// per capture, so retained memory grew with the producer count. What this
    /// code does is refuse pages once the backlog ceiling is reached, so the
    /// retained set is *constant*.
    ///
    /// The proof is the difference between the two runs: capturing 10x more
    /// pages must leave exactly as many pages retained. Under the old behaviour
    /// the second run retained 10x the first.
    #[tokio::test]
    async fn a_stalled_writer_holds_a_constant_number_of_pages_however_many_arrive() {
        async fn retained_after(dir: &tempfile::TempDir, pages: usize) -> (usize, usize) {
            // Channel of 1, backlog ceiling of 128 bytes: both are tiny, and the
            // backlog is far smaller than `pages` in both runs, so the ceiling is
            // what decides how much is retained.
            let sink = sink_with_backlog(dir, 1, 128).await;
            let before = sink.backlog.occupancy().0;
            for i in 0..pages {
                sink.capture(&format!("https://example.com/{i}"), "0123456789");
            }
            let (held, _bytes) = sink.backlog.occupancy();
            let captured = sink.captured();
            let dropped = sink.dropped();
            assert!(dropped > 0, "the ceiling must bite to prove a bound exists");
            // Every capture is accounted for exactly once: spooled or refused.
            assert_eq!(captured + dropped, pages, "no silent loss or duplication");
            (held + 1, before) // +1: the page the channel accepted
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let (retained_small, _) = retained_after(&dir, 200).await;
        let (retained_large, _) = retained_after(&dir, 2_000).await;

        assert_eq!(
            retained_small, retained_large,
            "pages retained under a stalled writer must not grow with the producer count"
        );
    }

    /// A full channel with room in the backlog keeps every page — the #631
    /// no-drop guarantee still holds, it just lives in the sink now.
    #[tokio::test]
    async fn a_full_channel_defers_to_the_backlog_without_dropping() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Generous backlog, so only the channel is ever full.
        let sink = sink_with_backlog(&dir, 1, DEFAULT_MAX_BACKLOG_BYTES).await;

        for i in 0..64 {
            sink.capture(&format!("https://example.com/{i}"), "<p>body</p>");
        }

        assert_eq!(sink.dropped(), 0, "a roomy backlog must not refuse pages");
        assert_eq!(sink.finish().await.expect("flush"), 64);
        assert_eq!(drain(&sink).await.len(), 64);
    }

    /// Once the ceiling is reached the sink stays refused — the bound is sticky,
    /// so a wedged spool cannot be walked back into unbounded growth by a later
    /// capture.
    #[tokio::test]
    async fn a_tripped_backlog_stays_tripped() {
        let backlog = Backlog::new(4);
        let page = |u: &str| CapturedPage {
            url: u.to_string(),
            html: "0123456789".to_string(),
        };

        assert!(matches!(
            backlog.admit(page("a")),
            Admit::Saturated { first: true }
        ));
        // A later, smaller page is still refused: the latch does not reopen.
        assert!(matches!(
            backlog.admit(page("b")),
            Admit::Saturated { first: false }
        ));
    }

    /// The backlog admits exactly up to its byte ceiling and never past it.
    #[test]
    fn the_backlog_admits_up_to_its_byte_ceiling() {
        let backlog = Backlog::new(6);
        let page = |u: &str| CapturedPage {
            url: u.to_string(),
            html: "ab".to_string(),
        };

        // Each page costs 3 bytes (1 url + 2 html); the third would exceed 6.
        assert!(matches!(backlog.admit(page("a")), Admit::Queued));
        assert!(matches!(backlog.admit(page("b")), Admit::Queued));
        assert!(matches!(backlog.admit(page("c")), Admit::Saturated { .. }));
        assert_eq!(backlog.occupancy(), (2, 6));
        assert!(!backlog.is_empty());

        let drained = backlog.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(
            backlog.occupancy(),
            (0, 0),
            "draining resets the accounting"
        );
        assert!(backlog.is_empty());
    }

    // ===== #1616 CC-D3: the writer must not park forever =====

    /// A cancelled writer stops, and `finish` still joins it.
    ///
    /// The timeout here is a hang tripwire, not a synchronisation device: the
    /// test's correctness comes from the cancellation token (injected, not timed)
    /// and from `finish`'s own join, so a regression that parks the writer fails
    /// deterministically instead of flaking.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_unparks_an_idle_writer_and_finish_still_drains() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = sink_in(&dir, 2).await;

        sink.capture("https://example.com/a", "<p>a</p>");

        // Let the writer drain the page, then idle with the channel still open:
        // that is the state a real run sits in between pages, and the state a
        // dead writer used to strand producers in.
        let drained = tokio::time::timeout(Duration::from_secs(10), sink.finish())
            .await
            .expect("drain completes")
            .expect("flush succeeds");
        assert_eq!(drained, 1);

        assert!(!sink.is_cancelled());
        sink.cancel();
        assert!(sink.is_cancelled());

        // A second `finish` is a lifecycle error, not a hang: proving the join
        // is consumed is enough to show the writer task was reaped.
        let err = tokio::time::timeout(Duration::from_secs(10), sink.finish())
            .await
            .expect("a cancelled sink must not park finish")
            .expect_err("second flush must fail");
        assert!(matches!(err, BoundedSinkError::AlreadyFinished));
    }

    /// Cancelling mid-run flushes what was already persisted instead of hanging.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancelled_writer_flushes_what_it_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Single-threaded: the writer cannot run until this task yields, so the
        // cancel is requested while every page is still queued.
        let sink = sink_in(&dir, 4).await;
        sink.capture("https://example.com/a", "<p>a</p>");
        sink.cancel();

        let written = tokio::time::timeout(Duration::from_secs(10), sink.finish())
            .await
            .expect("a cancelled writer must not park finish")
            .expect("flush succeeds");
        assert!(written <= 1, "only the queued page could have been written");
        // Whatever it wrote is still a valid spool, not a torn one.
        for page in drain(&sink).await {
            assert!(page.url.starts_with("https://example.com/"));
        }
    }

    /// Regression test for a real page-loss defect this change introduced and
    /// then fixed: the writer's exit condition tested `rx.is_closed()` but not
    /// `rx.is_empty()`, so a page buffered *between* the writer's drain and its
    /// close check was dropped along with the receiver. `finish()` then
    /// under-reported and the page was simply gone — the exact silent loss
    /// #631 exists to prevent, reintroduced through the fix for it.
    ///
    /// The interleaving is a genuine race and is NOT forced here, so this is a
    /// repeated probe rather than a deterministic proof: the writer is polled on
    /// a separate worker while the producer sends and the sink closes, and each
    /// round asserts the full accounting identity. A single-threaded test cannot
    /// reproduce it at all, because the writer never runs between the captures
    /// and the close. The bound is a claim about coverage, not a guarantee, and
    /// the invariant it checks is exact whenever it does fire.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn no_accepted_page_is_lost_when_the_sink_closes_mid_flight() {
        const ROUNDS: usize = 200;
        const PAGES: usize = 8;

        for round in 0..ROUNDS {
            let dir = tempfile::tempdir().expect("tempdir");
            // Buffer of 1 keeps the channel saturated, so most pages take the
            // backlog path and the writer's drain/check loop runs repeatedly.
            let sink = sink_in(&dir, 1).await;

            for i in 0..PAGES {
                sink.capture(&format!("https://example.com/{round}/{i}"), "<p>body</p>");
            }
            let captured = sink.captured();
            let written = sink.finish().await.expect("flush succeeds");

            assert_eq!(sink.dropped(), 0, "the default backlog absorbs the burst");
            assert_eq!(
                written, captured,
                "round {round}: every accepted page must reach the spool"
            );
            assert_eq!(drain(&sink).await.len(), captured, "round {round}");
        }
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

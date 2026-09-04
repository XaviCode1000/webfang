//! Append-only storage for crawl results
//!
//! Implements [`CrawlResultRepository`] using a binary append-only log file
//! with a [`DashMap`] in-memory index (URL → byte offset). A single background
//! writer runs on the blocking pool and receives writes via
//! [`mpsc::channel`] — no locks on the hot path, and no synchronous disk I/O
//! on the Tokio executor (#1121).
//!
//! ## Storage Format
//!
//! ```text
//! [4 bytes: u32 LE payload_length][N bytes: JSON ScrapedContent][1 byte: \n]
//! ```
//!
//! - `\n` terminator enables corruption detection and manual inspection
//! - Size prefix enables O(1) random access via index offset
//! - Sequential append → HDD-friendly sequential write (~120MB/s)

use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::domain::repositories::CrawlResultRepository;
use crate::domain::{CrawlError, ScrapedContent};

enum WriteCommand {
    Append {
        url: String,
        payload: Vec<u8>,
    },
    /// Graceful stop: drain everything already queued, then exit (#1121).
    Shutdown,
}

/// Append-only storage for crawl results
///
/// Writes are sent to a background writer task via an mpsc channel.
/// Reads use the in-memory DashMap index for O(1) lookups.
pub struct CrawlResultRepositoryImpl {
    tx: mpsc::Sender<WriteCommand>,
    index: Arc<DashMap<String, u64>>,
    log_path: PathBuf,
    /// Set to true if the background writer encounters an I/O error.
    /// Subsequent save() calls will fail explicitly instead of silently
    /// accepting writes that will never be persisted.
    write_error: Arc<AtomicBool>,
    /// Handle of the blocking-pool writer thread (#1121). Taken by
    /// `shutdown()` and joined there; a writer panic is surfaced as a
    /// `JoinError` instead of being silently detached.
    writer_handle: Mutex<Option<JoinHandle<()>>>,
}

impl CrawlResultRepositoryImpl {
    /// Create a new append-only repository.
    ///
    /// Spawns a background writer task and, if the log file exists, rebuilds
    /// the index by scanning existing records.
    ///
    /// # Arguments
    ///
    /// * `log_path` - Path to the append-only log file
    /// * `buffer_capacity` - Capacity of the mpsc channel (backpressure limit)
    pub fn new(log_path: PathBuf, buffer_capacity: usize) -> Result<Self, CrawlError> {
        let (tx, rx) = mpsc::channel(buffer_capacity);
        let index = Arc::new(DashMap::new());
        let write_error = Arc::new(AtomicBool::new(false));

        // Recovery: scan existing log if present
        if log_path.exists() {
            Self::recover_index(&log_path, &index)?;
        }

        // Spawn the background writer on the BLOCKING pool (#1121): its
        // open/write/flush syscalls must never occupy a Tokio worker, and
        // blocking-pool tasks are awaited at runtime shutdown, so buffered
        // records survive process exit. The handle is kept for `shutdown()`.
        //
        // No span is attached: the repository is process-lifetime
        // infrastructure constructed before any run-root exists, so writer
        // diagnostics must not pretend to belong to a startup span. The
        // correlated durability summary is the `shutdown()` join.
        let writer = BackgroundWriter::new(
            log_path.clone(),
            rx,
            Arc::clone(&index),
            Arc::clone(&write_error),
        );
        let writer_handle = tokio::task::spawn_blocking(move || writer.run());

        Ok(Self {
            tx,
            index,
            log_path,
            write_error,
            writer_handle: Mutex::new(Some(writer_handle)),
        })
    }

    /// Rebuild the DashMap index by scanning the log file sequentially.
    fn recover_index(path: &PathBuf, index: &DashMap<String, u64>) -> Result<(), CrawlError> {
        use std::io::Read;

        let file = std::fs::File::open(path)
            .map_err(|e| CrawlError::Storage(format!("no se pudo abrir log: {e}")))?;
        let mut reader = std::io::BufReader::new(file);
        let mut offset: u64 = 0;

        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    return Err(CrawlError::Storage(format!(
                        "lectura corrupta en offset {offset}: {e}"
                    )))
                },
            }

            let len = u32::from_le_bytes(len_buf) as usize;

            // If remaining file is too short, discard incomplete trailing record
            let mut payload = vec![0u8; len];
            if reader.read_exact(&mut payload).is_err() {
                // Partial trailing record — crash-safe: skip silently
                break;
            }

            // Skip newline
            let mut newline = [0u8; 1];
            let _ = reader.read_exact(&mut newline);

            // Extract URL from JSON to populate index
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&payload) {
                if let Some(url) = json.get("url").and_then(|u| u.as_str()) {
                    index.insert(url.to_string(), offset);
                }
            }

            offset += 4 + len as u64 + 1;
        }

        Ok(())
    }
}

impl CrawlResultRepository for CrawlResultRepositoryImpl {
    fn save(&self, content: &ScrapedContent) -> Result<(), CrawlError> {
        // Guard: if the background writer is dead, fail explicitly
        if self.write_error.load(Ordering::Relaxed) {
            return Err(CrawlError::Storage(
                "writer caído, datos no persistidos".to_string(),
            ));
        }

        let payload = serde_json::to_vec(content)
            .map_err(|e| CrawlError::Storage(format!("serialización fallida: {e}")))?;

        let url = content.url.as_str().to_string();
        self.tx
            .try_send(WriteCommand::Append { url, payload })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => {
                    CrawlError::Storage("canal lleno, backpressure".to_string())
                },
                mpsc::error::TrySendError::Closed(_) => {
                    // Writer task dropped the receiver — mark as dead
                    self.write_error.store(true, Ordering::Relaxed);
                    CrawlError::Storage("writer caído, canal cerrado".to_string())
                },
            })?;

        Ok(())
    }

    fn find_by_url(&self, url: &str) -> Result<Option<ScrapedContent>, CrawlError> {
        use std::io::{Read, Seek, SeekFrom};

        let offset = match self.index.get(url) {
            Some(entry) => *entry,
            None => return Ok(None),
        };

        let mut file = std::fs::File::open(&self.log_path)
            .map_err(|e| CrawlError::Storage(format!("no se pudo abrir log: {e}")))?;

        file.seek(SeekFrom::Start(offset))
            .map_err(|e| CrawlError::Storage(format!("seek fallido: {e}")))?;

        let mut len_buf = [0u8; 4];
        file.read_exact(&mut len_buf)
            .map_err(|e| CrawlError::Storage(format!("lectura de longitud fallida: {e}")))?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut payload = vec![0u8; len];
        file.read_exact(&mut payload)
            .map_err(|e| CrawlError::Storage(format!("lectura de payload fallida: {e}")))?;

        let result: ScrapedContent = serde_json::from_slice(&payload)
            .map_err(|e| CrawlError::Storage(format!("deserialización fallida: {e}")))?;

        Ok(Some(result))
    }

    fn get_all_urls(&self) -> Result<Vec<String>, CrawlError> {
        Ok(self.index.iter().map(|entry| entry.key().clone()).collect())
    }

    /// Load all persisted content with a single sequential log scan.
    ///
    /// Overrides the default N+1 (`get_all_urls` → `find_by_url`) loop with
    /// one forward pass over the append-only log, reusing the same record
    /// framing as `Self::recover_index`: `[4-byte LE length][JSON payload]
    /// [\n]`. A partial trailing record (torn write) stops the scan cleanly
    /// instead of erroring, preserving crash-safety.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlError::Storage`] if the log cannot be opened, a record
    /// length/payload cannot be read (mid-record corruption), or a payload
    /// fails to deserialize.
    fn load_all(&self) -> Result<Vec<ScrapedContent>, CrawlError> {
        use std::io::Read;

        // A fresh repository has no log file yet (the background writer
        // creates it asynchronously). No file means nothing persisted —
        // return empty, matching the default trait method's behavior for an
        // empty index.
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let file = std::fs::File::open(&self.log_path)
            .map_err(|e| CrawlError::Storage(format!("no se pudo abrir log: {e}")))?;
        let mut reader = std::io::BufReader::new(file);
        let mut results = Vec::with_capacity(self.index.len());
        let mut offset: u64 = 0;

        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    return Err(CrawlError::Storage(format!(
                        "lectura corrupta en offset {offset}: {e}"
                    )))
                },
            }

            let len = u32::from_le_bytes(len_buf) as usize;

            // A trailing record shorter than its declared length is a torn
            // write — stop cleanly (crash-safe) rather than erroring.
            let mut payload = vec![0u8; len];
            if reader.read_exact(&mut payload).is_err() {
                break;
            }

            // Skip the newline terminator.
            let mut newline = [0u8; 1];
            let _ = reader.read_exact(&mut newline);

            let content: ScrapedContent = serde_json::from_slice(&payload)
                .map_err(|e| CrawlError::Storage(format!("deserialización fallida: {e}")))?;
            results.push(content);

            offset += 4 + len as u64 + 1;
        }

        Ok(results)
    }

    /// Drain and join the background writer (#1121).
    ///
    /// Sends a `Shutdown` command through the same bounded channel — `send`
    /// (not `try_send`) waits for capacity, so the request lands after every
    /// buffered append — then awaits the blocking-pool writer so every write
    /// acknowledged by `save` is confirmed flushed before the caller
    /// proceeds. A writer panic or I/O failure is reported, never swallowed.
    /// Idempotent: a second call finds no handle and returns `Ok`.
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = Result<(), CrawlError>> + Send + '_>> {
        Box::pin(async move {
            // If the writer is already gone the request fails; the join
            // below still reports its terminal status.
            let _send_result = self.tx.send(WriteCommand::Shutdown).await;
            // The std guard is dropped before the await — never held across
            // a suspension point.
            let handle = self
                .writer_handle
                .lock()
                .ok()
                .and_then(|mut pending| pending.take());
            let Some(handle) = handle else {
                return Ok(());
            };
            match handle.await {
                Ok(()) => {
                    // The writer is gone: any later save would land in a dead
                    // queue, so fail explicitly from here on (#1121).
                    let io_failed = self.write_error.swap(true, Ordering::Relaxed);
                    if io_failed {
                        Err(CrawlError::Storage(
                            "el writer reportó errores de I/O durante el cierre".to_string(),
                        ))
                    } else {
                        tracing::info!("crawl-result writer drained and joined");
                        Ok(())
                    }
                },
                Err(e) => {
                    self.write_error.store(true, Ordering::Relaxed);
                    tracing::error!(error = %e, "crawl-result writer task failed during shutdown");
                    Err(CrawlError::Storage(format!(
                        "writer caído durante shutdown: {e}"
                    )))
                },
            }
        })
    }
}

/// Background writer task that processes write commands sequentially.
struct BackgroundWriter {
    rx: mpsc::Receiver<WriteCommand>,
    index: Arc<DashMap<String, u64>>,
    log_path: PathBuf,
    write_error: Arc<AtomicBool>,
}

impl BackgroundWriter {
    fn new(
        log_path: PathBuf,
        rx: mpsc::Receiver<WriteCommand>,
        index: Arc<DashMap<String, u64>>,
        write_error: Arc<AtomicBool>,
    ) -> Self {
        Self {
            rx,
            index,
            log_path,
            write_error,
        }
    }

    /// Blocking-pool writer loop (#1121).
    ///
    /// Runs on a dedicated blocking thread: `blocking_recv` parks THIS OS
    /// thread (never a Tokio worker) while the channel is empty, and every
    /// filesystem syscall below stays off the executor. The loop exits when
    /// the channel closes — all senders dropped (process exit) or after a
    /// `Shutdown` command has drained the queue — so buffered records are
    /// always written before the writer leaves.
    fn run(mut self) {
        // The log file is opened lazily on the first actual write (issue #606):
        // when nothing is ever persisted we must not litter the CWD with an
        // empty `output/` directory and a 0-byte `crawl_results.bin`.
        let mut file: Option<std::fs::File> = None;
        // Byte offset of the next append, tracked locally (#1121): replaces
        // the per-record `metadata().unwrap_or(0)` lie with one honest stat
        // at open time plus arithmetic per frame.
        let mut offset: u64 = 0;

        while let Some(cmd) = self.rx.blocking_recv() {
            match cmd {
                WriteCommand::Append { url, payload } => {
                    if self
                        .append_cmd(&mut file, &mut offset, url, &payload)
                        .is_err()
                    {
                        return;
                    }
                },
                WriteCommand::Shutdown => {
                    // Drain commands that raced in after the shutdown request,
                    // then exit. `try_recv` never blocks, so the drain is
                    // bounded by what senders have already committed.
                    while let Ok(WriteCommand::Append { url, payload }) = self.rx.try_recv() {
                        if self
                            .append_cmd(&mut file, &mut offset, url, &payload)
                            .is_err()
                        {
                            return;
                        }
                    }
                    break;
                },
            }
        }
        // No exit log: the repository is constructed once per process, before
        // any run-root span exists, so a terminal event here would mint an
        // orphan `trace_id` and break the single-trace invariant asserted by
        // `trace_orphan_spawn_test`. The correlated summary lives in
        // `shutdown()` (caller's span); I/O failures still log from inside
        // the loop.
    }

    /// Ensure the log is open, then append one framed record.
    ///
    /// `Err(())` signals a fatal open/stat failure that was already reported
    /// through `write_error`; the caller must bail out of the loop.
    fn append_cmd(
        &self,
        file: &mut Option<std::fs::File>,
        offset: &mut u64,
        url: String,
        payload: &[u8],
    ) -> Result<(), ()> {
        if file.is_none() {
            match self.open_log() {
                Ok(f) => match f.metadata() {
                    Ok(m) => {
                        *offset = m.len();
                        *file = Some(f);
                    },
                    Err(e) => {
                        tracing::error!("failed to stat log for initial offset: {e}");
                        self.write_error.store(true, Ordering::Relaxed);
                        return Err(());
                    },
                },
                Err(()) => return Err(()),
            }
        }
        if let Some(f) = file.as_mut() {
            self.append_record(f, offset, url, payload);
        }
        Ok(())
    }

    /// Create the parent directory (if any) and open the log file for appending.
    /// On any failure, marks the writer as errored and returns `Err(())` so the
    /// caller can bail out of the writer loop.
    fn open_log(&self) -> Result<std::fs::File, ()> {
        // H6 FIX: Create parent directory before opening log file
        if let Some(parent) = self.log_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::error!("failed to create log directory: {e}");
                self.write_error.store(true, Ordering::Relaxed);
                return Err(());
            }
        }

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
        {
            Ok(f) => Ok(f),
            Err(e) => {
                tracing::error!("failed to open log for writing: {e}");
                self.write_error.store(true, Ordering::Relaxed);
                Err(())
            },
        }
    }

    /// Append a single framed record (`[len][payload][\n]`) to the log and index
    /// the URL at its byte offset. Any write failure marks the writer errored.
    ///
    /// Takes `&mut dyn Write` so the framing is testable against failure
    /// stubs; the offset is tracked by the caller so no per-record `stat`
    /// syscall is needed.
    fn append_record(&self, file: &mut dyn Write, offset: &mut u64, url: String, payload: &[u8]) {
        let len = payload.len() as u32;

        if let Err(e) = Self::write_frame(file, len, payload) {
            tracing::error!("error writing record to log: {e}");
            self.write_error.store(true, Ordering::Relaxed);
            return;
        }

        self.index.insert(url, *offset);
        *offset += 4 + u64::from(len) + 1;
    }

    /// Write one framed record `[len][payload][\n]` and flush it.
    ///
    /// #1121: the flush result is propagated, never discarded — a failed
    /// flush means the record is not durable, so the caller must not index
    /// it as if it were.
    fn write_frame(file: &mut dyn Write, len: u32, payload: &[u8]) -> Result<(), std::io::Error> {
        file.write_all(&len.to_le_bytes())?;
        file.write_all(payload)?;
        file.write_all(b"\n")?;
        file.flush()
    }
}

#[cfg(all(test, not(miri)))] // wait_for_index uses tokio::time::sleep which hangs under Miri
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use url::Url;

    use crate::domain::value_objects::ValidUrl;

    /// Helper: URL with trailing slash removed for consistent assertions.
    /// Url::parse normalizes "https://a.com" to "https://a.com/"
    fn make_content(url_str: &str, title: &str) -> ScrapedContent {
        let url = Url::parse(url_str).unwrap();
        ScrapedContent {
            url: ValidUrl::new(url),
            title: title.to_string(),
            content: format!("Content for {title}"),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
            quality_hint: None,
        }
    }

    /// Poll until the background writer has processed a write for the given URL.
    async fn wait_for_index(repo: &CrawlResultRepositoryImpl, url: &str) {
        for _ in 0..40 {
            if repo.find_by_url(url).unwrap().is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn test_save_and_find_by_url() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

        let content = make_content("https://example.com", "Example");
        repo.save(&content).unwrap();

        // Poll until the background writer has updated the index
        wait_for_index(&repo, "https://example.com/").await;

        let found = repo.find_by_url("https://example.com/").unwrap();
        assert!(found.is_some(), "expected to find saved content");
        let found = found.unwrap();
        assert_eq!(found.title, "Example");
        assert_eq!(found.content, "Content for Example");
        // Normalized URL from url::Url includes trailing slash
        assert_eq!(found.url.as_str(), "https://example.com/");
    }

    #[tokio::test]
    async fn test_find_by_url_unknown_returns_none() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

        let found = repo.find_by_url("https://unknown.com").unwrap();
        assert!(found.is_none(), "expected None for unknown URL");
    }

    #[tokio::test]
    async fn test_get_all_urls_returns_all_saved() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

        repo.save(&make_content("https://a.com", "A")).unwrap();
        repo.save(&make_content("https://b.com", "B")).unwrap();
        repo.save(&make_content("https://c.com", "C")).unwrap();

        // Wait for all three writes
        wait_for_index(&repo, "https://a.com/").await;
        wait_for_index(&repo, "https://b.com/").await;
        wait_for_index(&repo, "https://c.com/").await;

        let mut urls = repo.get_all_urls().unwrap();
        urls.sort();
        // url::Url normalizes bare domains with trailing slash
        assert_eq!(
            urls,
            vec!["https://a.com/", "https://b.com/", "https://c.com/"]
        );
    }

    #[tokio::test]
    async fn test_load_all_returns_all_saved() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

        repo.save(&make_content("https://a.com", "A")).unwrap();
        repo.save(&make_content("https://b.com", "B")).unwrap();
        repo.save(&make_content("https://c.com", "C")).unwrap();

        // Wait for all three writes to be indexed
        wait_for_index(&repo, "https://a.com/").await;
        wait_for_index(&repo, "https://b.com/").await;
        wait_for_index(&repo, "https://c.com/").await;

        let all = repo.load_all().unwrap();
        assert_eq!(all.len(), 3, "load_all should return all 3 saved items");

        let mut titles: Vec<String> = all.iter().map(|c| c.title.clone()).collect();
        titles.sort();
        assert_eq!(titles, vec!["A", "B", "C"]);

        // Content integrity: each item carries its expected body
        for item in &all {
            assert_eq!(item.content, format!("Content for {}", item.title));
        }
    }

    #[tokio::test]
    async fn test_load_all_empty_repository_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

        let all = repo.load_all().unwrap();
        assert!(all.is_empty(), "load_all on fresh repo should be empty");
    }

    #[tokio::test]
    async fn test_recovery_rebuilds_index() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");

        // First session: save entries
        {
            let repo = CrawlResultRepositoryImpl::new(log_path.clone(), 64).unwrap();
            repo.save(&make_content("https://first.com", "First"))
                .unwrap();
            repo.save(&make_content("https://second.com", "Second"))
                .unwrap();
            wait_for_index(&repo, "https://first.com/").await;
            wait_for_index(&repo, "https://second.com/").await;
        } // repo drops — writer task stops

        // Second session: recover from existing log
        {
            let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();
            let mut urls = repo.get_all_urls().unwrap();
            urls.sort();
            assert_eq!(urls, vec!["https://first.com/", "https://second.com/"]);

            let found = repo.find_by_url("https://first.com/").unwrap();
            assert!(found.is_some());
            assert_eq!(found.unwrap().title, "First");
        }
    }

    #[tokio::test]
    async fn test_empty_repository_returns_empty_urls() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

        let urls = repo.get_all_urls().unwrap();
        assert!(urls.is_empty(), "expected empty URL list for fresh repo");
    }

    #[tokio::test]
    async fn test_save_multiple_and_read_each() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

        let contents = vec![
            make_content("https://alpha.com/page1", "Alpha One"),
            make_content("https://beta.com/page2", "Beta Two"),
            make_content("https://gamma.com/page3", "Gamma Three"),
        ];

        for c in &contents {
            repo.save(c).unwrap();
        }

        // Wait for all writes
        wait_for_index(&repo, "https://alpha.com/page1").await;
        wait_for_index(&repo, "https://beta.com/page2").await;
        wait_for_index(&repo, "https://gamma.com/page3").await;

        let found = repo
            .find_by_url("https://alpha.com/page1")
            .unwrap()
            .unwrap();
        assert_eq!(found.title, "Alpha One");
        assert_eq!(found.content, "Content for Alpha One");

        let found = repo
            .find_by_url("https://gamma.com/page3")
            .unwrap()
            .unwrap();
        assert_eq!(found.title, "Gamma Three");
        assert_eq!(found.content, "Content for Gamma Three");
    }

    #[tokio::test]
    async fn test_crash_safe_partial_record() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");

        // Write a valid record followed by a partial (truncated) record
        {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .unwrap();

            // Valid record
            let content = make_content("https://valid.com", "Valid");
            let payload = serde_json::to_vec(&content).unwrap();
            let len = (payload.len() as u32).to_le_bytes();
            file.write_all(&len).unwrap();
            file.write_all(&payload).unwrap();
            file.write_all(b"\n").unwrap();

            // Partial record: write only 2 bytes of a 4-byte length prefix
            file.write_all(&[0xFF, 0xFF]).unwrap();
            file.flush().unwrap();
        }

        // Recovery should succeed and only contain valid.com
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();
        let urls = repo.get_all_urls().unwrap();
        assert_eq!(urls, vec!["https://valid.com/"]);

        // And we can retrieve the valid record
        let found = repo.find_by_url("https://valid.com/").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, "Valid");
    }

    // ============================================================================
    // #1121 — background writer: drain-on-shutdown, joined handle, flush truth.
    // ============================================================================

    /// `shutdown()` must drain every buffered record and join the writer:
    /// 50 saves accepted into the bounded channel with NO polling in between
    /// must all be on disk when shutdown returns. The pre-fix shape dropped
    /// the `JoinHandle` (writer death invisible) and detached the task from
    /// runtime exit, so buffered records could be lost silently.
    #[tokio::test]
    async fn test_shutdown_drains_buffered_writes() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 1024).unwrap();

        for i in 0..50 {
            repo.save(&make_content(
                &format!("https://drain-{i}.example.com"),
                &format!("Drain {i}"),
            ))
            .expect("save accepted into bounded channel");
        }

        repo.shutdown()
            .await
            .expect("clean shutdown drains and joins the writer");

        // Idempotent: second shutdown finds no handle and succeeds.
        repo.shutdown().await.expect("shutdown is idempotent");

        let all = repo.load_all().expect("load_all after shutdown");
        assert_eq!(
            all.len(),
            50,
            "every save acknowledged before shutdown must be persisted"
        );
    }

    /// After `shutdown()` the send side is closed: a late `save` must fail
    /// explicitly instead of accepting data that will never be persisted.
    #[tokio::test]
    async fn test_save_after_shutdown_fails_explicitly() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("crawl_results.bin");
        let repo = CrawlResultRepositoryImpl::new(log_path, 8).unwrap();

        repo.shutdown().await.expect("clean shutdown");

        let err = repo
            .save(&make_content("https://late.example.com", "Late"))
            .expect_err("save after shutdown must fail");
        assert!(
            err.to_string().contains("writer caído"),
            "late save must name the dead writer, got: {err}"
        );
    }

    /// #1121 flush truth: a failing flush must mark the writer errored and
    /// must NOT index the record — the old `let _ = file.flush()` lied about
    /// durability by acknowledging a record the OS never accepted.
    #[test]
    fn append_record_flush_error_marks_write_error_and_skips_index() {
        struct FlushFails;
        impl Write for FlushFails {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("simulated disk full"))
            }
        }

        let dir = TempDir::new().unwrap();
        let (_tx, rx) = mpsc::channel(1);
        let index = Arc::new(DashMap::new());
        let write_error = Arc::new(AtomicBool::new(false));
        let writer = BackgroundWriter::new(
            dir.path().join("unused.bin"),
            rx,
            Arc::clone(&index),
            Arc::clone(&write_error),
        );

        let mut offset = 0u64;
        writer.append_record(
            &mut FlushFails,
            &mut offset,
            "https://x.example.com".into(),
            b"payload",
        );

        assert!(
            write_error.load(Ordering::Relaxed),
            "flush failure must mark the writer errored (never `let _ =`)"
        );
        assert!(
            index.is_empty(),
            "a record whose flush failed must not be indexed as durable"
        );
        assert_eq!(offset, 0, "offset must not advance on a failed frame");
    }

    /// Happy-path framing keeps the tracked offset exact across frames
    /// (replaces the per-record `metadata().unwrap_or(0)` stat).
    #[test]
    fn append_record_tracks_offset_across_frames() {
        struct Sink(Vec<u8>);
        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let (_tx, rx) = mpsc::channel(1);
        let index = Arc::new(DashMap::new());
        let write_error = Arc::new(AtomicBool::new(false));
        let writer = BackgroundWriter::new(
            dir.path().join("unused.bin"),
            rx,
            Arc::clone(&index),
            Arc::clone(&write_error),
        );

        let mut sink = Sink(Vec::new());
        let mut offset = 0u64;
        writer.append_record(
            &mut sink,
            &mut offset,
            "https://a.example.com".into(),
            b"aa",
        );
        writer.append_record(
            &mut sink,
            &mut offset,
            "https://b.example.com".into(),
            b"bbb",
        );

        assert_eq!(offset, (4 + 2 + 1) + (4 + 3 + 1), "two framed records");
        assert_eq!(index.get("https://a.example.com").map(|r| *r), Some(0u64));
        assert_eq!(index.get("https://b.example.com").map(|r| *r), Some(7u64));
        assert!(!write_error.load(Ordering::Relaxed));
    }

    // ============================================================================
    // Task 5.1 memory probe — repository index growth (BEFORE numbers).
    // Reuses this module's make_content/wait helpers; no byte assertions by
    // design (Q3 MEASURE FIRST).
    // ============================================================================
    #[tokio::test]
    async fn memory_probe_repository_index_growth_20k_results() {
        const N: usize = 20_000;
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = CrawlResultRepositoryImpl::new(dir.path().join("probe-log.jsonl"), 1_024)
            .expect("repository builds");
        let before = crate::infrastructure::observability::memory_probe::rss_bytes();

        for i in 0..N {
            let content = make_content(
                &format!("https://probe.example.com/doc-{i}"),
                &format!("Probe document {i}"),
            );
            repo.save(&content).expect("save accepted");
            // Yield periodically so the background writer drains the bounded
            // channel — exercising its real backpressure, not bypassing it.
            if i % 256 == 255 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }
        wait_for_index(&repo, &format!("https://probe.example.com/doc-{}", N - 1)).await;
        // Give the writer a final drain window for the tail of the channel.
        for _ in 0..40 {
            if repo
                .find_by_url(&format!("https://probe.example.com/doc-{}", N - 1))
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(repo.index.len(), N, "every save must land in the index");

        let after = crate::infrastructure::observability::memory_probe::rss_bytes();
        crate::infrastructure::observability::memory_probe::append_report(
            "BEFORE - crawl_result_repository index",
            &format!(
                "entries={} rss_before={} rss_after={} delta={}",
                repo.index.len(),
                crate::infrastructure::observability::memory_probe::fmt_rss(before),
                crate::infrastructure::observability::memory_probe::fmt_rss(after),
                crate::infrastructure::observability::memory_probe::fmt_rss(
                    after.and_then(|a| before.map(|b| a.saturating_sub(b)))
                ),
            ),
        );
    }
}

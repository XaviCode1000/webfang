//! Append-only storage for crawl results
//!
//! Implements [`CrawlResultRepository`] using a binary append-only log file
//! with a [`DashMap`] in-memory index (URL → byte offset). A single background
//! writer task receives writes via [`mpsc::channel`] — no locks on the hot path.
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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;

use crate::domain::repositories::CrawlResultRepository;
use crate::domain::{CrawlError, ScrapedContent};

enum WriteCommand {
    Append { url: String, payload: Vec<u8> },
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

        // Spawn background writer
        let writer = BackgroundWriter::new(
            log_path.clone(),
            rx,
            Arc::clone(&index),
            Arc::clone(&write_error),
        );
        tokio::spawn(writer.run());

        Ok(Self {
            tx,
            index,
            log_path,
            write_error,
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

    async fn run(mut self) {
        use std::io::Write;

        // H6 FIX: Create parent directory before opening log file
        if let Some(parent) = self.log_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::error!("no se pudo crear directorio para log: {e}");
                self.write_error.store(true, Ordering::Relaxed);
                return;
            }
        }

        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
        {
            Ok(f) => f,
            Err(e) => {
                tracing::error!("no se pudo abrir log para escritura: {e}");
                self.write_error.store(true, Ordering::Relaxed);
                return;
            },
        };

        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                WriteCommand::Append { url, payload } => {
                    let len = payload.len() as u32;
                    let len_bytes = len.to_le_bytes();

                    let offset = file.metadata().map(|m| m.len()).unwrap_or(0);

                    if file.write_all(&len_bytes).is_err() {
                        tracing::error!("error escribiendo longitud al log");
                        self.write_error.store(true, Ordering::Relaxed);
                        continue;
                    }
                    if file.write_all(&payload).is_err() {
                        tracing::error!("error escribiendo payload al log");
                        self.write_error.store(true, Ordering::Relaxed);
                        continue;
                    }
                    if file.write_all(b"\n").is_err() {
                        tracing::error!("error escribiendo newline al log");
                        self.write_error.store(true, Ordering::Relaxed);
                        continue;
                    }
                    let _ = file.flush();

                    self.index.insert(url, offset);
                },
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(miri, ignore)] // wait_for_index uses tokio::time::sleep which hangs under Miri
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
}

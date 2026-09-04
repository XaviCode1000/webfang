//! Headless JSONL vector sink — dependency-free RAG export (core-slimming, T3-C).
//!
//! `StreamRepository` implements [`VectorRepository`] by emitting one JSON line
//! per chunk that carries an embedding. It has **no** SQLite / `rusqlite` /
//! `sys-info` dependency, so it compiles into the lightweight `webfang_core`
//! binary and backs `--output-vectors <path|->` (spec R2 / S2.1).
//!
//! # Record shape
//!
//! Each emitted JSONL line is a [`VectorRecord`]:
//!
//! ```json
//! {"url":"...","sha256_hex":"<64 hex>","title":null,
//!  "chunk_text":"...","embedding":[0.01, … 384 floats …],
//!  "metadata":null,"timestamp":"2026-07-11T12:00:00Z"}
//! ```
//!
//! The embedding array is the **raw 384-dim** vector (no rounding, no base64) so
//! downstream RAG pipelines can ingest it directly.
//!
//! # Key decisions (T3-C design)
//!
//! - **Q3 (ai OFF):** when a chunk has `embedding = None`, the record is *omitted*
//!   (no line is written) — we never emit a null/zero vector.
//! - **D2 (broken pipe):** a write / flush `io::Error` (incl. `WriteZero` from a
//!   closed pipe) is returned as a fatal [`ScraperError::Io`], which propagates
//!   out of `ElasticIngestion::run` and aborts the crawl.
//! - **Concurrency (#1119):** the writer and title cache live behind `Arc<Mutex<…>>`
//!   and every lock acquisition + blocking `write_all`/`flush` runs inside
//!   `spawn_blocking`, so the JSONL stream stays line-oriented under concurrent
//!   [`crate::application::elastic_ingestion::ElasticIngestion`] processing
//!   without ever parking a Tokio worker on a std lock or a disk syscall.
//!   A poisoned lock maps to a typed `ScraperError::Io` — never a panic inside
//!   the async sink.

use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::clock::{SystemUtcClock, UtcClock};
use crate::domain::repository::VectorRepository;
use crate::error::ScraperError;

/// A single JSONL vector record emitted by [`StreamRepository`].
///
/// Mirrors the fields surfaced by the elastic ingestion pipeline: the source
/// URL, the content `sha256_hex` (content-hash dedup key), an optional title,
/// the cleaned `chunk_text`, the raw `embedding` vector, arbitrary `metadata`,
/// and an RFC3339 `timestamp`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorRecord {
    /// Source URL the chunk was extracted from.
    pub url: String,
    /// Content hash (SHA-256 hex) of the resource — the dedup key.
    pub sha256_hex: String,
    /// Best-effort title (first ≤200 chars of the first chunk line), or `null`.
    pub title: Option<String>,
    /// Cleaned chunk text.
    pub chunk_text: String,
    /// Raw embedding vector (e.g. 384 floats for all-MiniLM-L6-v2). Empty only
    /// when the record would have been omitted (Q3).
    pub embedding: Vec<f32>,
    /// Optional arbitrary metadata (reserved for future use).
    #[serde(default)]
    pub metadata: Value,
    /// RFC3339 timestamp of emission.
    pub timestamp: String,
}

/// Headless vector sink that writes [`VectorRecord`] lines to a JSONL stream.
///
/// Construct with [`StreamRepository::new`]; `"-"` selects stdout (buffered),
/// any other path selects a file.
///
/// # Executor hygiene (#1119)
///
/// Both locks are `std::sync::Mutex`es wrapped in `Arc` so the synchronous
/// critical sections (HashMap ops, `write_all`/`flush` syscalls) can be moved
/// into `spawn_blocking`. No lock is ever acquired on a Tokio worker thread,
/// and no `.await` happens while a guard is held. Poisoned locks surface as
/// `ScraperError::Io`, never as a panic inside the sink future.
pub struct StreamRepository {
    /// Serialized JSONL writer. `Box<dyn Write + Send>` so both `Stdout` and
    /// `File` fit behind one type; the `Arc<Mutex<…>>` keeps the stream
    /// line-oriented under concurrent ingestion and lets the write section
    /// run on the blocking pool (#1119).
    writer: Arc<Mutex<std::io::BufWriter<Box<dyn Write + Send>>>>,
    /// Title cache keyed by resource URL, populated by [`VectorRepository::save_resource`]
    /// and read by [`VectorRepository::save_chunk`] (the chunk call only receives
    /// `resource_url`, not the title).
    titles: Arc<Mutex<HashMap<String, String>>>,
    /// Injected clock for deterministic timestamps in tests.
    clock: Arc<dyn UtcClock>,
}

/// Map a poisoned lock to a typed I/O error instead of panicking inside the
/// async sink (#1119 — "0 `.expect` por poison en async").
fn lock_error(lock: &str) -> ScraperError {
    ScraperError::Io(std::io::Error::other(format!(
        "vector sink lock poisoned: {lock}"
    )))
}

impl StreamRepository {
    /// Open the JSONL sink with the system clock.
    ///
    /// * `path == "-"` → buffered stdout.
    /// * otherwise → a file created (truncated) at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Io`] if the file cannot be created.
    pub fn new(path: &str) -> Result<Self, ScraperError> {
        Self::with_clock(path, Arc::new(SystemUtcClock))
    }

    /// Open the JSONL sink with an injected clock.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Io`] if the file cannot be created.
    pub fn with_clock(path: &str, clock: Arc<dyn UtcClock>) -> Result<Self, ScraperError> {
        let boxed: Box<dyn Write + Send> = if path == "-" {
            Box::new(std::io::stdout())
        } else {
            if let Some(parent) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ScraperError::Io(std::io::Error::new(
                        e.kind(),
                        format!("no se pudo crear el directorio '{}': {e}", parent.display()),
                    ))
                })?;
            }
            Box::new(std::fs::File::create(path).map_err(|e| {
                ScraperError::Io(std::io::Error::new(
                    e.kind(),
                    format!("no se pudo crear el archivo de vectores '{path}': {e}"),
                ))
            })?)
        };
        Ok(Self {
            writer: Arc::new(Mutex::new(std::io::BufWriter::new(boxed))),
            titles: Arc::new(Mutex::new(HashMap::new())),
            clock,
        })
    }

    /// Build a sink over an arbitrary writer.
    ///
    /// Used by tests to inject deterministic, failure-simulating writers
    /// (e.g. a broken-pipe stub) without touching the filesystem or stdout.
    /// The wrapping matches [`StreamRepository::new`] exactly.
    #[cfg(test)]
    pub(crate) fn from_writer(w: Box<dyn Write + Send>) -> Self {
        Self {
            writer: Arc::new(Mutex::new(std::io::BufWriter::new(w))),
            titles: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(SystemUtcClock),
        }
    }
}

impl VectorRepository for StreamRepository {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        _content_hash: &'a str,
        _size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        let titles = Arc::clone(&self.titles);
        let url = url.to_string();
        let title = title.to_string();
        Box::pin(async move {
            if !title.is_empty() {
                // #1119: the HashMap insert runs on the blocking pool — the
                // std lock is never acquired on a Tokio worker, and a
                // poisoned lock is a typed error, not a panic.
                let key = url.clone();
                let join = tokio::task::spawn_blocking(move || -> Result<(), ScraperError> {
                    let mut cache = titles.lock().map_err(|_| lock_error("title cache"))?;
                    cache.insert(key, title);
                    Ok(())
                })
                .await;
                join.map_err(|e| {
                    ScraperError::Io(std::io::Error::other(format!(
                        "title cache blocking task join failed: {e}"
                    )))
                })??;
            }
            Ok(url)
        })
    }

    fn save_chunk<'a>(
        &'a self,
        id: &'a str,
        resource_url: &'a str,
        _chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        // Q3: without an embedding there is nothing to vectorize — omit the
        // record rather than emit a null/zero vector.
        let embedding = match embedding {
            Some(e) => e.to_vec(),
            None => return Box::pin(async { Ok(()) }),
        };

        let titles = Arc::clone(&self.titles);
        let writer = Arc::clone(&self.writer);
        let clock = Arc::clone(&self.clock);
        let id = id.to_string();
        let resource_url = resource_url.to_string();
        let content = content.to_string();

        Box::pin(async move {
            // #1119: title lookup, record build, JSON serialization and the
            // `write_all`/`flush` syscalls all run inside ONE blocking-pool
            // section. The writer `Mutex` serializes the two `write_all` +
            // `flush` calls so the JSONL stream stays line-oriented even
            // under concurrent `ElasticIngestion`, and no std lock or disk
            // write ever parks a Tokio worker.
            tokio::task::spawn_blocking(move || -> Result<(), ScraperError> {
                // The chunk id is formatted as "{sha256_hex}-{index}" by
                // `ElasticIngestion::run`, so the hash is the segment before the
                // first '-' (a SHA-256 hex string contains no '-').
                let sha256_hex = id.split('-').next().unwrap_or(id.as_str()).to_string();

                let title = titles
                    .lock()
                    .map_err(|_| lock_error("title cache"))?
                    .get(&resource_url)
                    .cloned();

                let record = VectorRecord {
                    url: resource_url,
                    sha256_hex,
                    title,
                    chunk_text: content,
                    embedding,
                    metadata: Value::Null,
                    timestamp: clock.now().to_rfc3339(),
                };

                let line = serde_json::to_string(&record)?;
                // D2: a broken pipe / WriteZero must surface as a fatal Io error so
                // the crawl aborts. `?` converts io::Error → ScraperError::Io.
                let mut writer = writer.lock().map_err(|_| lock_error("vector stream"))?;
                writer.write_all(line.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
                Ok(())
            })
            .await
            .map_err(|e| {
                ScraperError::Io(std::io::Error::other(format!(
                    "vector sink blocking task join failed: {e}"
                )))
            })?
        })
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        _content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        // No dedup for the stream sink — every chunk is emitted.
        Box::pin(async move { Ok(None) })
    }

    fn get_vector<'a>(
        &'a self,
        _chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(None) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_save_chunk_omits_record_without_embedding() {
        // Write to a temp file so we can assert no line is produced.
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_string_lossy().to_string();
        let repo = StreamRepository::new(&path).expect("open stream");

        repo.save_resource("https://example.com", "", "deadbeef", 10)
            .await
            .expect("save_resource");
        // No embedding → record omitted, write must succeed without a line.
        repo.save_chunk("deadbeef-0", "https://example.com", 0, "hello", None)
            .await
            .expect("save_chunk");

        let contents = std::fs::read_to_string(&path).expect("read stream");
        assert!(
            contents.trim().is_empty(),
            "no line expected when embedding is None"
        );
    }

    #[tokio::test]
    async fn test_save_chunk_emits_384_dim_embedding_and_hash() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_string_lossy().to_string();
        let repo = StreamRepository::new(&path).expect("open stream");

        let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
        repo.save_resource("https://example.com/p", "Page Title", "abc123def", 42)
            .await
            .expect("save_resource");
        repo.save_chunk(
            "abc123def-0",
            "https://example.com/p",
            0,
            "cleaned chunk text",
            Some(&embedding),
        )
        .await
        .expect("save_chunk");

        let contents = std::fs::read_to_string(&path).expect("read stream");
        let line = contents.lines().next().expect("one JSONL line");
        let record: VectorRecord = serde_json::from_str(line).expect("valid JSONL");

        assert_eq!(record.sha256_hex, "abc123def");
        assert_eq!(record.embedding.len(), 384, "explicit 384-dim embedding");
        assert_eq!(record.title.as_deref(), Some("Page Title"));
        assert_eq!(record.chunk_text, "cleaned chunk text");
    }

    // Collecting writer that buffers everything in memory so tests can inspect
    // the exact JSONL bytes that `StreamRepository` emits.
    /// D2 — a broken-pipe write error must surface as `Err(ScraperError::Io)`,
    /// never as a panic. Uses a deterministic in-memory writer stub (no OS pipes).
    #[tokio::test]
    async fn contract_broken_pipe_returns_err_not_panic() {
        struct BrokenPipeWriter;

        impl Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "simulated broken pipe",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let repo = StreamRepository::from_writer(Box::new(BrokenPipeWriter));
        let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();

        let result = repo
            .save_chunk(
                "deadbeefcafe-0",
                "https://example.com/p",
                0,
                "cleaned chunk text",
                Some(&embedding),
            )
            .await;

        assert!(
            result.is_err(),
            "broken pipe must return Err, not panic/panic"
        );
        let err = result.expect_err("broken pipe error");
        assert!(
            matches!(err, ScraperError::Io(_)),
            "broken pipe must map to ScraperError::Io, got: {err:?}"
        );
    }

    /// #1119 — a poisoned writer lock must surface as a typed `ScraperError::Io`
    /// from `save_chunk`, never as a panic inside the sink future (the old
    /// `.expect("vector stream poisoned")` aborted the ingestion task).
    #[tokio::test]
    async fn contract_poisoned_writer_lock_returns_err_not_panic() {
        struct NullWriter;
        impl Write for NullWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let repo = StreamRepository::from_writer(Box::new(NullWriter));
        // Poison the writer mutex: panic while holding the lock on a separate
        // thread, then wait for the panic to land.
        let writer = Arc::clone(&repo.writer);
        let poisoner = std::thread::spawn(move || {
            // `unwrap` is test-only: the lock is fresh and cannot fail here.
            let _guard = writer.lock().unwrap();
            panic!("poison the vector stream lock");
        });
        assert!(poisoner.join().is_err(), "poisoner must have panicked");

        let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
        let result = repo
            .save_chunk(
                "deadbeefcafe-0",
                "https://example.com/p",
                0,
                "cleaned chunk text",
                Some(&embedding),
            )
            .await;

        assert!(
            result.is_err(),
            "poisoned writer lock must return Err, not panic"
        );
        let err = result.expect_err("poison error");
        assert!(
            matches!(err, ScraperError::Io(_)),
            "poison must map to ScraperError::Io, got: {err:?}"
        );
    }

    /// 384-dim embedding integrity: round-trips exactly (same values + length)
    /// and the raw JSON line carries a 384-length array.
    #[tokio::test]
    async fn contract_embedding_384_dim_roundtrip() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_string_lossy().to_string();
        let repo = StreamRepository::new(&path).expect("open stream");

        let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
        repo.save_resource("https://example.com/p", "Title", "deadbeefcafe", 42)
            .await
            .expect("save_resource");
        repo.save_chunk(
            "deadbeefcafe-0",
            "https://example.com/p",
            0,
            "cleaned chunk text",
            Some(&embedding),
        )
        .await
        .expect("save_chunk");

        let contents = std::fs::read_to_string(&path).expect("read stream");
        let line = contents.lines().next().expect("one JSONL line");
        let record: VectorRecord = serde_json::from_str(line).expect("valid JSONL");

        assert_eq!(record.embedding.len(), 384, "embedding must be 384 floats");
        assert_eq!(
            record.embedding, embedding,
            "deserialized embedding must equal the original values"
        );

        // Raw JSON line carries a 384-length array in the "embedding" field.
        let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
        let arr = value
            .get("embedding")
            .expect("embedding field present")
            .as_array()
            .expect("embedding is a JSON array");
        assert_eq!(
            arr.len(),
            384,
            "raw JSON embedding array must be length 384"
        );
    }

    /// Lowercase-hex SHA-256 integrity: what upstream provides (a 32-char
    /// lowercase hex id) is written verbatim and stays lowercase.
    #[tokio::test]
    async fn contract_sha256_hex_is_lowercase_and_preserved() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_string_lossy().to_string();
        let repo = StreamRepository::new(&path).expect("open stream");

        let id = "abcdef0123456789abcdef0123456789-0";
        repo.save_resource(
            "https://example.com/p",
            "T",
            "abcdef0123456789abcdef0123456789",
            1,
        )
        .await
        .expect("save_resource");
        repo.save_chunk(
            id,
            "https://example.com/p",
            0,
            "cleaned chunk text",
            Some(&vec![0.0f32; 384]),
        )
        .await
        .expect("save_chunk");

        let contents = std::fs::read_to_string(&path).expect("read stream");
        let line = contents.lines().next().expect("one JSONL line");
        let record: VectorRecord = serde_json::from_str(line).expect("valid JSONL");

        assert_eq!(
            record.sha256_hex, "abcdef0123456789abcdef0123456789",
            "sha256_hex must be preserved verbatim from the id"
        );

        let is_lowercase_hex = record
            .sha256_hex
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            && record.sha256_hex.len() == 32;
        assert!(
            is_lowercase_hex,
            "sha256_hex must be 32 lowercase hex chars (no uppercase), got: {}",
            record.sha256_hex
        );
    }
}

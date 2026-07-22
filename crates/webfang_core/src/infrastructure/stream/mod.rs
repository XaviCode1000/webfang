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
//! - **Concurrency:** writes are serialized through a `Mutex` so the JSONL stream
//!   stays line-oriented even when [`ElasticIngestion`] processes URLs concurrently.

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
pub struct StreamRepository {
    /// Serialized JSONL writer. `Box<dyn Write + Send>` so both `Stdout` and
    /// `File` fit behind one type; the `Mutex` keeps the stream line-oriented
    /// under concurrent ingestion.
    writer: Mutex<std::io::BufWriter<Box<dyn Write + Send>>>,
    /// Title cache keyed by resource URL, populated by [`VectorRepository::save_resource`]
    /// and read by [`VectorRepository::save_chunk`] (the chunk call only receives
    /// `resource_url`, not the title).
    titles: Mutex<HashMap<String, String>>,
    /// Injected clock for deterministic timestamps in tests.
    clock: Arc<dyn UtcClock>,
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
            writer: Mutex::new(std::io::BufWriter::new(boxed)),
            titles: Mutex::new(HashMap::new()),
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
            writer: Mutex::new(std::io::BufWriter::new(w)),
            titles: Mutex::new(HashMap::new()),
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
        Box::pin(async move {
            if !title.is_empty() {
                self.titles
                    .lock()
                    .expect("title cache poisoned")
                    .insert(url.to_string(), title.to_string());
            }
            Ok(url.to_string())
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
        Box::pin(async move {
            // Q3: without an embedding there is nothing to vectorize — omit the
            // record rather than emit a null/zero vector.
            let embedding = match embedding {
                Some(e) => e.to_vec(),
                None => return Ok(()),
            };

            // The chunk id is formatted as "{sha256_hex}-{index}" by
            // `ElasticIngestion::run`, so the hash is the segment before the
            // first '-' (a SHA-256 hex string contains no '-').
            let sha256_hex = id.split('-').next().unwrap_or(id).to_string();

            let title = self
                .titles
                .lock()
                .expect("title cache poisoned")
                .get(resource_url)
                .cloned();

            let record = VectorRecord {
                url: resource_url.to_string(),
                sha256_hex,
                title,
                chunk_text: content.to_string(),
                embedding,
                metadata: Value::Null,
                timestamp: self.clock.now().to_rfc3339(),
            };

            let line = serde_json::to_string(&record)?;
            // D2: a broken pipe / WriteZero must surface as a fatal Io error so
            // the crawl aborts. `?` converts io::Error → ScraperError::Io.
            let mut writer = self.writer.lock().expect("vector stream poisoned");
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
            Ok(())
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

    #[test]
    fn test_save_chunk_omits_record_without_embedding() {
        // Write to a temp file so we can assert no line is produced.
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_string_lossy().to_string();
        let repo = StreamRepository::new(&path).expect("open stream");

        futures::executor::block_on(async {
            repo.save_resource("https://example.com", "", "deadbeef", 10)
                .await
                .expect("save_resource");
            // No embedding → record omitted, write must succeed without a line.
            repo.save_chunk("deadbeef-0", "https://example.com", 0, "hello", None)
                .await
                .expect("save_chunk");
        });

        let contents = std::fs::read_to_string(&path).expect("read stream");
        assert!(
            contents.trim().is_empty(),
            "no line expected when embedding is None"
        );
    }

    #[test]
    fn test_save_chunk_emits_384_dim_embedding_and_hash() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_string_lossy().to_string();
        let repo = StreamRepository::new(&path).expect("open stream");

        let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
        futures::executor::block_on(async {
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
        });

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
    #[test]
    fn contract_broken_pipe_returns_err_not_panic() {
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

        let result = futures::executor::block_on(async {
            repo.save_chunk(
                "deadbeefcafe-0",
                "https://example.com/p",
                0,
                "cleaned chunk text",
                Some(&embedding),
            )
            .await
        });

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

    /// 384-dim embedding integrity: round-trips exactly (same values + length)
    /// and the raw JSON line carries a 384-length array.
    #[test]
    fn contract_embedding_384_dim_roundtrip() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_string_lossy().to_string();
        let repo = StreamRepository::new(&path).expect("open stream");

        let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
        futures::executor::block_on(async {
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
        });

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
    #[test]
    fn contract_sha256_hex_is_lowercase_and_preserved() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_string_lossy().to_string();
        let repo = StreamRepository::new(&path).expect("open stream");

        let id = "abcdef0123456789abcdef0123456789-0";
        futures::executor::block_on(async {
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
        });

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

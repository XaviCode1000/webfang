//! Filesystem binary writer adapter.
//!
//! Production [`BinaryWriterPort`]
//! implementation. Extracted from the application layer (issue #442) so the
//! single-URL scrape use case no longer calls `std::fs` directly — the
//! application depends on the domain port, this adapter provides the real I/O.

use std::path::Path;

use crate::domain::ports::BinaryWriterPort;
use crate::error::{Result, ScraperError};

/// Filesystem-backed [`BinaryWriterPort`].
///
/// Creates the parent directory tree (if any) and writes the payload bytes
/// synchronously. This is the default writer used when a scrape use case
/// receives no injected writer (`None` fallback).
#[derive(Debug, Default, Clone, Copy)]
pub struct FsBinaryWriter;

impl FsBinaryWriter {
    /// Create a new filesystem binary writer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl BinaryWriterPort for FsBinaryWriter {
    fn write_bytes(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ScraperError::Io)?;
        }
        std::fs::write(path, bytes).map_err(ScraperError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_bytes_creates_file_with_exact_contents() {
        let dir = TempDir::new().expect("temp dir");
        let target = dir.path().join("doc.pdf");
        let payload = b"%PDF-1.4 fake bytes";

        FsBinaryWriter::new()
            .write_bytes(&target, payload)
            .expect("write succeeds");

        let read_back = std::fs::read(&target).expect("file readable");
        assert_eq!(read_back, payload, "bytes on disk must match the payload");
    }

    #[test]
    fn write_bytes_creates_missing_parent_directories() {
        let dir = TempDir::new().expect("temp dir");
        let target = dir.path().join("nested").join("deep").join("file.bin");

        FsBinaryWriter::new()
            .write_bytes(&target, b"abc")
            .expect("write succeeds into a not-yet-existing subtree");

        assert!(target.exists(), "file must exist after parent creation");
    }
}

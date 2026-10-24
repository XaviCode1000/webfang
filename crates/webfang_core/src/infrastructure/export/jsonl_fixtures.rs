//! Fixtures shared by the JSONL export unit tests.
//!
//! `jsonl_writer` and `tail_window` exercise the same line shape; one
//! canonical definition per fixture keeps the duplication ratchet honest.

use std::io::Write;
use std::path::Path;

/// The canonical valid export line for test `n`.
pub(crate) fn valid_line(n: u32) -> String {
    format!(r#"{{"n":{n},"checksum_sha256":"hash-{n}"}}"#)
}

/// `line` exactly as the writer emits it: bytes with the trailing newline.
pub(crate) fn terminated(line: &str) -> Vec<u8> {
    let mut bytes = line.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes
}

/// Seed `path` with `len` unterminated filler bytes and return them.
///
/// The unit tests use this for tails longer than the recovery window cap;
/// the integration suite seeds its own copy across the crate boundary.
pub(crate) fn seed_unterminated(path: &Path, len: usize) -> std::io::Result<Vec<u8>> {
    let content = vec![b'x'; len];
    let mut file = std::fs::File::create(path)?;
    file.write_all(&content)?;
    Ok(content)
}

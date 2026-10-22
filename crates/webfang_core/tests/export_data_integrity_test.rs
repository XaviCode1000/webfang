//! Export-path data integrity (issue #1812).
//!
//! # The DECIDED policy for a non-UTF8 JSONL
//!
//! A JSONL file (or line) that is not valid UTF-8 **degrades and skips,
//! loudly**: the export COMPLETES, the damaged line is skipped, the records
//! around it survive untouched, and the skip is reported. One damaged byte
//! must never abort the whole export.
//!
//! The reportable half of the policy — the structured error and the count of
//! skipped entries in the export summary — is pinned in the module that owns
//! it (`application::export_factory`'s own tests). This file pins the
//! end-to-end BEHAVIOR through the public export ports, on both the
//! standard and the `--clean-ai` path.

use webfang_core::application::export_factory::{process_results, process_results_with_chunks};
use webfang_core::domain::entities::ExportFormat;
use webfang_core::domain::{DocumentChunkUnvalidated, ScrapedContent, ValidUrl};

/// Bytes that are not valid UTF-8 — the same input class the config
/// loader's contract test uses (`[0xff, 0xfe, 0x00]`).
const INVALID_UTF8_LINE: [u8; 3] = [0xff, 0xfe, 0x00];

fn scraped(url: &str, title: &str, body: &str) -> ScrapedContent {
    ScrapedContent {
        title: title.to_string(),
        content: body.to_string(),
        url: ValidUrl::parse(url).expect("valid url"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    }
}

/// An export file that already holds one valid record and one line that is
/// not valid UTF-8.
fn write_damaged_export(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("export.jsonl");
    let mut bytes =
        b"{\"url\":\"https://kept.test/a\",\"content\":\"kept\",\"checksum_sha256\":\"aaaa\"}\n"
            .to_vec();
    bytes.extend_from_slice(&INVALID_UTF8_LINE);
    bytes.push(b'\n');
    std::fs::write(&path, &bytes).expect("damaged export file");
    path
}

/// The export file's bytes, decoded leniently: the file is not valid UTF-8,
/// so `read_to_string` would fail on the very bytes under test.
fn export_bytes(path: &std::path::Path) -> String {
    String::from_utf8_lossy(&std::fs::read(path).expect("export file")).into_owned()
}

/// The DECIDED policy: a damaged line is skipped, the export COMPLETES, and
/// the records around it are untouched.
#[test]
fn a_non_utf8_line_does_not_abort_the_export() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = write_damaged_export(dir.path());

    let processed = process_results(
        &[scraped("https://new.test/b", "B", "body of the new record")],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        None,
    )
    .expect("the export must COMPLETE even with a damaged line");

    assert_eq!(processed.len(), 1, "the new record is exported");
    let body = export_bytes(&path);
    assert!(
        body.contains("kept.test/a"),
        "the surviving record must stay: {body}"
    );
    assert!(
        body.contains("new.test/b"),
        "the new record must be appended: {body}"
    );
}

/// The same policy on the `--clean-ai` path: the AI export opens the same
/// file through the same reader.
#[test]
fn a_non_utf8_line_does_not_abort_the_ai_export() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = write_damaged_export(dir.path());

    let chunk = DocumentChunkUnvalidated::new(
        uuid::Uuid::new_v4(),
        "https://new.test/c",
        "C",
        "body of the cleaned chunk",
    );

    let processed = process_results_with_chunks(
        &[chunk],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        None,
    )
    .expect("the --clean-ai export must COMPLETE even with a damaged line");

    assert_eq!(processed.len(), 1, "the cleaned chunk is exported");
    let body = export_bytes(&path);
    assert!(
        body.contains("new.test/c"),
        "the new record must be appended: {body}"
    );
}

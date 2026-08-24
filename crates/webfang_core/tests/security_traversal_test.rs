//! Security: path-traversal containment for hostile filenames (batch 1).
//!
//! A malicious server controls `Content-Disposition` filenames and URL path
//! segments. Every file WebFang writes MUST stay INSIDE the configured output
//! directory, no matter what the server sends:
//!
//! - Direct binary download (`--download-documents`): a PDF response whose
//!   `Content-Disposition` filename is `../../escape.bin` must not escape.
//! - Asset downloads (`--download-assets --asset-naming content-disposition`):
//!   the same hostile-header surface through the asset downloader.
//! - Regression smoke: percent-encoded `%2e%2e%2f` URL segments (already
//!   neutralized by `UrlPath`) keep writing inside the output directory.
//!
//! Containment is proven structurally: every regular file under the sandbox
//! TempDir must live inside the nested output directory. The output dir is
//! nested two levels deep (`<sandbox>/lvl1/output`) so a single `../..`
//! escape lands inside the sandbox where the walk can observe it — nothing
//! ever touches the real filesystem outside the TempDir.
//!
//! Run with: `cargo nextest run --test security_traversal_test`

#[path = "common/cli_harness.rs"]
mod common;
use common::{cmd, BehavioralTest};

use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Minimal page: one article node with a linked document so readability keeps
/// it as main content and the asset pipeline discovers `/doc.pdf`.
const PAGE_WITH_DOC: &str = r#"
<html><body><article>
<h1>Traversal Probe</h1>
<p>Content paragraph long enough so readability scores this node as the
main article body and the crawl pipeline runs end to end.</p>
<a href="/doc.pdf">Download report</a>
</article></body></html>
"#;

/// Minimal scrapable page without assets (for the UrlPath smoke test).
const MINIMAL_PAGE: &str = r#"
<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><title>Encoded Probe</title></head>
<body>
<article>
<h1>Encoded Segment Probe</h1>
<p>Filler paragraph long enough so readability keeps this node and scores it
as the main article body, proving the scrape pipeline ran end to end.</p>
</article>
</body></html>"#;

const PDF_BYTES: &[u8] = b"%PDF-1.4 fake pdf content for traversal containment testing";

/// Assert every regular file under `root` lives inside `allowed`.
///
/// Lexical component check (`Path::starts_with`); `WalkDir` never follows
/// symlinks by default, so this observes exactly what was written.
fn assert_all_files_within(root: &Path, allowed: &Path) {
    let escaped: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .filter(|p| !p.starts_with(allowed))
        .collect();
    assert!(
        escaped.is_empty(),
        "files escaped the output dir {allowed:?}: {escaped:?}"
    );
}

/// Nested output dir two levels below the sandbox TempDir root: a single
/// `../..` hostile filename escapes the output dir but stays observable
/// inside the sandbox (auto-cleaned by TempDir).
fn nested_output(harness: &BehavioralTest) -> PathBuf {
    harness.out.path().join("lvl1").join("output")
}

// ---------------------------------------------------------------------------
// Surface 1b: direct binary download (--download-documents)
// ---------------------------------------------------------------------------

/// Hostile server sends `Content-Disposition: attachment; filename="../../escape.bin"`
/// on a PDF response fetched directly by URL. The saved file must stay INSIDE
/// the output directory.
#[tokio::test]
async fn content_disposition_traversal_filename_cannot_escape_output_dir() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;
    let output = nested_output(&harness);

    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(PDF_BYTES.to_vec())
                .insert_header("content-type", "application/pdf")
                .insert_header(
                    "content-disposition",
                    "attachment; filename=\"../../escape.bin\"",
                ),
        )
        .expect(1)
        .mount(server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/report.pdf", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(&output)
        .arg("--quiet")
        .assert()
        .success();

    assert_all_files_within(harness.out.path(), &output);

    // The bytes must still be saved (sanitized), not silently dropped.
    let saved: Vec<PathBuf> = WalkDir::new(&output)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| std::fs::read(e.path()).is_ok_and(|bytes| bytes == PDF_BYTES))
        .map(|e| e.path().to_path_buf())
        .collect();
    assert!(
        !saved.is_empty(),
        "the downloaded payload should be preserved (under a sanitized name) in {output:?}"
    );
}

/// Bare `..` filename: joining it onto the output dir resolves to the PARENT
/// directory. The run must succeed without writing anything outside the
/// output directory.
#[tokio::test]
async fn content_disposition_dotdot_filename_stays_inside_output_dir() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;
    let output = nested_output(&harness);

    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(PDF_BYTES.to_vec())
                .insert_header("content-type", "application/pdf")
                .insert_header("content-disposition", "attachment; filename=\"..\""),
        )
        .expect(1)
        .mount(server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/report.pdf", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(&output)
        .arg("--quiet")
        .assert()
        .success();

    assert_all_files_within(harness.out.path(), &output);
}

/// Absolute-path filename: `Path::join` with an absolute path REPLACES the
/// base entirely, so an unsanitized filename like `<abs>/abs_escape.bin`
/// would write wherever the server points. It must be relocated inside the
/// output directory instead.
#[tokio::test]
async fn absolute_path_content_disposition_filename_is_contained() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;
    let output = nested_output(&harness);
    // Absolute target OUTSIDE the output dir but INSIDE the sandbox TempDir,
    // so the pre-fix escape is observable and auto-cleaned.
    let hostile_absolute = harness.out.path().join("lvl1").join("abs_escape.bin");

    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(PDF_BYTES.to_vec())
                .insert_header("content-type", "application/pdf")
                .insert_header(
                    "content-disposition",
                    format!("attachment; filename=\"{}\"", hostile_absolute.display()),
                ),
        )
        .expect(1)
        .mount(server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/report.pdf", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(&output)
        .arg("--quiet")
        .assert()
        .success();

    assert_all_files_within(harness.out.path(), &output);
}

/// NUL byte smuggled via RFC 5987 encoding (`filename*=UTF-8''nul%00byte.pdf`).
/// The decoded name contains a raw NUL which Unix filesystems reject — the
/// sanitized name must strip it and the payload must be saved inside the
/// output directory.
#[tokio::test]
async fn null_byte_filename_is_sanitized_and_saved_inside_output_dir() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;
    let output = nested_output(&harness);

    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(PDF_BYTES.to_vec())
                .insert_header("content-type", "application/pdf")
                .insert_header(
                    "content-disposition",
                    "attachment; filename*=UTF-8''nul%00byte.pdf",
                ),
        )
        .expect(1)
        .mount(server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/report.pdf", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(&output)
        .arg("--quiet")
        .assert()
        .success();

    assert_all_files_within(harness.out.path(), &output);

    let saved: Vec<PathBuf> = WalkDir::new(&output)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| std::fs::read(e.path()).is_ok_and(|bytes| bytes == PDF_BYTES))
        .map(|e| e.path().to_path_buf())
        .collect();
    assert!(
        !saved.is_empty(),
        "the payload should survive sanitization of the NUL byte in {output:?}"
    );
}

// ---------------------------------------------------------------------------
// Surface 1b (assets): downloader with ContentDisposition naming strategy
// ---------------------------------------------------------------------------

/// Asset download via `--asset-naming content-disposition`: the asset response
/// carries a hostile `../../escape_asset.bin` filename that the downloader
/// joins onto `documents/`. The file must stay INSIDE the output directory.
#[tokio::test]
async fn asset_download_traversal_content_disposition_cannot_escape_output_dir() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;
    let output = nested_output(&harness);

    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_WITH_DOC))
        .expect(1)
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/doc.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(PDF_BYTES.to_vec())
                .insert_header("content-type", "application/pdf")
                .insert_header(
                    "content-disposition",
                    "attachment; filename=\"../../escape_asset.bin\"",
                ),
        )
        .expect(1)
        .mount(server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/page", server.uri()))
        .arg("--single-page")
        .arg("--download-assets")
        .arg("--asset-naming")
        .arg("content-disposition")
        .arg("--output")
        .arg(&output)
        .arg("--quiet")
        .assert()
        .success();

    assert_all_files_within(harness.out.path(), &output);
}

// ---------------------------------------------------------------------------
// Smoke guard: already-good UrlPath surface (%2e%2e%2f URL segment)
// ---------------------------------------------------------------------------

/// End-to-end regression guard: a page URL containing a percent-encoded
/// `%2e%2e%2f` segment must produce its Markdown INSIDE the output directory
/// (UrlPath::sanitize_path_segment already guarantees this — cheap tripwire).
#[tokio::test]
async fn percent_encoded_dotdot_url_segment_writes_inside_output_dir() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;
    let output = nested_output(&harness);

    // Catch-all mock: the exact wire form of the encoded path after client
    // normalization is not part of this contract — only WHERE files land.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MINIMAL_PAGE))
        .mount(server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/%2e%2e%2fpage", server.uri()))
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--output")
        .arg(&output)
        .arg("--quiet")
        .assert()
        .success();

    assert_all_files_within(harness.out.path(), &output);

    let md_inside = WalkDir::new(&output)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "md"));
    assert!(
        md_inside,
        "expected a scraped .md INSIDE the output dir at {output:?}"
    );
}

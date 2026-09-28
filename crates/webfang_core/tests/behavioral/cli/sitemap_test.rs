//! Sitemap-based discovery: --use-sitemap with explicit --sitemap-url.

use crate::assert_snapshot_redacted;
use crate::cmd;
use tempfile::TempDir;
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// --sitemap-url with --use-sitemap fetches the explicit sitemap URL and
/// scrapes the URLs listed in it.
///
/// The crawl output is snapshotted per file, in sorted path order, rather
/// than substring-matched: the two `contains("Page A")` / `contains("Page B")`
/// asserts this replaces would have passed on a single page, could not see
/// which files were written, and said nothing about the exported Markdown.
/// A per-file diff names the page a regression landed on.
#[tokio::test]
async fn sitemap_url_scrapes_listed_urls() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Seed page (may or may not be fetched — sitemap discovery takes precedence)
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Seed Page</h1>\
                 <p>Seed content.</p>\
                 </article></body></html>",
        ))
        .mount(&server)
        .await;

    // robots.txt (empty — allow all)
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
        .mount(&server)
        .await;

    // Explicit sitemap listing two pages
    let base = server.uri();
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/page-a</loc></url>
    <url><loc>{base}/page-b</loc></url>
</urlset>"#,
        )))
        .mount(&server)
        .await;

    // Pages listed in the sitemap
    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Page A</h1>\
                 <p>Substantive content from sitemap page A, long enough to clear the \
                 fifty character minimum content guard comfortably.</p>\
                 </article></body></html>",
        ))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Page B</h1>\
                 <p>Substantive content from sitemap page B, long enough to clear the \
                 fifty character minimum content guard comfortably.</p>\
                 </article></body></html>",
        ))
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{}/sitemap.xml", server.uri()))
        .arg("--output")
        .arg(output.path())
        .arg("--max-pages")
        .arg("5")
        .arg("--quiet")
        .assert()
        .success();

    // Snapshot every exported file, per file and in sorted path order, so a
    // failure diff reads as "this page changed", not "somewhere in the
    // concatenation". Sorting is what makes it deterministic: `WalkDir` does
    // not promise an order.
    let mut files: Vec<std::path::PathBuf> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect();
    files.sort();
    assert!(!files.is_empty(), "the crawl must export at least one file");

    let mut exported = String::new();
    for file in &files {
        let relative = file
            .strip_prefix(output.path())
            .expect("WalkDir yields paths under the output dir");
        // The snapshot heading is a document line, and the document format
        // requires `/` as the separator on every platform. `display()` is
        // platform-native, so on Windows it would render
        // `## 127.0.0.1\page-a.md` against a Linux-authored snapshot. Only
        // the heading text is normalized — the tree the exporter wrote on
        // disk is untouched and already correct (#1631).
        let heading = relative.to_string_lossy().replace('\\', "/");
        exported.push_str(&format!("## {heading}\n"));
        exported.push_str(&canonical_export(file));
        if !exported.ends_with('\n') {
            exported.push('\n');
        }
    }

    assert_snapshot_redacted("sitemap_url_scrapes_listed_urls", output.path(), exported);
}

/// Render one exported file for snapshotting, comparing raw JSONL bytes.
///
/// The JSONL writer emits `extra_metadata` as a `BTreeMap<String, String>`
/// flattened into the record (#1595), so the key order in the serialized
/// line is lexicographic and byte-stable across processes and runs. Raw
/// lines are therefore compared verbatim — no `serde_json::Value`
/// round-trip remains. Every field — including `checksum_sha256` — is
/// asserted exactly as written.
///
/// This asserts the production ordering guarantee end to end:
/// `sitemap_crawl_run_staleness_test.rs` in webfang_mcp uses the same idiom
/// for the same field. Non-JSONL files (Markdown) are returned untouched.
fn canonical_export(file: &std::path::Path) -> String {
    let raw =
        std::fs::read_to_string(file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    if file.extension().is_none_or(|ext| ext != "jsonl") {
        return raw;
    }

    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

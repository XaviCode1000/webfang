//! JSONL byline-artifact regression tests (#800 slice 1, non-AI path).
//!
//! Reproduces the quotes.toscrape.com audit (2026-08-17) against the FULL
//! binary pipeline with wiremock — no real network:
//!
//! - Readability (legible) captures the first author node (`<small
//!   itemprop="author">`) as the page byline and STRIPS it from the content,
//!   leaving the wrapper's `by ` prefix plus the sibling `(about)` anchor
//!   behind. The excerpt therefore ships `… by  (about)` (empty author slot).
//! - #800 fix: the excerpt is repaired at the extraction SOURCE (the new
//!   `domain::excerpt_repair` module), so every downstream sink — including
//!   the standard JSONL export — observes the repaired excerpt. This path
//!   does NOT touch `--clean-ai`; it proves the non-AI JSONL sink (issue #800)
//!   no longer emits the broken fragment.

#[path = "common/cli_harness.rs"]
mod common;
use common::{cmd, redact_nondeterministic};

use serde_json::Value;
use std::path::Path;
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Minimal quotes.toscrape.com page shape: repeated `.quote` blocks whose
/// byline span wraps the author node and a sibling `(about)` anchor.
const QUOTES_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><title>Quotes to Scrape</title></head>
<body>
<div class="row">
<div class="col-md-8">
<div class="quote" itemscope itemtype="http://schema.org/CreativeWork">
    <span class="text" itemprop="text">&#8220;The world as we have created it is a process of our thinking. It cannot be changed without changing our thinking.&#8221;</span>
    <span>by <small class="author" itemprop="author">Albert Einstein</small>
    <a href="/author/Albert-Einstein/">(about)</a>
    </span>
    <div class="tags">
        Tags:
        <a class="tag" href="/tag/change/page/1/">change</a>
    </div>
</div>
<div class="quote" itemscope itemtype="http://schema.org/CreativeWork">
    <span class="text" itemprop="text">&#8220;It is our choices, Harry, that show what we truly are, far more than our abilities.&#8221;</span>
    <span>by <small class="author" itemprop="author">J.K. Rowling</small>
    <a href="/author/JK-Rowling/">(about)</a>
    </span>
</div>
</div>
</div>
</body></html>"#;

/// Canonicalize each JSONL line (parse → re-serialize) so map key order is
/// deterministic. `serde_json` writes `HashMap`-backed fields in iteration
/// order (non-deterministic per process — RandomState), while a parsed
/// `Value` re-serializes its keys sorted (BTreeMap-backed map), so the
/// snapshot is stable run-to-run.
fn canonicalize_jsonl(content: &str) -> String {
    content
        .lines()
        .map(|line| match serde_json::from_str::<Value>(line) {
            Ok(value) => serde_json::to_string(&value).expect("re-serialize canonical value"),
            Err(_) => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Snapshot helper: canonicalize JSONL key order, then redact the temp dir +
/// port + timestamp so the JSONL is deterministic run-to-run.
fn snapshot_jsonl_byline(name: &str, dir: &Path, content: &str) {
    let canonical = canonicalize_jsonl(content);
    let redacted = redact_nondeterministic(dir, &canonical);
    insta::assert_snapshot!(name, redacted);
}

fn read_all_jsonl(out: &Path) -> String {
    let mut contents: Vec<String> = WalkDir::new(out)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .collect();
    assert!(!contents.is_empty(), "expected at least one .jsonl file");
    contents.sort();
    contents.join("\n---\n")
}

fn scrape_quotes_jsonl(server: &MockServer, out: &Path) {
    let mut command = cmd();
    command
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--output")
        .arg(out)
        .arg("--quiet");
    command.assert().success();
}

/// #800 proof: the standard (non-AI) JSONL sink ships the REPAIRED excerpt.
///
/// The Readability empty-byline scar (`by (about)` / `by  (about)`) must not
/// reach `extra_metadata.excerpt` of the emitted record — the excerpt is
/// repaired at the extraction source, so every downstream sink observes the
/// completed byline ("by Albert Einstein").
#[tokio::test]
async fn jsonl_excerpt_repaired() {
    let server = MockServer::start().await;
    let out = tempfile::tempdir().expect("tempdir");
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(QUOTES_PAGE))
        .expect(1)
        .mount(&server)
        .await;

    scrape_quotes_jsonl(&server, out.path());

    let content = read_all_jsonl(out.path());
    snapshot_jsonl_byline("jsonl_excerpt_repaired", out.path(), &content);

    // Semantic invariant beyond the snapshot: parse the record and assert the
    // repair holds in the excerpt field specifically (the `content` body may
    // legitimately retain the source fragment — only the excerpt is repaired).
    let record: Value = serde_json::from_str(content.lines().next().expect("one JSONL record"))
        .expect("JSONL record must parse");
    let excerpt = record["extra_metadata"]["excerpt"]
        .as_str()
        .expect("excerpt field must be present");
    assert!(
        !excerpt.contains("by (about)") && !excerpt.contains("by  (about)"),
        "empty-byline fragment must not reach the JSONL excerpt: {excerpt}"
    );
}

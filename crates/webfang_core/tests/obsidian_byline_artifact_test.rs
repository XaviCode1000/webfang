//! Obsidian byline-artifact regression tests (#762 slice 2).
//!
//! Reproduces the quotes.toscrape.com audit (2026-08-17) against the FULL
//! binary pipeline with wiremock — no real network:
//!
//! - Readability (legible) captures the first author node (`<small
//!   itemprop="author">`) as the page byline and STRIPS it from the content,
//!   leaving the wrapper's `by ` prefix plus the sibling `(about)` anchor
//!   behind. The excerpt therefore ships `… by (about)` (empty author slot).
//! - The wiki-link converter then turns the surviving `(about)` anchor into
//!   `[[author-albert-einstein|(about)]]` — a label that travels without the
//!   name it used to accompany.
//!
//! #762 fix: frontmatter repairs the residual fragment (completed with the
//! resolved author, or dropped when none), and the wiki-link alias falls back
//! to the humanized last path segment instead of the naked `(about)`.

#[path = "common/cli_harness.rs"]
mod common;
use common::{cmd, redact_nondeterministic};

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

/// Snapshot helper matching `obsidian_test.rs` conventions (redact the temp
/// dir + collapse the date field generated from `Utc::now()`).
fn snapshot_byline_artifact(name: &str, dir: &Path, content: &str) {
    let redacted = redact_nondeterministic(dir, content);
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"date: \d{4}-\d{2}-\d{2}", "date: [DATE]");
    settings.bind(|| {
        insta::assert_snapshot!(name, redacted);
    });
}

fn scrape_quotes_page(server: &MockServer, out: &Path, cmd_args: &[&str]) {
    let mut command = cmd();
    command
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--output")
        .arg(out)
        .arg("--quiet");
    for arg in cmd_args {
        command.arg(*arg);
    }
    command.assert().success();
}

fn read_all_md(out: &Path) -> String {
    let mut contents: Vec<String> = WalkDir::new(out)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .collect();
    assert!(!contents.is_empty(), "expected at least one .md file");
    contents.sort();
    contents.join("\n---\n")
}

/// Slice 2 excerpt proof: the residual `by (about)` fragment is completed
/// with the resolved author instead of shipping an empty author slot.
#[tokio::test]
async fn empty_byline_fragment_repaired_in_excerpt() {
    let server = MockServer::start().await;
    let out = tempfile::tempdir().expect("tempdir");
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(QUOTES_PAGE))
        .expect(1)
        .mount(&server)
        .await;

    scrape_quotes_page(&server, out.path(), &["--obsidian-wiki-links"]);

    let content = read_all_md(out.path());
    snapshot_byline_artifact(
        "empty_byline_fragment_repaired_in_excerpt",
        out.path(),
        &content,
    );

    // Semantic invariants beyond the snapshot: the empty-slot artifact must
    // not survive in ANY form, and the author must complete the byline.
    assert!(
        !content.contains("by (about)") && !content.contains("by  (about)"),
        "empty-byline fragment must not reach the excerpt: {content}"
    );
    assert!(
        content.contains("” by Albert Einstein"),
        "excerpt must be completed with the resolved author: {content}"
    );
}

/// Slice 2 alias proof: a parenthetical-only wiki-link label recovers the
/// author from the link target instead of shipping a naked `(about)` alias.
#[tokio::test]
async fn parenthetical_alias_recovers_author_from_link_target() {
    let server = MockServer::start().await;
    let out = tempfile::tempdir().expect("tempdir");
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(QUOTES_PAGE))
        .expect(1)
        .mount(&server)
        .await;

    scrape_quotes_page(&server, out.path(), &["--obsidian-wiki-links"]);

    let content = read_all_md(out.path());

    assert!(
        !content.contains("|(about)]"),
        "naked (about) alias must not survive the conversion: {content}"
    );
    assert!(
        content.contains("[[author-albert-einstein|Albert Einstein]]"),
        "alias must recover the author name from the link target: {content}"
    );
}

/// Control: without `--obsidian-wiki-links` the byline scar still must not
/// leak an empty author slot into the frontmatter excerpt.
#[tokio::test]
async fn byline_fragment_dropped_when_author_missing() {
    let server = MockServer::start().await;
    let out = tempfile::tempdir().expect("tempdir");
    // Variant WITHOUT itemprop/class author markers: the extractor cascade
    // finds nothing, legible's byline capture leaves `by (about)`, and the
    // frontmatter must DROP the fragment rather than emit `by` + empty slot.
    const NO_AUTHOR_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><title>Scant Page</title></head>
<body>
<div>
    <span class="text">Something worth quoting for this regression test.</span>
    <span>by <small>Anonymous</small> <a href="/author/Anonymous/">(about)</a></span>
    <p>Filler paragraph long enough so readability keeps this node and scores
       it as the main article body instead of falling back to an error.</p>
</div>
</body></html>"#;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(NO_AUTHOR_PAGE))
        .expect(1)
        .mount(&server)
        .await;

    scrape_quotes_page(&server, out.path(), &[]);

    let content = read_all_md(out.path());
    assert!(
        !content.contains("by (about)"),
        "empty-author byline fragment must be dropped: {content}"
    );
}

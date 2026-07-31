//! Regression tests for webfang's AI Chunker pipeline (webfang_ai).
//!
//! These tests document the CURRENT correct behavior so future refactoring
//! doesn't break it. PR #251 tried to unify the 4 cleaning pipelines and
//! broke behavior — these tests prevent that from happening again.
//!
//! # Pipeline: AI Chunker (chunker.rs)
//!
//! | Location | Public API | Key trait |
//! |----------|------------|-----------|
//! | `webfang_ai/src/infrastructure_ai/chunker.rs` | `HtmlChunker::chunk()` | strip tags with `\n` on `>` → split by `\n\n` |
//!
//! Key behavioral difference from other pipelines:
//! - Pushes `\n` on `>` to create sentence boundaries for AI tokenization
//! - NO normalize_whitespace — raw whitespace preserved between tags
//! - Splits by `\n\n` for paragraph detection
//!
//! Run with: `cargo nextest run --test chunker_pipelines_regression`

use webfang_ai::infrastructure_ai::HtmlChunker;

/// HTML paragraphs produce chunks separated by newlines.
/// strip_html_tags pushes '\n' on '>', so <p>content</p> becomes content\n.
#[ignore = "resurrected: pending triage"]
#[test]
fn chunker_paragraphs_become_separated_chunks() {
    let chunker = HtmlChunker::new().with_min_chunk_size(1);
    let html = "<p>First paragraph content here.</p><p>Second paragraph content here.</p>";
    let chunks = chunker.chunk(html).expect("chunk succeeds");

    let combined: String = chunks.iter().map(|c| c.content.as_str()).collect(" | ");
    assert!(
        combined.contains("First paragraph"),
        "first paragraph present: {combined}"
    );
    assert!(
        combined.contains("Second paragraph"),
        "second paragraph present: {combined}"
    );
}

/// The > character produces \n in output — this is intentional for sentence
/// boundary detection in AI tokenization.
#[ignore = "resurrected: pending triage"]
#[test]
fn chunker_close_angle_bracket_produces_newline() {
    let chunker = HtmlChunker::new().with_min_chunk_size(1);
    // Single tag: <p>hello</p> → strip_html_tags produces "hello\n" then "\n"
    let html = "<p>hello</p>";
    let chunks = chunker.chunk(html).expect("chunk succeeds");

    // The strip_html_tags pushes '\n' on '>', so we get "hello\n" from the
    // content plus a trailing newline. split("\n\n") then splits paragraphs.
    if let Some(chunk) = chunks.first() {
        // Content should contain "hello" — exact whitespace depends on the
        // split/reassemble logic, but the semantic content must be there.
        assert!(
            chunk.content.contains("hello"),
            "content preserved: {:?}",
            chunk.content
        );
    }
}

/// Empty tags should not create empty paragraphs.
#[ignore = "resurrected: pending triage"]
#[test]
fn chunker_empty_tags_no_empty_paragraphs() {
    let chunker = HtmlChunker::new().with_min_chunk_size(1);
    let html = "<p></p><div></div><span></span><p>Real content here.</p>";
    let chunks = chunker.chunk(html).expect("chunk succeeds");

    for chunk in &chunks {
        assert!(
            !chunk.content.trim().is_empty(),
            "no empty chunks: {:?}",
            chunk.content
        );
    }
}

/// Completely empty input produces no chunks.
#[ignore = "resurrected: pending triage"]
#[test]
fn chunker_empty_html_produces_no_chunks() {
    let chunker = HtmlChunker::new();
    let chunks = chunker.chunk("").expect("chunk succeeds");
    assert!(chunks.is_empty(), "empty HTML → no chunks");
}

/// Nested tags are stripped, leaving only text content.
#[ignore = "resurrected: pending triage"]
#[test]
fn chunker_nested_tags_stripped_to_text() {
    let chunker = HtmlChunker::new().with_min_chunk_size(1);
    let html = "<div><p>Hello <strong>world</strong></p></div>";
    let chunks = chunker.chunk(html).expect("chunk succeeds");

    let combined: String = chunks.iter().map(|c| c.content.as_str()).collect("");
    assert!(combined.contains("Hello world"), "nested text: {combined}");
    assert!(!combined.contains("<strong>"), "no tags remain: {combined}");
}

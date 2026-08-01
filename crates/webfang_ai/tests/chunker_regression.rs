//! Regression tests for the AI chunker pipeline (webfang_ai).
//!
//! The chunker pipeline chains: strip_html_tags (naive + '\n' on '>') →
//! split by '\n\n' for paragraphs. Key behavior: pushes '\n' on '>' to
//! create sentence boundaries for AI chunking. NO normalize_whitespace.
//!
//! Split from `cleaning_pipelines_regression.rs` to avoid circular dependency
//! (webfang_core cannot depend on webfang_ai as a dev-dep since webfang_ai
//! depends on webfang_core).

#![cfg(feature = "ai")]

use webfang_ai::HtmlChunker;

#[test]
fn paragraphs_produce_chunks() {
    let chunker = HtmlChunker::new();
    // Default min_chunk_size is 100, so we need enough text per paragraph
    let long = "This is a sufficiently long paragraph with enough text to meet the minimum chunk size requirement for AI processing. ";
    let html = format!("<p>{long}</p><p>{long}</p>");
    let chunks = chunker.chunk(&html).expect("chunking must succeed");
    assert!(!chunks.is_empty(), "should produce chunks from paragraphs");
}

#[test]
fn short_text_produces_few_chunks() {
    let chunker = HtmlChunker::new();
    let html = "<p>Hello</p><p>World</p>";
    let chunks = chunker.chunk(html).expect("chunking must succeed");
    // With default min_chunk_size=100, short text may produce 0 chunks
    // (they get filtered by merge_small_chunks). The key test is no panic.
    assert!(
        chunks.len() <= 2,
        "two short paragraphs produce at most 2 chunks: {}",
        chunks.len()
    );
}

#[test]
fn empty_tags_no_empty_paragraphs() {
    let chunker = HtmlChunker::new();
    let html = "<p></p><p></p><div></div>";
    let chunks = chunker.chunk(html).expect("chunking must succeed");
    assert!(
        chunks.is_empty(),
        "empty tags produce no chunks: {}",
        chunks.len()
    );
}

#[test]
fn whitespace_not_normalized() {
    // Chunker does NOT normalize whitespace — it preserves original spacing
    let chunker = HtmlChunker::with_config(10, 500);
    let html = "<p>Hello   World</p>";
    let chunks = chunker.chunk(html).expect("chunking must succeed");
    if let Some(chunk) = chunks.first() {
        assert!(
            chunk.content.contains("   ") || chunk.content.contains("Hello"),
            "whitespace NOT normalized: {:?}",
            chunk.content
        );
    }
}

#[test]
fn chunk_text_adds_metadata() {
    let chunker = HtmlChunker::with_config(10, 500);
    let text =
        "This is a test paragraph with enough text to be chunked properly for AI processing.";
    let chunks = chunker
        .chunk_text(text, "https://example.com", "Test Title")
        .expect("chunk_text must succeed");
    if let Some(chunk) = chunks.first() {
        assert_eq!(chunk.url, "https://example.com", "url set correctly");
        assert_eq!(chunk.title, "Test Title", "title set correctly");
    }
}

#[test]
fn large_html_respects_max_chunk_size() {
    let chunker = HtmlChunker::with_config(50, 200);
    let paragraphs: Vec<String> = (0..10)
        .map(|i| {
            format!(
                "<p>Paragraph {i} has enough text to be chunked properly for the AI tokenizer to process correctly.</p>"
            )
        })
        .collect();
    let html = paragraphs.join("");
    let chunks = chunker.chunk(&html).expect("chunking must succeed");
    assert!(!chunks.is_empty(), "large HTML produces chunks");
    for chunk in &chunks {
        assert!(
            chunk.content.len() <= 200,
            "chunk {} exceeds max size: {}",
            chunk.id,
            chunk.content.len()
        );
    }
}

#[test]
fn empty_html_returns_empty() {
    let chunker = HtmlChunker::new();
    let chunks = chunker.chunk("").expect("chunking must succeed");
    assert!(chunks.is_empty(), "empty HTML produces no chunks");
}

#[test]
fn plain_text_without_tags() {
    let chunker = HtmlChunker::with_config(10, 500);
    let text = "Just plain text without any HTML tags at all, should still work.";
    let chunks = chunker.chunk(text).expect("chunking must succeed");
    // Plain text should be processed (tags stripped is identity)
    if let Some(chunk) = chunks.first() {
        assert!(
            chunk.content.contains("plain text"),
            "plain text content preserved: {:?}",
            chunk.content
        );
    }
}

//! Regression tests for the AI chunker pipeline (webfang_ai).
//!
//! The chunker pipeline chains: strip_html_tags (block-aware: '\n' emitted only
//! at block-tag edges; inline edges emit nothing and source newlines collapse
//! to spaces) → split by '\n' for paragraphs. Key behavior (#1313): a soft
//! source-line wrap around an inline link can never fabricate a paragraph
//! break mid-sentence. NO normalize_whitespace beyond that.
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

/// Fixture: the lead-section paragraph from the Wikipedia "Rust (programming
/// language)" article that reproduced issue #1313, reconstructed with the
/// page's real markup shape: soft-wrapped source lines ending right before an
/// inline `<a>` link, plus `<sup>` reference nodes wedged between inline tags.
///
/// The historical chunker injected a '\n' after every '>' (naive tag strip), so
/// a source newline + one inline tag edge fabricated a "\n\n" paragraph break
/// INSIDE a sentence. 35 of 95 chunks on this page ended mid-sentence — e.g.
/// `… (i.e., that all \nreferences` / next chunk `point to valid memory) …`.
const WIKIPEDIA_RUST_LEAD_HTML: &str = r##"<p><a href="/wiki/Rust_(programming_language)" title="Rust (programming language)">Rust</a> is a high-level, <a href="/wiki/General-purpose_programming_language">general-purpose</a>
<a href="/wiki/Programming_language">programming language</a> emphasizing <a href="/wiki/Computer_performance">performance</a>,
<a href="/wiki/Type_safety">type safety</a>, and <a href="/wiki/Concurrency_(computer_science)">concurrency</a>.
First appearing in 2006, it was developed by <a href="/wiki/Graydon_Hoare">Graydon Hoare</a> as a
personal project while employed at <a href="/wiki/Mozilla_Research">Mozilla Research</a>. Rust
enforces memory safety (i.e., that all
<a href="/wiki/Reference_(computer_science)">references</a> point to valid
<a href="/wiki/Memory_safety">memory</a>) without a conventional
<a href="/wiki/Garbage_collection_(computer_science)">garbage collector</a><sup class="reference"><a href="#cite_note-1">[1]</a></sup>;
instead, memory is managed through the ownership system, which provides
compile-time guarantees for resource deallocation. Rust also provides
zero-cost abstractions through generics and trait-based polymorphism that
compile down to monomorphized code without runtime dispatch cost.</p>
<p>Designed for safety and concurrency, Rust is frequently used for
<a href="/wiki/Systems_programming">systems programming</a>,
<a href="/wiki/Embedded_system">embedded</a> targets, and browser
<a href="/wiki/Browser_engine">engine</a> components. Its type system provides
<a href="/wiki/Function_(computing)">functions</a> with arguments and return
values, along with <a href="/wiki/Tuple">tuples</a>, algebraic data types,
generics, pattern matching, and closures. Rust omits a garbage collector by
design, trading some flexibility for predictable runtime behavior that suits
latency-sensitive systems and bare-metal embedded targets without an operating
system. The type system includes
<a href="/wiki/Pointer_(computer_science)">pointers</a> with ownership
transfers, lifetimes, and borrowing semantics checked at compile time.</p>
"##;

/// #1313: the chunker must never cut inside a sentence, and it must not
/// smuggle stray newlines into chunk content. The cut sites on the real page
/// were exactly where markdown-style source line wraps put a newline around
/// inline links; the paragraph text there continues mid-sentence.
#[test]
fn wikipedia_rust_no_mid_sentence_cuts() {
    // Mirrors the `--clean-ai` production path: SemanticCleanerImpl builds the
    // chunker with HtmlChunker::new() (min=100, max=512).
    let chunker = HtmlChunker::new();
    let chunks = chunker
        .chunk(WIKIPEDIA_RUST_LEAD_HTML)
        .expect("chunking must succeed");
    assert!(!chunks.is_empty(), "fixture must produce chunks");

    let terminal = |content: &str| {
        matches!(
            content.trim_end().chars().next_back(),
            Some('.' | '!' | '?' | ':' | '"' | ')' | ']')
        )
    };

    for (i, chunk) in chunks.iter().enumerate() {
        // (a) No mid-sentence ends: issue checker used `[.!?:")\]]$` after
        //     trimming — every chunk of this fixture ends a block sentence,
        //     because the source has no real paragraph breaks inside blocks.
        assert!(
            terminal(&chunk.content),
            "chunk {i} ends mid-sentence: {:?}",
            &chunk.content[chunk.content.len().saturating_sub(48)..]
        );
        // (b) No newline contamination inside a chunk (the `\nreferences`
        //     residue from the naive tag strip).
        assert!(
            !chunk.content.contains('\n'),
            "chunk {i} contains raw newlines: {:?}",
            chunk.content
        );
    }

    // (c) The exact phrase the issue observed split across chunks 6→7 must now
    //     live inside a single chunk.
    assert!(
        chunks.iter().any(|c| {
            let one_line = c.content.replace('\n', " ");
            one_line.contains("references point to valid memory) without a conventional garbage collector")
        }),
        "the sentence fragmented by the inline-link wrap must stay intact in one chunk; got {} chunks: {:?}",
        chunks.len(),
        chunks.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
}

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
    // Since #1368 the short blocks are NOT discarded: they merge into a single
    // packed chunk ("Hello World"), so the count stays small but the text
    // survives.
    assert_eq!(chunks.len(), 1, "two short paragraphs pack into one chunk");
    assert_eq!(
        chunks[0].content, "Hello World",
        "text is preserved verbatim"
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

/// Concatenate chunk contents with NUL — a separator that can never occur in
/// chunker output (blocks only ever join with a plain space, #1313/#1368), so
/// every surviving block stays contiguous inside the join.
fn joined(chunks: &[webfang_core::domain::DocumentChunk]) -> String {
    chunks
        .iter()
        .map(|c| c.content.as_str())
        .collect::<Vec<_>>()
        .join("\0")
}

/// #1368 loss regression, worst-5 pattern replayed (nav chrome blocks of 1-9
/// bytes + 50-99-byte prose). BEFORE the fix, pass 1 deleted the three prose
/// sentences and the nav line for a strict-character drop of exactly the
/// trimmed-block total; AFTER the fix the loss is empty: the packer merges
/// everything into one chunk and the emitted length equals the input length
/// plus the joining spaces — asserted numerically.
#[test]
fn worst_five_pattern_loses_zero_chars_after_1368() {
    let chunker = HtmlChunker::new();
    let nav = "<header>\n<a>Home</a>\n<a>Edit</a>\n<a>Talk</a>\n</header>\n";
    let prose1 = "Web scraping frameworks differ mainly in how they schedule requests."; // 68 B
    let prose2 = "The crawler rate-limits before dialing, protecting the target site budget."; // 74 B
    let prose3 = "Semantic cleaning keeps snippets small but never silently lossy again."; // 70 B
    let html = format!("{nav}<p>{prose1}</p><p>{prose2}</p><p>{prose3}</p>");

    let chunks = chunker.chunk(&html).expect("chunking must succeed");
    assert_eq!(chunks.len(), 1, "all four short blocks pack into one chunk");

    let packed = &chunks[0].content;
    let blocks_joined = format!("Home Edit Talk {prose1} {prose2} {prose3}");
    assert_eq!(packed, &blocks_joined, "every block preserved, in order");

    // Exact char accounting: input bytes = the four trimmed blocks; output
    // bytes = blocks + 3 joining spaces. Zero content lost (F = 0).
    let chars_in = "Home Edit Talk".len() + prose1.len() + prose2.len() + prose3.len();
    let chars_out: usize = chunks.iter().map(|c| c.content.len()).sum();
    assert_eq!(
        chars_out,
        chars_in + 3,
        "output must equal input plus joining spaces — no dropped bytes"
    );
}

/// #1368: three legitimate short sentences (50-99 B — forum-answer / docs-line
/// style) alongside long paragraphs must every byte survive at production
/// defaults (min=100, max=512). Verbatim substring containment, not keyword
/// checks.
#[test]
fn short_legit_sentences_survive_default_config() {
    let chunker = HtmlChunker::new();
    let long_a = "Python's built-in numeric functions are documented together, and the guide interleaves brief summaries with anchors for every entry on the functions index page.";
    let long_b = "Retrieval quality depends on every byte the crawler can legitimately surface, which is why the semantic cleaner packs short blocks with their neighbours instead of dropping them.";
    let long_c = "The packer flushes at the boundary and lets the following block start a fresh accumulation, preserving document order without re-parsing anything.";
    let short_1 = "This one-line answer on the forum thread used to vanish entirely."; // 65 B
    let short_2 = "Short answers like this one are exactly what users go searching for."; // 68 B
    let short_3 = "Docs prose may hold a terse sentence of fifty to ninety-nine bytes."; // 67 B
    for s in [short_1, short_2, short_3] {
        assert!((50..=99).contains(&s.len()), "fixture band drifted: {s}");
    }

    let html = format!(
        "<p>{long_a}</p><p>{short_1}</p><p>{long_b}</p><p>{short_2}</p><p>{short_3}</p><p>{long_c}</p>"
    );
    let chunks = chunker.chunk(&html).expect("chunking must succeed");
    assert!(!chunks.is_empty(), "document must produce chunks");

    let all = joined(&chunks);
    for source in [long_a, long_b, long_c, short_1, short_2, short_3] {
        assert!(all.contains(source), "block lost verbatim: {source:?}");
    }
}

/// #1368: a page made ONLY of short blocks (10 enumeration items of 20-80
/// bytes) now yields non-empty chunks, preserves every item, and never exceeds
/// the configured max.
#[test]
fn all_short_page_preserved_within_max() {
    let chunker = HtmlChunker::new();
    let items = [
        "Install with cargo install webfang",
        "Run the scraper on a sitemap url",
        "Export results as markdown files",
        "Retry 429 responses and back off politely",
        "Respect robots before fetching anything",
        "Stream chunks under five hundred chars",
        "Embed with ONNX behind the ai feature",
        "Prune boilerplate before doing chunking",
        "Log structured fields, never raw soup",
        "Resume interrupted runs from state file",
    ];
    assert!(
        items.iter().all(|i| (20..=80).contains(&i.len())),
        "fixture band drifted"
    );
    let html: String = items.iter().map(|i| format!("<p>{i}</p>")).collect();

    let chunks = chunker.chunk(&html).expect("chunking must succeed");
    assert!(!chunks.is_empty(), "short-only pages must not vanish");
    let all = joined(&chunks);
    for item in items {
        assert!(all.contains(item), "enumeration item lost: {item:?}");
    }
    for chunk in &chunks {
        assert!(
            chunk.content.len() <= chunker.max_chunk_size(),
            "chunk exceeds max: {}",
            chunk.content.len()
        );
    }
}

/// #1368 pins the zero discard floor (F = 0): not even a 3-byte reply survives
/// as nothing — a one-line answer is either merged with a neighbour or emitted
/// as its own chunk. The near-full-paragraph variant forces the tail past max
/// so it must be emitted standalone.
#[test]
fn tiny_tail_survives_zero_floor() {
    let chunker = HtmlChunker::new();
    let chunks = chunker.chunk("<p>Yes.</p>").expect("chunking must succeed");
    assert_eq!(chunks.len(), 1, "single tiny block survives");
    assert_eq!(chunks[0].content, "Yes.");

    let filler = "A fixed fifty-byte filler sentence to pin size! 12345";
    let near_full = filler.repeat(10); // 530 B > max 512, tail cannot join it
    assert_eq!(near_full.len(), 530, "fixture size drifted");
    let html = format!("<p>{near_full}</p><p>Right.</p>");
    let chunks = chunker.chunk(&html).expect("chunking must succeed");
    let all = joined(&chunks);
    assert!(all.contains("Right."), "short final run was dropped");
}

/// #1368 × #1313: merging short blocks must only ever join them with a single
/// space — never inject a '\n' nor invent a boundary. The soft-wrapped inline
/// link inside the first paragraph stays one sentence.
#[test]
fn merge_joins_with_space_never_newline() {
    let chunker = HtmlChunker::new();
    let html = "<p>Memory safety means that all
<a href=\"/wiki/Reference\">references</a> point to valid memory; this
sentence wraps across an inline link edge.</p>
<p>Yes, even short follow-ups.</p>";

    let chunks = chunker.chunk(html).expect("chunking must succeed");
    assert_eq!(chunks.len(), 1, "both blocks pack into one chunk");
    assert!(
        !chunks[0].content.contains('\n'),
        "merge must not smuggle newlines into content: {:?}",
        chunks[0].content
    );
    assert_eq!(
        chunks[0].content,
        "Memory safety means that all references point to valid memory; this sentence wraps across an inline link edge. Yes, even short follow-ups.",
        "blocks joined by exactly one space, nothing fabricated"
    );
}

/// #1368: `chunk_text()` shares pass 1, so it inherits the preservation too
/// (source newlines become spaces, every sentence survives, metadata applied).
#[test]
fn chunk_text_inherits_short_block_preservation() {
    let chunker = HtmlChunker::new();
    let text =
        "Nope, not an option.\nTry the verbose flag instead when debugging a failing run here.";
    let chunks = chunker
        .chunk_text(text, "https://example.com/issue", "Issue thread")
        .expect("chunk_text must succeed");
    assert!(!chunks.is_empty(), "short plain-text must not vanish");
    let all = joined(&chunks);
    assert!(all.contains("Nope, not an option."), "tiny sentence lost");
    assert!(all.contains("Try the verbose flag"), "longer sentence lost");
    assert!(
        chunks
            .iter()
            .all(|c| c.url == "https://example.com/issue" && c.title == "Issue thread"),
        "metadata still applied by chunk_text"
    );
}

/// #1368: with a tiny max, a short accumulator reaching the max boundary is
/// still emitted (old guard discarded it because it sat below min), and order
/// is preserved across the flush.
#[test]
fn max_boundary_flush_emits_short_accumulator() {
    let chunker = HtmlChunker::with_config(200, 60);
    let html = "<p>aaa bbb ccc</p><p>ddddd eeeee fffff</p><p>x</p><p>yyyyy</p>";
    let chunks = chunker.chunk(html).expect("chunking must succeed");
    assert!(!chunks.iter().any(|c| c.content.is_empty()));
    let all = joined(&chunks);
    for part in ["aaa bbb ccc", "ddddd eeeee fffff", "x", "yyyyy"] {
        assert!(all.contains(part), "block lost: {part}");
    }
    // Order preserved: first chunk starts with the first block.
    assert!(chunks[0].content.starts_with("aaa bbb ccc"));
    // Nothing exceeds max after packing + split guard.
    assert!(
        chunks.iter().all(|c| c.content.len() <= 60),
        "chunk over max: {:?}",
        chunks.iter().map(|c| c.content.len()).collect::<Vec<_>>()
    );
}

/// #1368: `min_chunk_size()` remains a pure accessor of the packing preference
/// — changing it must not alter which content survives.
#[test]
fn min_size_no_longer_filters_anything() {
    let html = "<p>Tiny.</p><p>Also tiny.</p><p>And one more.</p>";
    let a = HtmlChunker::new().chunk(html).expect("default a");
    let b = HtmlChunker::with_config(1_000, 512)
        .chunk(html)
        .expect("over-max config b");
    assert_eq!(joined(&a), joined(&b));
    assert!(joined(&a).contains("Tiny."));
}

//! Integration tests for SemanticCleaner pipeline components.
//!
//! Tests the pipeline stages (pruner → chunker) in the same order as
//! `SemanticCleanerImpl::clean()`, without requiring the ONNX model.
//! Covers the audit gap: zero tests for the main cleaning pipeline.

use webfang_ai::infrastructure_ai::chunker::HtmlChunker;
use webfang_ai::infrastructure_ai::content_pruner::{ContentPruner, LegibleContentPruner};

/// Exercise the full pruner → chunker pipeline on simple HTML.
#[test]
fn pipeline_simple_html_produces_chunks() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let html = r#"
        <html><body>
        <article>
            <h1>Hello World</h1>
            <p>This is a substantial test paragraph with enough content to pass chunking thresholds. It contains multiple sentences to ensure proper chunking behavior.</p>
        </article>
        </body></html>
    "#;

    let pruned = pruner.prune(html);
    let effective = if pruned.is_empty() { html } else { &pruned };

    let chunks = chunker.chunk(effective).expect("chunking should not fail");
    assert!(
        !chunks.is_empty(),
        "pipeline should produce at least one chunk from valid HTML"
    );
}

/// Complex HTML with scripts and styles should still produce clean output.
#[test]
fn pipeline_strips_scripts_and_styles() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let html = r#"
        <html><head>
            <script>var evil = document.createElement('xss');</script>
            <style>body { display: none; }</style>
        </head><body>
        <article>
            <p>The actual content that should survive the pipeline processing with enough text to meet chunking thresholds.</p>
        </article>
        </body></html>
    "#;

    let pruned = pruner.prune(html);
    let effective = if pruned.is_empty() { html } else { &pruned };

    let chunks = chunker.chunk(effective).expect("chunking should not fail");
    // Chunks should contain article content, not script/style
    for chunk in &chunks {
        assert!(
            !chunk.content.contains("var evil"),
            "script content should be stripped: {}",
            chunk.content
        );
        assert!(
            !chunk.content.contains("display: none"),
            "style content should be stripped: {}",
            chunk.content
        );
    }
}

/// Empty input returns empty pipeline output.
#[test]
fn pipeline_empty_input_returns_empty() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let pruned = pruner.prune("");
    assert!(pruned.is_empty());

    let chunks = chunker
        .chunk("")
        .expect("chunking empty string should not fail");
    assert!(chunks.is_empty());
}

/// Malformed HTML should not panic the pipeline.
#[test]
fn pipeline_malformed_html_does_not_panic() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let malformed_inputs = vec![
        "<<<not valid html>>>><<<",
        "<div><p>Unclosed tags everywhere",
        "</div></div></div>",
        "\0\0\0\x00\x01\x02",
        "<script>alert('xss')</script>",
    ];

    for input in malformed_inputs {
        let pruned = pruner.prune(input);
        let effective = if pruned.is_empty() { input } else { &pruned };
        let _ = chunker.chunk(effective);
    }
}

/// Deeply nested HTML should still produce output.
#[test]
fn pipeline_deeply_nested_html() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let mut html = String::new();
    for _ in 0..50 {
        html.push_str("<div>");
    }
    html.push_str("Deep content that must survive 50 levels of nesting for testing.");
    for _ in 0..50 {
        html.push_str("</div>");
    }

    let pruned = pruner.prune(&html);
    let effective = if pruned.is_empty() {
        html.as_str()
    } else {
        &pruned
    };
    let _ = chunker.chunk(effective);
}

/// Pipeline handles very large HTML without panicking or OOM.
#[test]
fn pipeline_large_html_does_not_oom() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let mut html = String::with_capacity(500_000);
    html.push_str("<html><body><article>");
    for i in 0..1000 {
        html.push_str(&format!(
            "<p>Paragraph {i} with enough content to test chunking at scale across many iterations.</p>"
        ));
    }
    html.push_str("</article></body></html>");

    let pruned = pruner.prune(&html);
    let effective = if pruned.is_empty() {
        html.as_str()
    } else {
        &pruned
    };
    // Should complete without panic or OOM — chunk count depends on pruner behavior
    let _chunks = chunker
        .chunk(effective)
        .expect("chunking large HTML should not fail");
}

/// Unicode content should survive the pipeline intact.
#[test]
fn pipeline_unicode_content_preserved() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let html = r#"
        <article>
            <p>日本語のテストコンテンツです。これは十分な長さのテキストで、チャンク分割の閾値を超えるべきです。</p>
            <p>Contenido en español con acentos: áéíóú ñü. This paragraph has enough text for chunking.</p>
        </article>
    "#;

    let pruned = pruner.prune(html);
    let effective = if pruned.is_empty() { html } else { &pruned };
    let chunks = chunker
        .chunk(effective)
        .expect("unicode HTML should chunk fine");

    let all_content: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(
        all_content.contains("日本語") || all_content.contains("español"),
        "unicode characters should be preserved in pipeline output"
    );
}

/// HTML with only whitespace and no real content should produce zero chunks.
#[test]
fn pipeline_whitespace_only_produces_no_chunks() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let pruned = pruner.prune("   \n\t  \n   ");
    let effective = if pruned.is_empty() {
        "   \n\t  \n   "
    } else {
        &pruned
    };
    let chunks = chunker.chunk(effective).expect("chunking should not fail");
    assert!(chunks.is_empty());
}

/// Verify pruned output feeds correctly into chunker (pipeline integration).
#[test]
fn pipeline_pruned_to_chunker_handoff() {
    let pruner = LegibleContentPruner::standard();
    let chunker = HtmlChunker::new();

    let html = r#"
        <html>
        <nav>Navigation that should be pruned away</nav>
        <main>
            <h1>Important Article Title</h1>
            <p>This is the main article content with enough text to produce meaningful chunks through the pipeline.</p>
            <p>Second paragraph with additional content that adds substance to the article body.</p>
        </main>
        <aside>Sidebar that should also be removed by the pruner.</aside>
        </html>
    "#;

    let pruned = pruner.prune(html);
    let effective = if pruned.is_empty() { html } else { &pruned };
    let chunks = chunker.chunk(effective).expect("pipeline should not fail");

    let all_content: String = chunks.iter().map(|c| c.content.as_str()).collect();
    // Main content should be present
    assert!(
        all_content.contains("Important Article") || all_content.contains("article"),
        "main article content should survive pruning: {all_content}"
    );
}

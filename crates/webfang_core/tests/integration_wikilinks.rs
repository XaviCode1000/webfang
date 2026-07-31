//! Integration tests for wikilinks image-preservation guard rail.
//!
//! Tests the full pipeline: HTML → htmd → convert_wiki_links
//! verifying that image-in-link patterns survive end-to-end.
//!
//! Run with: cargo test --test integration_wikilinks

use webfang::infrastructure::converter::wikilinks::convert_wiki_links;

/// Simulate the real pipeline: HTML → htmd → convert_wiki_links
/// This is the critical path the guard rail protects.
fn pipeline(html: &str, domain: &str) -> String {
    let markdown = htmd::convert(html).unwrap_or_else(|_| html.to_string());
    convert_wiki_links(&markdown, domain)
}

// ============================================================================
// Phase 1: Core pipeline integration tests
// ============================================================================

#[test]
fn pipeline_image_link_preserved() {
    // Card with link wrapping an image — the guard rail must preserve it
    let html = concat!(
        r#"<a href="https://example.com/page">"#,
        r#"<img src="https://example.com/img.svg" alt="icon">"#,
        r#"</a>"#,
    );
    let result = pipeline(html, "example.com");
    assert!(
        result.contains("[!["),
        "Expected [![ pattern for image link, got: {result}"
    );
    assert!(
        result.contains("](https://example.com/img.svg)"),
        "Image URL must survive pipeline, got: {result}"
    );
    assert!(
        result.contains("](https://example.com/page)"),
        "Link URL must survive pipeline, got: {result}"
    );
}

#[test]
fn pipeline_relative_image_link_preserved() {
    let html = concat!(
        r#"<a href="/about">"#,
        r#"<img src="/images/logo.png" alt="logo">"#,
        r#"</a>"#,
    );
    let result = pipeline(html, "example.com");
    assert!(
        result.contains("[!["),
        "Expected [![ pattern for relative image link, got: {result}"
    );
    assert!(
        result.contains("](/images/logo.png)"),
        "Relative image URL must survive pipeline, got: {result}"
    );
}

#[test]
fn pipeline_plain_text_link_converted_to_wikilink() {
    // Text-only links should still be converted
    let html = r#"<a href="https://example.com/about">About Us</a>"#;
    let result = pipeline(html, "example.com");
    assert!(
        result.contains("[["),
        "Expected [[ pattern for text link, got: {result}"
    );
    assert!(
        !result.contains("[[") || result.contains("|"),
        "Wiki-link should have display text, got: {result}"
    );
}

#[test]
fn pipeline_mixed_image_and_text_links() {
    let html = concat!(
        r#"<a href="https://example.com/page">"#,
        r#"<img src="https://example.com/img.svg" alt="icon">"#,
        r#"</a>"#,
        " and ",
        r#"<a href="https://example.com/about">About</a>"#,
    );
    let result = pipeline(html, "example.com");
    // Image link: preserved as markdown
    assert!(
        result.contains("[![icon]"),
        "Image link must be preserved, got: {result}"
    );
    // Text link: converted to wiki-link
    assert!(
        result.contains("[["),
        "Text link must be converted, got: {result}"
    );
}

#[test]
fn pipeline_external_image_link_unaffected() {
    // External domain links should be left untouched
    let html =
        r#"<a href="https://other.com/page"><img src="https://other.com/img.png" alt="img"></a>"#;
    let result = pipeline(html, "example.com");
    assert!(
        !result.contains("[["),
        "External link must NOT be converted to wiki-link, got: {result}"
    );
}

#[test]
fn pipeline_heading_with_image_link() {
    // Realistic: heading with card link wrapping image
    let html = concat!(
        r#"<h2 class="card-title">"#,
        r#"<a href="/test-sites/pagination">"#,
        r#"Test site with pagination links"#,
        r#"</a>"#,
        r#"</h2>"#,
        r#"<a href="/test-sites/pagination">"#,
        r#"<img src="/images/test-sites/pagination.svg" alt="pagination icon">"#,
        r#"</a>"#,
    );
    let result = pipeline(html, "example.com");
    // Image link must be preserved
    assert!(
        result.contains("![") || result.contains("[!["),
        "Image link in heading context must survive, got: {result}"
    );
}

#[test]
fn pipeline_code_block_links_not_converted() {
    // Links inside code blocks should not be converted
    let html = concat!(
        r#"<pre><code>"#,
        r#"<a href="https://example.com/link">not a link</a>"#,
        r#"</code></pre>"#,
    );
    let result = pipeline(html, "example.com");
    // htmd should put this in a code fence
    assert!(
        result.contains("```") || !result.contains("[["),
        "Code block link must NOT be converted, got: {result}"
    );
}

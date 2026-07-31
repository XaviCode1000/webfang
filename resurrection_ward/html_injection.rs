//! Security fuzzing tests for HTML injection / sanitization
//!
//! Verifies that the HTML cleaner strips dangerous elements and attributes:
//! - Script tags and their content
//! - Event handler attributes (onclick, onerror, etc.)
//! - iframe/object/embed elements
//! - SVG elements (XSS vector)
//! - CSS injection via style tags
//! - Attribute stripping (only href, src, alt, id, class, dir, code preserved)

use webfang::infrastructure::converter::html_cleaner::clean_html;
use webfang::infrastructure::converter::html_to_markdown::convert_to_markdown;

// ============================================================================
// Script tag injection
// ============================================================================

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_script_tag_simple() {
    let html = r#"<p>Hello</p><script>alert(1)</script><p>World</p>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("<script>"),
        "script tag should be removed: {cleaned}"
    );
    assert!(
        !cleaned.contains("alert"),
        "script content should be removed: {cleaned}"
    );
    assert!(cleaned.contains("Hello"));
    assert!(cleaned.contains("World"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_script_tag_with_event_handler() {
    let html = r#"<script onerror="alert(1)">var x=1;</script>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("<script>"));
    assert!(!cleaned.contains("alert"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_script_tag_with_src() {
    let html = r#"<script src="https://evil.com/steal.js"></script>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("evil.com"),
        "external script src should be removed: {cleaned}"
    );
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_multiple_script_tags() {
    let html = r#"
        <p>Before</p>
        <script>alert(1)</script>
        <script>document.cookie</script>
        <script src="evil.js"></script>
        <p>After</p>
    "#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("alert"));
    assert!(!cleaned.contains("document.cookie"));
    assert!(!cleaned.contains("evil.js"));
    assert!(cleaned.contains("Before"));
    assert!(cleaned.contains("After"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_script_with_obfuscated_content() {
    let html = r#"<p>Text</p><script>eval('al'+'ert(1)')</script>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("eval"));
    assert!(cleaned.contains("Text"));
}

// ============================================================================
// Event handler attribute injection
// ============================================================================

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_onclick_attribute() {
    let html = r#"<a href="https://example.com" onclick="alert(1)">Click</a>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("onclick"),
        "onclick should be stripped: {cleaned}"
    );
    assert!(cleaned.contains("Click"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_onerror_attribute() {
    let html = r#"<img src="x.png" onerror="alert(1)"/>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("onerror"),
        "onerror should be stripped: {cleaned}"
    );
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_onload_attribute() {
    let html = r#"<body onload="alert(1)">Content</body>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("onload"),
        "onload should be stripped: {cleaned}"
    );
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_onmouseover_attribute() {
    let html = r#"<div onmouseover="alert(1)">Hover me</div>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("onmouseover"),
        "onmouseover should be stripped: {cleaned}"
    );
    assert!(cleaned.contains("Hover me"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_all_event_handlers() {
    let html = r#"<input onfocus="a()" onblur="b()" onchange="c()" oninput="d()" onsubmit="e()"/>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("onfocus"));
    assert!(!cleaned.contains("onblur"));
    assert!(!cleaned.contains("onchange"));
    assert!(!cleaned.contains("oninput"));
    assert!(!cleaned.contains("onsubmit"));
}

// ============================================================================
// Dangerous element removal
// ============================================================================

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_iframe() {
    let html = r#"<p>Before</p><iframe src="https://evil.com/phish"></iframe><p>After</p>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("iframe"),
        "iframe should be removed: {cleaned}"
    );
    assert!(cleaned.contains("Before"));
    assert!(cleaned.contains("After"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_object() {
    let html = r#"<object data="evil.swf" type="application/x-shockwave-flash"></object>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("<object>"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_embed() {
    let html = r#"<embed src="evil.swf" type="application/x-shockwave-flash"/>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("<embed"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_svg() {
    let html = r#"<p>Text</p><svg onload="alert(1)"><circle r="50"/></svg>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("<svg"),
        "SVG should be removed: {cleaned}"
    );
    assert!(cleaned.contains("Text"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_noscript() {
    let html = r#"<p>Visible</p><noscript>Hidden content</noscript>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("Hidden content"));
    assert!(cleaned.contains("Visible"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_form() {
    let html =
        r#"<form action="https://evil.com/steal"><input name="password" type="password"/></form>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("<form"));
    assert!(!cleaned.contains("password"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_video_audio() {
    let html = r#"<video src="evil.mp4"></video><audio src="evil.mp3"></audio>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("<video"));
    assert!(!cleaned.contains("<audio"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_canvas() {
    let html = r#"<canvas id="c"></canvas><script>document.getElementById('c')</script>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("<canvas"));
}

// ============================================================================
// CSS injection via style tag
// ============================================================================

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_style_tag() {
    let html = r#"<style>body{background:url('javascript:alert(1)')}</style><p>Content</p>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("<style>"),
        "style tag should be removed: {cleaned}"
    );
    assert!(!cleaned.contains("javascript:"));
    assert!(cleaned.contains("Content"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_inline_style_attribute() {
    let html = r#"<p style="background:url('javascript:alert(1)')">Content</p>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("style="),
        "style attribute should be stripped: {cleaned}"
    );
    assert!(cleaned.contains("Content"));
}

// ============================================================================
// Attribute preservation and stripping
// ============================================================================

#[ignore = "resurrected: pending triage"]
#[test]
fn preserve_href_attribute() {
    let html = r#"<a href="https://example.com" class="link">Click</a>"#;
    let cleaned = clean_html(html);
    assert!(cleaned.contains("href="), "href should be preserved");
    assert!(cleaned.contains("https://example.com"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn preserve_src_attribute() {
    let html = r#"<img src="image.png" alt="Photo"/>"#;
    let cleaned = clean_html(html);
    assert!(cleaned.contains("src="), "src should be preserved");
    assert!(cleaned.contains("image.png"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn preserve_alt_attribute() {
    let html = r#"<img src="x.png" alt="Description"/>"#;
    let cleaned = clean_html(html);
    assert!(cleaned.contains("alt="), "alt should be preserved");
    assert!(cleaned.contains("Description"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_title_attribute() {
    let html = r#"<a href="https://example.com" title="Hover text">Link</a>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("title="),
        "title should be stripped: {cleaned}"
    );
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_style_attribute() {
    let html = r#"<p style="color:red">Text</p>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("style="), "style should be stripped");
}

#[ignore = "resurrected: pending triage"]
#[test]
fn strip_data_attributes() {
    let html = r#"<div data-track="click" data-user="123">Content</div>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("data-track"));
    assert!(!cleaned.contains("data-user"));
    assert!(cleaned.contains("Content"));
}

// ============================================================================
// HTML entity bypass attempts
// ============================================================================

#[ignore = "resurrected: pending triage"]
#[test]
fn entity_encoded_script_tag() {
    let html = r#"<p>Safe</p><scr&#x69;pt>alert(1)</scr&#x69;pt>"#;
    let cleaned = clean_html(html);
    // Entity-encoded tag names are NOT decoded by the HTML parser inside tags,
    // so <scr&#x69;pt> is treated as a <scr> element, not <script>.
    // The key security property: no literal <script> tag appears in output.
    assert!(
        !cleaned.contains("<script>"),
        "no actual script tag in output: {cleaned}"
    );
    assert!(cleaned.contains("Safe"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn nested_html_entities() {
    let html = r#"<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>"#;
    let cleaned = clean_html(html);
    assert!(
        !cleaned.contains("<script>"),
        "script tag must be stripped: {cleaned}"
    );
    assert!(
        cleaned.contains("Safe") || cleaned.contains("alert(1)"),
        "safe content or escaped entity must remain: {cleaned}"
    );
}

// ============================================================================
// HTML to Markdown conversion — verify sanitization propagates
// ============================================================================

#[ignore = "resurrected: pending triage"]
#[test]
fn markdown_conversion_strips_scripts() {
    let html = r#"<h1>Title</h1><script>alert(1)</script><p>Content</p>"#;
    let md = convert_to_markdown(html);
    assert!(
        !md.contains("alert"),
        "scripts should not appear in markdown output: {md}"
    );
    assert!(md.contains("Title"));
    assert!(md.contains("Content"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn markdown_conversion_strips_iframes() {
    let html = r#"<p>Before</p><iframe src="evil.com"></iframe><p>After</p>"#;
    let md = convert_to_markdown(html);
    assert!(!md.contains("evil.com"));
    assert!(md.contains("Before"));
    assert!(md.contains("After"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn markdown_conversion_strips_event_handlers() {
    let html = r#"<a href="https://example.com" onclick="steal()">Link</a>"#;
    let md = convert_to_markdown(html);
    assert!(!md.contains("onclick"));
    assert!(!md.contains("steal"));
}

// ============================================================================
// Edge cases and adversarial HTML
// ============================================================================

#[ignore = "resurrected: pending triage"]
#[test]
fn malformed_html_no_panic() {
    let adversarial = vec![
        "<".repeat(1000),
        ">".repeat(1000),
        "<<script>>alert(1)</script>>".to_string(),
        "<<img src=x onerror=alert(1)>>".to_string(),
        "\x00\x01\x02\x03<script>alert(1)</script>".to_string(),
        "<div>".repeat(500),
        "</div>".repeat(500),
    ];
    for html in &adversarial {
        let cleaned = clean_html(html);
        assert!(
            !cleaned.contains("<script>"),
            "adversarial input bypassed cleaner: {html}"
        );
        let _md = convert_to_markdown(html);
    }
}

#[ignore = "resurrected: pending triage"]
#[test]
fn deeply_nested_tags() {
    let mut html = String::new();
    for _ in 0..100 {
        html.push_str("<div>");
    }
    html.push_str("Content");
    for _ in 0..100 {
        html.push_str("</div>");
    }
    let cleaned = clean_html(&html);
    assert!(cleaned.contains("Content"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn script_inside_comment() {
    let html = r#"<!-- <script>alert(1)</script> --><p>Safe</p>"#;
    let cleaned = clean_html(html);
    assert!(cleaned.contains("Safe"));
}

#[ignore = "resurrected: pending triage"]
#[test]
fn noscript_with_fallback_content() {
    let html = r#"<noscript><style>body{background:red}</style></noscript><p>Content</p>"#;
    let cleaned = clean_html(html);
    assert!(!cleaned.contains("noscript"));
    assert!(!cleaned.contains("background:red"));
    assert!(cleaned.contains("Content"));
}

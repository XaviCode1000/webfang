//! Cleaning pipeline stage.
//!
//! Extracts readable text from raw HTML using the `legible` crate (Mozilla
//! Readability port), strips tags, normalizes whitespace, and populates
//! [`ScrapedItem::text_content`] and metadata.

use std::future::Future;
use std::pin::Pin;

use crate::domain::pipeline_item::{PipelineStage, ScrapedItem, StageOutcome};

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    CLEAN_REDUCTION_PCT, CLEAN_SPA_DETECTED,
};

/// Minimum cleaned text length (in characters) to be considered real content.
/// Items shorter than this are tagged as potential SPA (single-page app)
/// shells that rendered no meaningful content.
const MIN_TEXT_LENGTH: usize = 50;

/// Metadata keys written by [`CleanStage`].
pub const META_ORIGINAL_SIZE: &str = "original_size";
pub const META_CLEANED_SIZE: &str = "cleaned_size";
pub const META_REDUCTION_PCT: &str = "reduction_pct";
pub const META_POTENTIAL_SPA: &str = "potential_spa";

/// Pipeline stage that cleans raw HTML into readable text.
///
/// Uses the `legible` crate (Mozilla Readability port) to extract the main
/// content, then strips any residual HTML tags and normalizes whitespace.
///
/// # Metadata produced
///
/// | Key | Value |
/// |-----|-------|
/// | `original_size` | Byte length of raw_html |
/// | `cleaned_size` | Byte length of extracted text |
/// | `reduction_pct` | Percentage reduction (0–100) |
/// | `potential_spa` | `"true"` if cleaned text < 50 chars |
pub struct CleanStage;

impl PipelineStage for CleanStage {
    fn name(&self) -> &str {
        "clean"
    }

    fn process(
        &self,
        item: ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = StageOutcome> + Send + '_>> {
        Box::pin(async move { clean(item) })
    }
}

fn clean(mut item: ScrapedItem) -> StageOutcome {
    let original_size = item.raw_html.len();

    // 1. Extract readable content via legible (Readability port)
    let extracted = extract_readability(&item.raw_html);

    // 2. Strip residual HTML tags
    let no_tags = strip_html_tags(&extracted);

    // 3. Normalize whitespace
    let text = normalize_whitespace(&no_tags);

    let cleaned_size = text.len();
    let reduction_pct = if original_size > 0 {
        ((original_size as f64 - cleaned_size as f64) / original_size as f64 * 100.0) as u64
    } else {
        0
    };

    // 4. Populate text_content
    item.text_content = Some(text);

    // 5. Write metadata
    item.metadata
        .insert(META_ORIGINAL_SIZE.into(), original_size.to_string());
    item.metadata
        .insert(META_CLEANED_SIZE.into(), cleaned_size.to_string());
    item.metadata
        .insert(META_REDUCTION_PCT.into(), reduction_pct.to_string());

    #[cfg(feature = "otel-metrics")]
    CLEAN_REDUCTION_PCT.record(reduction_pct as f64, &[]);

    // 6. Tag potential SPA if cleaned content is too short
    if cleaned_size < MIN_TEXT_LENGTH {
        item.metadata
            .insert(META_POTENTIAL_SPA.into(), "true".into());
        #[cfg(feature = "otel-metrics")]
        CLEAN_SPA_DETECTED.add(1, &[]);
    }

    StageOutcome::Continue(item)
}

/// Extract main content using legible (Mozilla Readability).
/// Falls back to the raw HTML if extraction fails or returns empty.
fn extract_readability(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }

    match legible::parse(html, None, None) {
        Ok(article) => {
            if article.content.trim().is_empty() {
                html.to_string()
            } else {
                article.content
            }
        },
        Err(_) => html.to_string(),
    }
}

/// Strip HTML tags, returning only text content.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut inside_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            c if !inside_tag => result.push(c),
            _ => {},
        }
    }
    result
}

/// Collapse runs of whitespace (spaces, tabs, newlines) into a single space
/// and trim leading/trailing whitespace.
fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(url: &str, html: &str) -> ScrapedItem {
        ScrapedItem {
            url: url.into(),
            raw_html: html.into(),
            status_code: 200,
            ..Default::default()
        }
    }

    // legible (dom_query/servo_arc) uses atomic reference-counted nodes whose
    // aliasing is incompatible with Miri's Tree Borrows model. This is a known
    // third-party limitation (see infrastructure/bridge.rs for the same pattern),
    // not a bug in our cleaning logic, so skip these under Miri.
    #[cfg_attr(miri, ignore)] // legible/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_html_stripped_and_text_populated() {
        let html = r#"<html><body><p>Hello world</p></body></html>"#;
        let item = make_item("https://example.com", html);
        let result = CleanStage.process(item).await;

        match result {
            StageOutcome::Continue(item) => {
                let text = item.text_content.as_ref().expect("text_content set");
                assert!(text.contains("Hello world"));
                assert!(!text.contains("<p>"));
                assert!(!text.contains("</p>"));
            },
            _ => panic!("expected Continue"),
        }
    }

    #[cfg_attr(miri, ignore)] // legible/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_whitespace_normalized() {
        let html = "<p>  hello   world  \n\n  foo  </p>";
        let item = make_item("https://example.com", html);
        let result = CleanStage.process(item).await;

        match result {
            StageOutcome::Continue(item) => {
                let text = item.text_content.as_ref().unwrap();
                assert_eq!(text, "hello world foo");
            },
            _ => panic!("expected Continue"),
        }
    }

    #[cfg_attr(miri, ignore)] // legible/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_metadata_updated() {
        let html = "<p>Some content here with enough text to pass the minimum length check for clean stage</p>";
        let item = make_item("https://example.com", html);
        let result = CleanStage.process(item).await;

        match result {
            StageOutcome::Continue(item) => {
                assert!(item.metadata.contains_key(META_ORIGINAL_SIZE));
                assert!(item.metadata.contains_key(META_CLEANED_SIZE));
                assert!(item.metadata.contains_key(META_REDUCTION_PCT));
                let orig: usize = item.metadata[META_ORIGINAL_SIZE].parse().unwrap();
                let cleaned: usize = item.metadata[META_CLEANED_SIZE].parse().unwrap();
                assert!(orig > 0);
                assert!(cleaned > 0);
                assert!(cleaned <= orig);
            },
            _ => panic!("expected Continue"),
        }
    }

    #[cfg_attr(miri, ignore)] // legible/servo_arc aliasing incompatible with Tree Borrows
    #[tokio::test]
    async fn test_short_content_tags_spa() {
        // Content that legible will likely return as-is, with total cleaned < 50 chars
        let html = "<p>hi</p>";
        let item = make_item("https://example.com", html);
        let result = CleanStage.process(item).await;

        match result {
            StageOutcome::Continue(item) => {
                let text = item.text_content.as_ref().unwrap();
                // After cleaning, "hi" is < 50 chars
                if text.len() < MIN_TEXT_LENGTH {
                    assert_eq!(
                        item.metadata.get(META_POTENTIAL_SPA).map(String::as_str),
                        Some("true")
                    );
                }
            },
            _ => panic!("expected Continue"),
        }
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<p>hello</p>"), "hello");
        assert_eq!(strip_html_tags("<a href='x'>link</a>"), "link");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(strip_html_tags("<br><hr>"), "");
    }

    #[test]
    fn test_normalize_whitespace() {
        assert_eq!(normalize_whitespace("  hello  world  "), "hello world");
        assert_eq!(normalize_whitespace("a\n\nb\t\nc"), "a b c");
        assert_eq!(normalize_whitespace("single"), "single");
    }

    #[test]
    fn test_extract_readability_empty() {
        assert_eq!(extract_readability(""), "");
        assert_eq!(extract_readability("   \n  "), "");
    }

    #[test]
    fn test_stage_name() {
        assert_eq!(CleanStage.name(), "clean");
    }
}

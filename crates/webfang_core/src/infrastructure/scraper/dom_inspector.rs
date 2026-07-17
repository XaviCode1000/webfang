//! DOM inspector implementations — infrastructure layer.
//!
//! Provides two implementations of [`DomInspectorPort`]:
//! - [`NoOpInspector`] — Null Object pattern, zero overhead on happy path.
//! - [`DefaultDomInspector`] — walks the DOM tree and computes Jaro-Winkler
//!   similarity scores for selector suggestions.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::domain::{DomInspectorPort, DomStructureReport, SelectorSuggestion};

// ---------------------------------------------------------------------------
// NoOpInspector
// ---------------------------------------------------------------------------

/// No-op DOM inspector — returns empty reports with zero overhead.
///
/// Null Object pattern: used when diagnostics are not needed (non-MCP paths
/// or when an inspector is not configured in `McpState`).
#[derive(Debug, Clone, Default)]
pub struct NoOpInspector;

impl DomInspectorPort for NoOpInspector {
    fn inspect(&self, _document: &scraper::Html) -> DomStructureReport {
        DomStructureReport::default()
    }

    fn suggest(
        &self,
        _document: &scraper::Html,
        _failed_selector: &str,
    ) -> Vec<SelectorSuggestion> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// DefaultDomInspector
// ---------------------------------------------------------------------------

/// Default DOM inspector — walks the DOM tree and computes similarity scores.
///
/// Uses `scraper` for DOM traversal and `strsim::jaro_winkler` for similarity.
/// Node-count cap prevents hangs on very large DOMs (default: 10,000).
pub struct DefaultDomInspector {
    node_count_cap: usize,
    similarity_threshold: f64,
}

impl DefaultDomInspector {
    /// Default node-count cap (10,000 elements).
    pub const DEFAULT_NODE_CAP: usize = 10_000;

    /// Default Jaro-Winkler similarity threshold (0.6).
    pub const DEFAULT_THRESHOLD: f64 = 0.6;

    /// Create a new `DefaultDomInspector` with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            node_count_cap: Self::DEFAULT_NODE_CAP,
            similarity_threshold: Self::DEFAULT_THRESHOLD,
        }
    }

    /// Set the maximum number of elements to inspect before truncating.
    #[must_use]
    pub fn with_node_cap(mut self, cap: usize) -> Self {
        self.node_count_cap = cap;
        self
    }

    /// Set the minimum Jaro-Winkler similarity score for suggestions.
    #[must_use]
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.similarity_threshold = threshold;
        self
    }

    /// Collect candidate selectors from the DOM.
    ///
    /// Returns a deduplicated `Vec<String>` of bare tag names, `.class`
    /// selectors, and `#id` selectors.
    fn collect_candidates(document: &scraper::Html) -> Vec<String> {
        let selector = match scraper::Selector::parse("*") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut candidates: Vec<String> = Vec::new();

        for element_ref in document.select(&selector) {
            let el = element_ref.value();

            // Tag name candidate
            let tag = el.name().to_owned();
            if seen.insert(tag.clone()) {
                candidates.push(tag);
            }

            // Class selector candidates
            for class in el.classes() {
                let class_sel = format!(".{class}");
                if seen.insert(class_sel.clone()) {
                    candidates.push(class_sel);
                }
            }

            // ID selector candidate
            if let Some(id) = el.id() {
                let id_sel = format!("#{id}");
                if seen.insert(id_sel.clone()) {
                    candidates.push(id_sel);
                }
            }
        }

        candidates
    }
}

impl Default for DefaultDomInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl DomInspectorPort for DefaultDomInspector {
    fn inspect(&self, document: &scraper::Html) -> DomStructureReport {
        let selector = match scraper::Selector::parse("*") {
            Ok(s) => s,
            Err(_) => return DomStructureReport::default(),
        };

        let mut element_count: usize = 0;
        let mut truncated = false;
        let mut max_depth: usize = 0;
        let mut tag_counts: HashMap<String, usize> = HashMap::new();
        let mut class_counts: HashMap<String, usize> = HashMap::new();
        let mut id_counts: HashMap<String, usize> = HashMap::new();

        for element_ref in document.select(&selector) {
            if element_count >= self.node_count_cap {
                truncated = true;
                break;
            }

            let el = element_ref.value();

            element_count += 1;

            // Depth = number of ancestors (including the Document root node).
            let depth = element_ref.ancestors().count();
            if depth > max_depth {
                max_depth = depth;
            }

            // Tag count
            *tag_counts.entry(el.name().to_owned()).or_insert(0) += 1;

            // Class counts
            for class in el.classes() {
                *class_counts.entry(class.to_string()).or_insert(0) += 1;
            }

            // ID count
            if let Some(id) = el.id() {
                *id_counts.entry(id.to_owned()).or_insert(0) += 1;
            }
        }

        // Sort common_classes descending by frequency, then alphabetically for
        // deterministic output when frequencies are tied. Top 10.
        let mut common_classes: Vec<(String, usize)> = class_counts.into_iter().collect();
        common_classes.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        common_classes.truncate(10);

        // Sort common_ids descending by frequency, then alphabetically. Top 5.
        let mut common_ids: Vec<(String, usize)> = id_counts.into_iter().collect();
        common_ids.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        common_ids.truncate(5);

        DomStructureReport {
            element_count,
            truncated,
            tag_counts,
            max_depth,
            common_classes,
            common_ids,
        }
    }

    fn suggest(&self, document: &scraper::Html, failed_selector: &str) -> Vec<SelectorSuggestion> {
        let candidates = Self::collect_candidates(document);

        let mut suggestions: Vec<SelectorSuggestion> = candidates
            .into_iter()
            .map(|candidate| {
                let score = strsim::jaro_winkler(failed_selector, &candidate);
                SelectorSuggestion {
                    selector: candidate,
                    score,
                }
            })
            .filter(|s| s.score >= self.similarity_threshold)
            .collect();

        // Sort by score descending. Use unwrap_or(Equal) for NaN safety.
        suggestions
            .sort_unstable_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        // Truncate to top 5.
        suggestions.truncate(5);

        suggestions
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- NoOpInspector tests ---

    #[test]
    fn test_noop_inspector_inspect_returns_default() {
        let inspector = NoOpInspector;
        let document = scraper::Html::parse_document("<html><body><p>hello</p></body></html>");
        let report = inspector.inspect(&document);
        assert_eq!(report.element_count, 0);
        assert!(!report.truncated);
        assert!(report.tag_counts.is_empty());
        assert_eq!(report.max_depth, 0);
        assert!(report.common_classes.is_empty());
        assert!(report.common_ids.is_empty());
    }

    #[test]
    fn test_noop_inspector_suggest_returns_empty() {
        let inspector = NoOpInspector;
        let document = scraper::Html::parse_document("<html><body><p>hello</p></body></html>");
        let suggestions = inspector.suggest(&document, ".nonexistent");
        assert!(suggestions.is_empty());
    }

    // --- DefaultDomInspector::inspect tests ---

    #[test]
    fn test_default_inspector_counts_tags_correctly() {
        let html = r#"<html>
  <head><title>Test</title></head>
  <body>
    <div class="container main">
      <h1 id="header">Title</h1>
      <p class="text">Paragraph</p>
      <span>Span</span>
    </div>
  </body>
</html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let report = inspector.inspect(&document);

        // 8 elements: html, head, title, body, div, h1, p, span
        assert_eq!(report.element_count, 8);
        assert!(!report.truncated);

        // Tag counts
        assert_eq!(*report.tag_counts.get("html").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("head").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("title").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("body").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("div").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("h1").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("p").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("span").unwrap_or(&0), 1);

        // Classes: container(1), main(1), text(1)
        assert!(!report.common_classes.is_empty());
        let class_names: Vec<&str> = report
            .common_classes
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(class_names.contains(&"container"));
        assert!(class_names.contains(&"main"));
        assert!(class_names.contains(&"text"));

        // IDs: header(1)
        assert!(!report.common_ids.is_empty());
        assert_eq!(report.common_ids[0].0, "header");
        assert_eq!(report.common_ids[0].1, 1);

        // Max depth: html(1), head(2), title(3), body(2), div(3), h1(4), p(4), span(4)
        assert_eq!(report.max_depth, 4);
    }

    #[test]
    fn test_default_inspector_respects_node_cap() {
        // Generate HTML with 20 elements, cap at 10.
        let html = "<html><body>".to_owned()
            + &"<div>".repeat(20)
            + &"</div>".repeat(20)
            + "</body></html>";
        let document = scraper::Html::parse_document(&html);
        let inspector = DefaultDomInspector::new().with_node_cap(10);
        let report = inspector.inspect(&document);

        assert!(report.truncated);
        assert_eq!(report.element_count, 10);
    }

    #[test]
    fn test_default_inspector_empty_html_returns_minimal_report() {
        // The HTML5 parser creates implicit html/head/body even for empty input.
        let document = scraper::Html::parse_document("");
        let inspector = DefaultDomInspector::new();
        let report = inspector.inspect(&document);

        // Parser creates 3 implicit elements: html, head, body.
        assert_eq!(report.element_count, 3);
        assert!(!report.truncated);
        assert_eq!(report.tag_counts.len(), 3);
        assert!(report.tag_counts.contains_key("html"));
        assert!(report.tag_counts.contains_key("head"));
        assert!(report.tag_counts.contains_key("body"));
        assert!(report.common_classes.is_empty());
        assert!(report.common_ids.is_empty());
    }

    #[test]
    fn test_default_inspector_max_depth_calculation() {
        let inspector = DefaultDomInspector::new();

        // Shallow tree: <div><p>text</p></div>
        let shallow_doc = scraper::Html::parse_fragment("<div><p>text</p></div>");
        let shallow_report = inspector.inspect(&shallow_doc);

        // Deep tree: <div><div><div><div><p>deep</p></div></div></div></div>
        let deep_doc = scraper::Html::parse_fragment(
            "<div><div><div><div><p>deep</p></div></div></div></div>",
        );
        let deep_report = inspector.inspect(&deep_doc);

        // Deeper nesting must produce greater max_depth.
        assert!(
            deep_report.max_depth > shallow_report.max_depth,
            "deep tree max_depth ({}) should exceed shallow tree max_depth ({})",
            deep_report.max_depth,
            shallow_report.max_depth
        );
        // Both should have non-zero depth (at least 1 ancestor: the root node).
        assert!(shallow_report.max_depth >= 1);
        assert!(deep_report.max_depth >= 1);
    }

    #[test]
    fn test_default_inspector_class_frequency_sorted_descending() {
        let html = r#"<html><body>
          <div class="alpha">1</div>
          <div class="alpha">2</div>
          <div class="alpha">3</div>
          <div class="beta">4</div>
          <span class="beta">5</span>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let report = inspector.inspect(&document);

        // alpha appears 3 times, beta appears 2 times
        assert_eq!(report.common_classes.len(), 2);
        assert_eq!(report.common_classes[0].0, "alpha");
        assert_eq!(report.common_classes[0].1, 3);
        assert_eq!(report.common_classes[1].0, "beta");
        assert_eq!(report.common_classes[1].1, 2);
    }

    // --- DefaultDomInspector::suggest tests ---

    #[test]
    fn test_default_inspector_suggest_above_threshold() {
        let html = r#"<html><body>
          <div class="article-body"><p class="article-title">content</p></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".article-body");

        // ".article-body" is an exact match candidate → score 1.0
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].selector, ".article-body");
        assert!((suggestions[0].score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_default_inspector_suggest_below_threshold() {
        // Use a selector that shares no similarity with any DOM element
        let html = r#"<html><body>
          <div class="zzzz"><p class="zzzzz">content</p></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new().with_threshold(0.99);
        let suggestions = inspector.suggest(&document, ".aaaa");

        // With threshold 0.99, only exact matches pass. ".aaaa" ≠ any candidate.
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_default_inspector_suggest_class_selector() {
        let html = r#"<html><body>
          <div class="main-content"><p>text</p></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".main-content");

        // Should include the class selector ".main-content"
        let selectors: Vec<&str> = suggestions.iter().map(|s| s.selector.as_str()).collect();
        assert!(
            selectors.contains(&".main-content"),
            "expected .main-content in suggestions, got: {selectors:?}"
        );
    }

    #[test]
    fn test_default_inspector_suggest_id_selector() {
        let html = r#"<html><body>
          <div id="header"><p>text</p></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, "#header");

        // Should include the id selector "#header"
        let selectors: Vec<&str> = suggestions.iter().map(|s| s.selector.as_str()).collect();
        assert!(
            selectors.contains(&"#header"),
            "expected #header in suggestions, got: {selectors:?}"
        );
    }

    #[test]
    fn test_default_inspector_suggest_tag_selector() {
        let html = r#"<html><body>
          <article><p>text</p></article>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, "article");

        // Should include the bare tag "article"
        let selectors: Vec<&str> = suggestions.iter().map(|s| s.selector.as_str()).collect();
        assert!(
            selectors.contains(&"article"),
            "expected 'article' tag in suggestions, got: {selectors:?}"
        );
    }

    #[test]
    fn test_default_inspector_suggest_sorted_by_score_descending() {
        let html = r#"<html><body>
          <div class="main"><div class="main-content"><div class="main-content-extra">
            <p>text</p>
          </div></div></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".main-content");

        // Verify sorted by score descending
        for window in suggestions.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "suggestions not sorted desc: {} >= {}",
                window[0].score,
                window[1].score
            );
        }
    }

    #[test]
    fn test_default_inspector_suggest_truncated_to_top_5() {
        // Many similar candidates
        let html = r#"<html><body>
          <div class="main1"><div class="main2"><div class="main3">
          <div class="main4"><div class="main5"><div class="main6">
          <div class="main7"><div class="main8"><div class="main9">
            <p>text</p>
          </div></div></div></div></div></div></div></div></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".main1");

        assert!(suggestions.len() <= 5);
    }

    #[test]
    fn test_default_inspector_suggest_empty_html() {
        let document = scraper::Html::parse_document("");
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".anything");

        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_default_inspector_builder_methods() {
        let inspector = DefaultDomInspector::new()
            .with_node_cap(500)
            .with_threshold(0.8);

        // Verify via behavior: with a cap of 500, a 600-element DOM truncates
        let html = "<html><body>".to_owned()
            + &"<div>".repeat(600)
            + &"</div>".repeat(600)
            + "</body></html>";
        let document = scraper::Html::parse_document(&html);
        let report = inspector.inspect(&document);
        assert!(report.truncated);
        assert_eq!(report.element_count, 500);
    }
}

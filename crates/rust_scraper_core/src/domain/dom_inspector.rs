//! DOM inspector port and diagnostic types — owned by the **domain** layer.
//!
//! Defines the contract for DOM structural analysis and CSS selector
//! diagnostics. The domain trait [`DomInspectorPort`] accepts a pre-parsed
//! `&scraper::Html` to avoid re-parsing on the failure path. Concrete
//! implementations live in the infrastructure layer.

use std::collections::HashMap;

/// Error kind for CSS selector diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorErrorKind {
    /// Selector matched 0 elements in the DOM.
    ZeroMatches,
    /// Selector syntax is invalid (contains the parse error message).
    InvalidSelector(String),
    /// The HTML document is empty.
    EmptyDocument,
}

/// A closest-match selector suggestion computed via Jaro-Winkler similarity.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectorSuggestion {
    /// The suggested selector text (e.g. `.main-content`, `#article`, `article`).
    pub selector: String,
    /// Jaro-Winkler similarity score (0.0 to 1.0).
    pub score: f64,
}

/// Structural report of a DOM, used for selector diagnostics.
#[derive(Debug, Clone, Default)]
pub struct DomStructureReport {
    /// Total element count (capped at the inspector's node-count cap).
    pub element_count: usize,
    /// Whether the DOM was truncated due to exceeding the node-count cap.
    pub truncated: bool,
    /// Tag name → count (e.g. `{"div": 15, "p": 8}`).
    pub tag_counts: HashMap<String, usize>,
    /// Maximum nesting depth of the DOM tree.
    pub max_depth: usize,
    /// Common class names with frequency (sorted descending, top 10).
    pub common_classes: Vec<(String, usize)>,
    /// Common id names with frequency (sorted descending, top 5).
    pub common_ids: Vec<(String, usize)>,
}

/// Diagnostic information for a failed CSS selector.
#[derive(Debug, Clone)]
pub struct SelectorDiagnostic {
    /// What went wrong.
    pub error_kind: SelectorErrorKind,
    /// Structural report of the DOM.
    pub report: DomStructureReport,
    /// Closest-match suggestions (empty if below similarity threshold).
    pub suggestions: Vec<SelectorSuggestion>,
}

/// Result of CSS selector extraction.
#[derive(Debug, Clone)]
pub enum ExtractResult {
    /// Selector matched one or more elements.
    Matched(String),
    /// Selector matched 0 elements or was invalid; falling back to full HTML.
    Fallback {
        /// The full HTML to use as fallback.
        html: String,
        /// Diagnostic info (`None` if no inspector was provided).
        diagnostic: Option<SelectorDiagnostic>,
    },
}

impl ExtractResult {
    /// Get the HTML string from either variant.
    #[must_use]
    pub fn as_html(&self) -> &str {
        match self {
            Self::Matched(html) => html,
            Self::Fallback { html, .. } => html,
        }
    }

    /// Whether the selector matched elements.
    #[must_use]
    pub fn is_matched(&self) -> bool {
        matches!(self, Self::Matched(_))
    }
}

/// Port trait for DOM inspection — analyzes DOM structure for selector diagnostics.
///
/// This trait is **sync** (CPU-bound, not I/O). Callers may use `spawn_blocking`
/// if the DOM is very large. Implementations walk the DOM tree to extract
/// structural information and compute similarity scores.
///
/// # Thread safety
///
/// Implementations must be `Send + Sync` to work with Tokio's
/// multi-threaded runtime.
pub trait DomInspectorPort: Send + Sync {
    /// Build a structural report of the DOM.
    ///
    /// # Errors
    ///
    /// This method does not return errors. It returns an empty report
    /// for empty or malformed documents.
    fn inspect(&self, document: &scraper::Html) -> DomStructureReport;

    /// Compute closest-match selector suggestions for a failed selector.
    ///
    /// # Errors
    ///
    /// This method does not return errors. It returns an empty vec if
    /// no candidates exceed the similarity threshold.
    fn suggest(&self, document: &scraper::Html, failed_selector: &str) -> Vec<SelectorSuggestion>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // --- ExtractResult::Matched ---

    #[test]
    fn test_extract_result_matched_as_html() {
        let result = ExtractResult::Matched("<p>hello</p>".to_owned());
        assert_eq!(result.as_html(), "<p>hello</p>");
    }

    #[test]
    fn test_extract_result_matched_as_html_different_content() {
        let result = ExtractResult::Matched("<div>world</div>".to_owned());
        assert_eq!(result.as_html(), "<div>world</div>");
    }

    #[test]
    fn test_extract_result_matched_is_matched() {
        let result = ExtractResult::Matched("content".to_owned());
        assert!(result.is_matched());
    }

    #[test]
    fn test_extract_result_matched_empty_string() {
        let result = ExtractResult::Matched(String::new());
        assert_eq!(result.as_html(), "");
        assert!(result.is_matched());
    }

    // --- ExtractResult::Fallback ---

    #[test]
    fn test_extract_result_fallback_as_html() {
        let result = ExtractResult::Fallback {
            html: "<html>full</html>".to_owned(),
            diagnostic: None,
        };
        assert_eq!(result.as_html(), "<html>full</html>");
    }

    #[test]
    fn test_extract_result_fallback_as_html_different_content() {
        let result = ExtractResult::Fallback {
            html: "<body>different</body>".to_owned(),
            diagnostic: None,
        };
        assert_eq!(result.as_html(), "<body>different</body>");
    }

    #[test]
    fn test_extract_result_fallback_is_matched() {
        let result = ExtractResult::Fallback {
            html: "<html></html>".to_owned(),
            diagnostic: None,
        };
        assert!(!result.is_matched());
    }

    #[test]
    fn test_extract_result_fallback_with_diagnostic_as_html() {
        let diagnostic = SelectorDiagnostic {
            error_kind: SelectorErrorKind::ZeroMatches,
            report: DomStructureReport::default(),
            suggestions: vec![],
        };
        let result = ExtractResult::Fallback {
            html: "<html>fallback</html>".to_owned(),
            diagnostic: Some(diagnostic),
        };
        assert_eq!(result.as_html(), "<html>fallback</html>");
    }

    #[test]
    fn test_extract_result_fallback_with_diagnostic_is_matched() {
        let diagnostic = SelectorDiagnostic {
            error_kind: SelectorErrorKind::InvalidSelector("bad syntax".to_owned()),
            report: DomStructureReport::default(),
            suggestions: vec![],
        };
        let result = ExtractResult::Fallback {
            html: "<html></html>".to_owned(),
            diagnostic: Some(diagnostic),
        };
        assert!(!result.is_matched());
    }

    // --- DomStructureReport::default ---

    #[test]
    fn test_dom_structure_report_default() {
        let report = DomStructureReport::default();
        assert_eq!(report.element_count, 0);
        assert!(!report.truncated);
        assert!(report.tag_counts.is_empty());
        assert_eq!(report.max_depth, 0);
        assert!(report.common_classes.is_empty());
        assert!(report.common_ids.is_empty());
    }

    #[test]
    fn test_dom_structure_report_with_data() {
        let mut tag_counts = HashMap::new();
        tag_counts.insert("div".to_owned(), 5);
        tag_counts.insert("p".to_owned(), 3);

        let report = DomStructureReport {
            element_count: 8,
            truncated: false,
            tag_counts,
            max_depth: 4,
            common_classes: vec![("main".to_owned(), 3), ("sidebar".to_owned(), 1)],
            common_ids: vec![("header".to_owned(), 1)],
        };

        assert_eq!(report.element_count, 8);
        assert!(!report.truncated);
        assert_eq!(report.tag_counts.get("div"), Some(&5));
        assert_eq!(report.tag_counts.get("p"), Some(&3));
        assert_eq!(report.max_depth, 4);
        assert_eq!(report.common_classes.len(), 2);
        assert_eq!(report.common_ids.len(), 1);
    }

    // --- SelectorDiagnostic fields ---

    #[test]
    fn test_selector_diagnostic_fields_accessible() {
        let mut tag_counts = HashMap::new();
        tag_counts.insert("article".to_owned(), 2);

        let diagnostic = SelectorDiagnostic {
            error_kind: SelectorErrorKind::ZeroMatches,
            report: DomStructureReport {
                element_count: 2,
                truncated: false,
                tag_counts,
                max_depth: 1,
                common_classes: vec![],
                common_ids: vec![],
            },
            suggestions: vec![SelectorSuggestion {
                selector: "article".to_owned(),
                score: 0.85,
            }],
        };

        assert_eq!(diagnostic.error_kind, SelectorErrorKind::ZeroMatches);
        assert_eq!(diagnostic.report.element_count, 2);
        assert_eq!(diagnostic.suggestions.len(), 1);
        assert_eq!(diagnostic.suggestions[0].selector, "article");
        assert!((diagnostic.suggestions[0].score - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_selector_diagnostic_invalid_selector_variant() {
        let diagnostic = SelectorDiagnostic {
            error_kind: SelectorErrorKind::InvalidSelector("parse error".to_owned()),
            report: DomStructureReport::default(),
            suggestions: vec![],
        };

        match diagnostic.error_kind {
            SelectorErrorKind::InvalidSelector(ref msg) => assert_eq!(msg, "parse error"),
            _ => panic!("expected InvalidSelector variant"),
        }
    }

    #[test]
    fn test_selector_diagnostic_empty_document_variant() {
        let diagnostic = SelectorDiagnostic {
            error_kind: SelectorErrorKind::EmptyDocument,
            report: DomStructureReport::default(),
            suggestions: vec![],
        };

        assert_eq!(diagnostic.error_kind, SelectorErrorKind::EmptyDocument);
    }
}

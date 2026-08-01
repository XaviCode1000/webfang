//! Selector diagnostics — build failure diagnostics for CSS selector extraction.
//!
//! Helper used on the extraction failure path (0 matches, invalid selector, or
//! empty document) to attach a [`SelectorDiagnostic`] when a DOM inspector is
//! available.

use crate::domain::{DomInspectorPort, SelectorDiagnostic, SelectorErrorKind};

/// Build a [`SelectorDiagnostic`] using the inspector, or return `None` if no
/// inspector was provided.
///
/// This helper calls `inspector.inspect()` for the DOM structure report and
/// `inspector.suggest()` for closest-match selector suggestions. It is only
/// called on the failure path (0 matches or invalid selector).
pub(crate) fn build_diagnostic(
    inspector: Option<&dyn DomInspectorPort>,
    document: &scraper::Html,
    error_kind: SelectorErrorKind,
    failed_selector: &str,
) -> Option<SelectorDiagnostic> {
    inspector.map(|insp| SelectorDiagnostic {
        error_kind,
        report: insp.inspect(document),
        suggestions: insp.suggest(document, failed_selector),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DomStructureReport, SelectorSuggestion};

    /// Deterministic inspector stub: returns a fixed report and a single
    /// suggestion derived from the failed selector. No infrastructure involved.
    struct StubInspector;

    impl DomInspectorPort for StubInspector {
        fn inspect(&self, _document: &scraper::Html) -> DomStructureReport {
            DomStructureReport {
                element_count: 7,
                ..DomStructureReport::default()
            }
        }

        fn suggest(
            &self,
            _document: &scraper::Html,
            failed_selector: &str,
        ) -> Vec<SelectorSuggestion> {
            vec![SelectorSuggestion {
                selector: format!("suggest-{failed_selector}"),
                score: 0.9,
            }]
        }
    }

    #[test]
    fn test_no_inspector_returns_none() {
        let document = scraper::Html::parse_document("<html><body></body></html>");
        let result = build_diagnostic(None, &document, SelectorErrorKind::ZeroMatches, "div.x");
        assert!(
            result.is_none(),
            "without an inspector no diagnostic can be produced"
        );
    }

    #[test]
    fn test_with_inspector_wires_report_and_suggestions() {
        let document = scraper::Html::parse_document("<html><body><p>x</p></body></html>");
        let result = build_diagnostic(
            Some(&StubInspector),
            &document,
            SelectorErrorKind::ZeroMatches,
            "div.missing",
        );
        let diagnostic = result.expect("an inspector must produce a diagnostic");

        assert_eq!(diagnostic.error_kind, SelectorErrorKind::ZeroMatches);
        assert_eq!(diagnostic.report.element_count, 7);
        assert_eq!(diagnostic.suggestions.len(), 1);
        assert_eq!(diagnostic.suggestions[0].selector, "suggest-div.missing");
    }

    #[test]
    fn test_error_kind_is_preserved() {
        let document = scraper::Html::parse_document("");
        let result = build_diagnostic(
            Some(&StubInspector),
            &document,
            SelectorErrorKind::EmptyDocument,
            "article",
        );
        let diagnostic = result.expect("an inspector must produce a diagnostic");
        assert_eq!(diagnostic.error_kind, SelectorErrorKind::EmptyDocument);
    }
}

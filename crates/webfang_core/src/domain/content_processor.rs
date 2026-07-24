//! Content processing strategy — Domain layer
//!
//! Defines the contract for converting raw HTML into usable text.
//! Different consumers need different cleaning behaviors:
//! - Semantic extraction for RAG/pipeline (preserves readability structure)
//! - Aggressive stripping for download pipeline (removes all noise)
//! - MCP tools (removes boilerplate, keeps semantic tags)
//!
//! Following Clean Architecture: this trait is pure domain logic with no
//! infrastructure dependencies.

use std::fmt;

/// Content processing strategy for converting HTML to usable text.
///
/// Different consumers need different cleaning behaviors:
/// - Semantic extraction for RAG/pipeline (preserves readability structure)
/// - Aggressive stripping for download pipeline (removes all noise)
/// - MCP tools (removes boilerplate, keeps semantic tags)
///
/// The trait is intentionally simple — implementations hold the complexity.
pub trait ContentProcessor: Send + Sync {
    /// Process raw HTML into cleaned text according to this processor's strategy.
    fn process(&self, html: &str) -> String;

    /// Human-readable name for logging/diagnostics.
    fn name(&self) -> &str;
}

/// Display impl for logging.
impl fmt::Display for dyn ContentProcessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyProcessor;

    impl ContentProcessor for DummyProcessor {
        fn process(&self, html: &str) -> String {
            html.to_string()
        }

        fn name(&self) -> &str {
            "dummy"
        }
    }

    #[test]
    fn processor_name_displayed() {
        let p = DummyProcessor;
        let name = p.name();
        assert_eq!(name, "dummy");
    }

    #[test]
    fn dyn_display_shows_name() {
        let p: &dyn ContentProcessor = &DummyProcessor;
        assert_eq!(format!("{p}"), "dummy");
    }

    #[test]
    fn process_returns_input() {
        let p = DummyProcessor;
        assert_eq!(p.process("<p>hi</p>"), "<p>hi</p>");
    }
}

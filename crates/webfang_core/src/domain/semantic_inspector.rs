//! Semantic inspector port — async domain trait for Tier 2 selector repair.
//!
//! Defines the contract for semantic (embedding-based) CSS selector matching.
//! Concrete implementations live in the `webfang_ai` crate's infrastructure
//! layer. The trait uses `BoxFuture` for dyn-compatibility, matching the
//! pattern established in [`super::repository::VectorRepository`].

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use super::dom_inspector::SelectorErrorKind;

/// A boxed future for dyn-compatible async traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Whether a selector suggestion came from lexical or semantic analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TierSource {
    /// Jaro-Winkler lexical similarity (Tier 1).
    Lexical,
    /// Embedding-based semantic similarity (Tier 2).
    Semantic,
}

/// A semantic match candidate with confidence and provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticMatch {
    /// The suggested CSS selector.
    pub selector: String,
    /// Semantic similarity score (0.0 to 1.0).
    pub confidence: f32,
    /// Whether this came from lexical or semantic analysis.
    pub source: TierSource,
}

/// Context passed to the semantic inspector for selector matching.
pub struct SemanticContext {
    /// The text content that the failed selector was targeting.
    pub target_text: String,
    /// DOM text fragments extracted from the page for comparison.
    pub dom_fragments: Vec<String>,
    /// Optional domain hint for context-aware matching.
    pub domain_hint: Option<String>,
}

/// Domain trait for semantic (embedding-based) CSS selector repair.
///
/// Implementations use vector embeddings to find the best matching selector
/// when lexical similarity (Tier 1) is insufficient. The trait is
/// dyn-compatible via `BoxFuture` (matching `VectorRepository` in
/// `repository.rs`).
pub trait SemanticInspectorPort: Send + Sync {
    /// Find the best semantic match for a failed selector.
    ///
    /// Returns `Ok(Some(match))` if a candidate exceeds the confidence
    /// threshold, `Ok(None)` if no candidate is good enough, or
    /// `Err(SelectorErrorKind)` on infrastructure failure.
    fn find_semantic_match<'a>(
        &'a self,
        ctx: SemanticContext,
    ) -> BoxFuture<'a, Result<Option<SemanticMatch>, SelectorErrorKind>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_match_serialization_roundtrip() {
        let m = SemanticMatch {
            selector: ".article-body".to_owned(),
            confidence: 0.92,
            source: TierSource::Semantic,
        };
        let json = serde_json::to_string(&m).unwrap();
        let deserialized: SemanticMatch = serde_json::from_str(&json).unwrap();
        assert_eq!(m.selector, deserialized.selector);
        assert!((m.confidence - deserialized.confidence).abs() < f32::EPSILON);
        assert_eq!(m.source, deserialized.source);
    }

    #[test]
    fn test_tier_source_display_debug() {
        assert_eq!(format!("{:?}", TierSource::Lexical), "Lexical");
        assert_eq!(format!("{:?}", TierSource::Semantic), "Semantic");
    }
}

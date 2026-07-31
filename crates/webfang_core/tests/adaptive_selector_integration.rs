//! Integration tests for the adaptive selector repair engine.
//!
//! Tests the 2-tier cascade (lexical + semantic) with caching and backpressure.

#![cfg(feature = "adaptive-selectors")]

use std::sync::Arc;

use webfang_core::application::adaptive_engine::{
    AdaptiveSelectorEngine, AdaptiveSelectorOptions, RepairStatus,
};
use webfang_core::domain::dom_inspector::{
    DomInspectorPort, DomStructureReport, SelectorSuggestion,
};
use webfang_core::domain::semantic_inspector::{
    SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
};

// ---------------------------------------------------------------------------
// Mock helpers
// ---------------------------------------------------------------------------

/// Mock DOM inspector — returns configurable suggestions.
struct MockDomInspector {
    suggestions: Vec<SelectorSuggestion>,
}

impl DomInspectorPort for MockDomInspector {
    fn inspect(&self, _document: &scraper::Html) -> DomStructureReport {
        DomStructureReport::default()
    }

    fn suggest(
        &self,
        _document: &scraper::Html,
        _failed_selector: &str,
    ) -> Vec<SelectorSuggestion> {
        self.suggestions.clone()
    }
}

/// Mock semantic inspector — returns configurable matches.
struct MockSemanticInspector {
    response: Option<SemanticMatch>,
}

impl SemanticInspectorPort for MockSemanticInspector {
    fn find_semantic_match<'a>(
        &'a self,
        _ctx: SemanticContext,
    ) -> webfang_core::domain::semantic_inspector::BoxFuture<
        'a,
        Result<Option<SemanticMatch>, webfang_core::domain::dom_inspector::SelectorErrorKind>,
    > {
        let response = self.response.clone();
        Box::pin(async { Ok(response) })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tier1_high_score_no_tier2() {
    let inspector = Arc::new(MockDomInspector {
        suggestions: vec![SelectorSuggestion {
            selector: ".main-content".to_owned(),
            score: 0.92,
        }],
    });

    let engine = AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
    let html = "<div class='main-content'>Hello</div>";
    let document = scraper::Html::parse_document(html);

    let result = engine.repair(&document, html, ".content", None).await;
    assert!(
        result.is_ok(),
        "repair should succeed with high Tier 1 score"
    );

    let outcome = result.unwrap();
    assert_eq!(outcome.status, RepairStatus::Repaired);
    assert_eq!(outcome.suggestion.selector, ".main-content");
    // Tier 2 should not have been called
    assert!(
        outcome.trace.as_ref().unwrap().tier2_score.is_none(),
        "Tier 2 should not run when Tier 1 score > 0.85"
    );
}

#[tokio::test]
async fn test_tier1_low_score_no_semantic_fallback() {
    let inspector = Arc::new(MockDomInspector {
        suggestions: vec![SelectorSuggestion {
            selector: ".similar".to_owned(),
            score: 0.65,
        }],
    });

    let engine = AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
    let html = "<div class='similar'>Hello</div>";
    let document = scraper::Html::parse_document(html);

    let result = engine.repair(&document, html, ".content", None).await;
    assert!(
        result.is_ok(),
        "should return Tier 1 best when no semantic provider"
    );

    let outcome = result.unwrap();
    assert_eq!(outcome.status, RepairStatus::Repaired);
    assert_eq!(outcome.suggestion.selector, ".similar");
}

#[tokio::test]
async fn test_no_suggestions_returns_error() {
    let inspector = Arc::new(MockDomInspector {
        suggestions: vec![],
    });

    let engine = AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
    let html = "<div>Hello</div>";
    let document = scraper::Html::parse_document(html);

    let result = engine.repair(&document, html, ".content", None).await;
    assert!(result.is_err(), "should fail with no suggestions");

    match result.unwrap_err() {
        webfang_core::domain::dom_inspector::SelectorErrorKind::RepairInconclusive(_) => {},
        other => panic!("expected RepairInconclusive, got {other:?}"),
    }
}

#[tokio::test]
async fn test_cache_hit_returns_cached() {
    let inspector = Arc::new(MockDomInspector {
        suggestions: vec![SelectorSuggestion {
            selector: ".cached".to_owned(),
            score: 0.90,
        }],
    });

    let engine = AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
    let html = "<div class='cached'>Hello</div>";
    let document = scraper::Html::parse_document(html);

    // First call — populates cache
    let r1 = engine
        .repair(&document, html, ".target", None)
        .await
        .unwrap();
    assert!(!r1.trace.as_ref().unwrap().cache_hit);
    assert_eq!(engine.cache_len(), 1);

    // Second call — should hit cache
    let r2 = engine
        .repair(&document, html, ".target", None)
        .await
        .unwrap();
    assert!(
        r2.trace.as_ref().unwrap().cache_hit,
        "second call should hit cache, cache_len={}",
        engine.cache_len()
    );
}

#[tokio::test]
async fn test_semantic_provider_called_in_ambiguity_zone() {
    let inspector = Arc::new(MockDomInspector {
        suggestions: vec![SelectorSuggestion {
            selector: ".similar".to_owned(),
            score: 0.75, // In ambiguity band (0.70-0.85)
        }],
    });

    let semantic = Arc::new(MockSemanticInspector {
        response: Some(SemanticMatch {
            selector: ".article-body".to_owned(),
            confidence: 0.88,
            source: TierSource::Semantic,
        }),
    });

    let engine = AdaptiveSelectorEngine::new(
        inspector,
        Some(semantic),
        AdaptiveSelectorOptions::default(),
    );
    let html = "<div class='article-body'>Hello</div>";
    let document = scraper::Html::parse_document(html);

    let result = engine.repair(&document, html, ".content", None).await;
    assert!(result.is_ok());

    let outcome = result.unwrap();
    assert_eq!(outcome.status, RepairStatus::Repaired);
    assert_eq!(outcome.suggestion.selector, ".article-body");
    // Tier 2 should have been called
    assert!(
        outcome.trace.as_ref().unwrap().tier2_score.is_some(),
        "Tier 2 should run in the ambiguity zone"
    );
}

#[tokio::test]
async fn test_semaphore_timeout_skips_tier2() {
    // Test that when Tier 2 is unavailable (no semantic provider),
    // Tier 1 best is returned as fallback — same behavior as semaphore timeout.
    let inspector = Arc::new(MockDomInspector {
        suggestions: vec![SelectorSuggestion {
            selector: ".fallback".to_owned(),
            score: 0.75,
        }],
    });
    let engine = AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
    let html = "<div class='similar'>Hello</div>";
    let document = scraper::Html::parse_document(html);

    let result = engine.repair(&document, html, ".content", None).await;
    assert!(result.is_ok());
    let outcome = result.unwrap();
    assert_eq!(
        outcome.suggestion.selector, ".fallback",
        "should return Tier 1 best when Tier 2 is unavailable"
    );
    assert!(
        outcome.trace.as_ref().unwrap().tier2_score.is_none(),
        "Tier 2 should not be called when no semantic provider"
    );
}

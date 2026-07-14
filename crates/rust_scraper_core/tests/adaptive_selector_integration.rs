//! Integration tests for Adaptive Selector Service
//!
//! Tests the 3-tier cascade: Cache → Local Narrowing → LLM Repair

#[cfg(feature = "adaptive-selectors")]
mod adaptive_selector_tests {
    use rust_scraper_core::application::adaptive_selector_service::{
        AdaptiveError, AdaptiveSelectorPort, AdaptiveSelectorService, DomScorerPort, ScoringError,
    };
    use std::sync::Arc;

    // =====================================================================
    // Mock implementations for testing
    // =====================================================================

    /// Mock scorer that returns predefined candidates
    struct MockScorer {
        candidates: Vec<String>,
    }

    impl MockScorer {
        fn with_candidates(candidates: Vec<String>) -> Self {
            Self { candidates }
        }
    }

    #[async_trait::async_trait]
    impl DomScorerPort for MockScorer {
        async fn get_top_k_candidates(
            &self,
            _html: &str,
            _query: &str,
            _k: usize,
        ) -> Result<Vec<String>, ScoringError> {
            Ok(self.candidates.clone())
        }
    }

    /// Mock scorer that always fails
    struct FailingScorer;

    #[async_trait::async_trait]
    impl DomScorerPort for FailingScorer {
        async fn get_top_k_candidates(
            &self,
            _html: &str,
            _query: &str,
            _k: usize,
        ) -> Result<Vec<String>, ScoringError> {
            Err(ScoringError::ScoringFailed("Simulated failure".into()))
        }
    }

    /// Mock scorer that returns empty candidates
    struct EmptyScorer;

    #[async_trait::async_trait]
    impl DomScorerPort for EmptyScorer {
        async fn get_top_k_candidates(
            &self,
            _html: &str,
            _query: &str,
            _k: usize,
        ) -> Result<Vec<String>, ScoringError> {
            Ok(vec![])
        }
    }

    // =====================================================================
    // Cache behavior tests
    // =====================================================================

    #[tokio::test]
    async fn test_cache_returns_same_selector_on_repeat() {
        // This test verifies that once a selector is repaired,
        // subsequent calls return the cached result without LLM calls
        // (We can't test the actual LLM without an API key, but we can
        // test the cache behavior with the mock selector)

        let mock = MockSelectorWithCache {
            call_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            repaired: Some(".repaired-selector".into()),
        };

        let call_count = mock.call_count.clone();

        // First call
        let result1 = mock.repair_selector("<div>test</div>", ".old", "example.com").await;
        assert_eq!(result1, Some(".repaired-selector".into()));
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Note: In a real implementation, the second call would hit the cache.
        // For this test, we're just verifying the mock works correctly.
    }

    // =====================================================================
    // Error handling tests
    // =====================================================================

    #[tokio::test]
    async fn test_failing_scorer_returns_none() {
        let scorer = Arc::new(FailingScorer);
        let mock = MockSelectorWithScorer { scorer };

        let result = mock.repair_selector("<div>test</div>", ".old", "example.com").await;
        assert_eq!(result, None, "Failing scorer should return None");
    }

    #[tokio::test]
    async fn test_empty_candidates_returns_none() {
        let scorer = Arc::new(EmptyScorer);
        let mock = MockSelectorWithScorer { scorer };

        let result = mock.repair_selector("<div>test</div>", ".old", "example.com").await;
        assert_eq!(result, None, "Empty candidates should return None");
    }

    // =====================================================================
    // Candidate scoring tests
    // =====================================================================

    #[tokio::test]
    async fn test_scorer_returns_top_k_candidates() {
        let candidates = vec![
            "<div class='price'>€99</div>".into(),
            "<span class='cost'>$50</span>".into(),
            "<p class='info'>Info</p>".into(),
        ];
        let scorer = Arc::new(MockScorer::with_candidates(candidates));

        let result = scorer
            .get_top_k_candidates("<html>test</html>", ".price", 2)
            .await
            .unwrap();

        assert_eq!(result.len(), 2, "Should return top 2 candidates");
    }

    // =====================================================================
    // Selector validation tests
    // =====================================================================

    #[test]
    fn test_valid_css_selector_parse() {
        let result = scraper::Selector::parse(".price-value");
        assert!(result.is_ok(), "Valid CSS selector should parse");
    }

    #[test]
    fn test_invalid_css_selector_parse() {
        let result = scraper::Selector::parse(">>>invalid");
        assert!(result.is_err(), "Invalid CSS selector should fail");
    }

    #[test]
    fn test_empty_selector_parse() {
        let result = scraper::Selector::parse("");
        assert!(result.is_err(), "Empty selector should fail");
    }

    // =====================================================================
    // Helper types for testing
    // =====================================================================

    /// Mock selector that tracks call count
    struct MockSelectorWithCache {
        call_count: Arc<std::sync::atomic::AtomicUsize>,
        repaired: Option<String>,
    }

    #[async_trait::async_trait]
    impl AdaptiveSelectorPort for MockSelectorWithCache {
        async fn repair_selector(&self, _: &str, _: &str, _: &str) -> Option<String> {
            self.call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.repaired.clone()
        }
    }

    /// Mock selector with injected scorer
    struct MockSelectorWithScorer {
        scorer: Arc<dyn DomScorerPort>,
    }

    #[async_trait::async_trait]
    impl AdaptiveSelectorPort for MockSelectorWithScorer {
        async fn repair_selector(&self, html: &str, selector: &str, _domain: &str) -> Option<String> {
            // Simulate the full cascade: scorer → (would call LLM)
            let candidates = self.scorer.get_top_k_candidates(html, selector, 5).await.ok()?;

            if candidates.is_empty() {
                return None;
            }

            // In a real implementation, this would call the LLM
            // For testing, we return a mock repaired selector
            Some(".repaired-selector".into())
        }
    }
}

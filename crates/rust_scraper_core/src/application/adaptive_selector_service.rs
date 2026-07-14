//! Adaptive Selector Service — LLM-powered DOM resilience
//!
//! When a CSS selector fails to match any elements, this service uses
//! a 3-tier cascade to find and repair the selector:
//!
//! 1. **Cache Check**: DashMap lookup for previously repaired selectors
//! 2. **Local Narrowing**: Score DOM fragments by semantic similarity
//! 3. **LLM Repair**: rig-core + Gemini generates a new CSS selector
//!
//! # Feature Gate
//!
//! This module requires the `adaptive-selectors` feature (which implies `ai`).

use std::sync::Arc;

use dashmap::DashMap;
use rig_core::agent::Agent;
use rig_core::client::{CompletionClient, ProviderClient};
use rig_core::completion::Prompt;
use rig_core::providers::gemini;
use rig_core::providers::gemini::completion::GEMINI_2_0_FLASH;
use tracing::{debug, info, warn};

/// Port trait for dependency injection (allows testing without real LLM)
#[async_trait::async_trait]
pub trait AdaptiveSelectorPort: Send + Sync {
    /// Attempt to repair a failed CSS selector
    async fn repair_selector(
        &self,
        html: &str,
        failed_selector: &str,
        domain: &str,
    ) -> Option<String>;
}

/// Port trait for DOM fragment scoring
#[async_trait::async_trait]
pub trait DomScorerPort: Send + Sync {
    /// Get top-k DOM fragments most similar to a query
    async fn get_top_k_candidates(
        &self,
        html: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<String>, ScoringError>;
}

/// Errors from scoring operations
#[derive(Debug, thiserror::Error)]
pub enum ScoringError {
    #[error("Scoring failed: {0}")]
    ScoringFailed(String),
}

/// Adaptive Selector Service — 3-tier cascade for selector repair
pub struct AdaptiveSelectorService {
    llm_agent: Agent<gemini::CompletionModel>,
    scorer: Arc<dyn DomScorerPort>,
    cache: Arc<DashMap<String, String>>,
    top_k: usize,
    max_tokens: usize,
}

impl AdaptiveSelectorService {
    pub fn new(
        scorer: Arc<dyn DomScorerPort>,
        top_k: usize,
        max_tokens: usize,
    ) -> Result<Self, AdaptiveError> {
        let gemini_client =
            gemini::Client::from_env().map_err(|e| AdaptiveError::LlmClientInit(e.to_string()))?;

        let llm_agent = gemini_client
            .agent(GEMINI_2_0_FLASH)
            .preamble(
                "You are an expert CSS selector engineer. Given HTML content and a failed CSS selector, \
                 generate a new CSS selector that matches the same semantic content. \
                 Return ONLY the CSS selector string, nothing else.",
            )
            .build();

        Ok(Self {
            llm_agent,
            scorer,
            cache: Arc::new(DashMap::new()),
            top_k,
            max_tokens,
        })
    }

    pub fn cache_stats(&self) -> (usize, usize) {
        (self.cache.len(), self.cache.capacity())
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

#[async_trait::async_trait]
impl AdaptiveSelectorPort for AdaptiveSelectorService {
    async fn repair_selector(
        &self,
        html: &str,
        failed_selector: &str,
        domain: &str,
    ) -> Option<String> {
        let cache_key = format!("{}:{}", domain, failed_selector);

        // Tier 1: Cache Check
        if let Some(cached) = self.cache.get(&cache_key) {
            debug!(target: "adaptive_selector", "Cache hit: {}", cache_key);
            return Some(cached.value().clone());
        }

        // Tier 2: Local Narrowing
        let candidates = self
            .scorer
            .get_top_k_candidates(html, failed_selector, self.top_k)
            .await
            .ok()?;

        if candidates.is_empty() {
            return None;
        }

        // Tier 3: LLM Repair
        let context = candidates.join("\n---\n");
        let truncated = if context.len() > self.max_tokens * 4 {
            &context[..self.max_tokens * 4]
        } else {
            &context
        };

        let prompt = format!(
            "Failed selector: {}\n\nHTML context:\n{}\n\nGenerate a new CSS selector:",
            failed_selector, truncated
        );

        match self.llm_agent.prompt(&prompt).await {
            Ok(response) => {
                let new_selector = response.trim().to_string();
                if scraper::Selector::parse(&new_selector).is_ok() {
                    info!(target: "adaptive_selector", "Repaired: {} -> {}", failed_selector, new_selector);
                    self.cache.insert(cache_key, new_selector.clone());
                    Some(new_selector)
                } else {
                    warn!(target: "adaptive_selector", "Invalid selector: {}", new_selector);
                    None
                }
            },
            Err(e) => {
                warn!(target: "adaptive_selector", "LLM failed: {}", e);
                None
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdaptiveError {
    #[error("Failed to initialize LLM client: {0}")]
    LlmClientInit(String),
    #[error("LLM repair failed: {0}")]
    LlmRepairFailed(String),
    #[error("Token budget exceeded: {0}")]
    TokenBudgetExceeded(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockScorer;

    #[async_trait::async_trait]
    impl DomScorerPort for MockScorer {
        async fn get_top_k_candidates(
            &self,
            _html: &str,
            _query: &str,
            _k: usize,
        ) -> Result<Vec<String>, ScoringError> {
            Ok(vec!["<div class='price'>€99</div>".into()])
        }
    }

    struct MockAdaptiveSelector {
        repaired: Option<String>,
    }

    #[async_trait::async_trait]
    impl AdaptiveSelectorPort for MockAdaptiveSelector {
        async fn repair_selector(&self, _: &str, _: &str, _: &str) -> Option<String> {
            self.repaired.clone()
        }
    }

    #[tokio::test]
    async fn test_mock_returns_repaired_selector() {
        let mock = MockAdaptiveSelector {
            repaired: Some(".new-selector".into()),
        };
        let result = mock
            .repair_selector("<div>test</div>", ".old", "example.com")
            .await;
        assert_eq!(result, Some(".new-selector".into()));
    }

    #[tokio::test]
    async fn test_mock_returns_none() {
        let mock = MockAdaptiveSelector { repaired: None };
        let result = mock
            .repair_selector("<div>test</div>", ".old", "example.com")
            .await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_mock_scorer_returns_candidates() {
        let scorer = MockScorer;
        let candidates = scorer
            .get_top_k_candidates("<div>test</div>", ".price", 5)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
    }
}

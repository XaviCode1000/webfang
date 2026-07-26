//! Adaptive CSS selector repair engine — 2-tier cascade with caching.
//!
//! Tier 1 (lexical, Jaro-Winkler) runs synchronously via [`DomInspectorPort`].
//! Tier 2 (semantic, embeddings) runs asynchronously via [`SemanticInspectorPort`]
//! only when Tier 1 confidence is ambiguous (0.70–0.85). A [`DashMap`] cache
//! with TTL prevents redundant inference calls.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Semaphore;
use tracing::{debug, info, instrument, warn};

use crate::domain::dom_inspector::{
    DomInspectorPort, RepairFailureDiagnostic, SelectorErrorKind, SelectorSuggestion,
};
use crate::domain::semantic_inspector::{SemanticContext, SemanticInspectorPort};

/// Whether the repair succeeded and how.
#[derive(Debug, Clone, PartialEq)]
pub enum RepairStatus {
    /// Original selector was kept (score > 0.85, no repair needed).
    Original,
    /// Selector was repaired by Tier 1 or Tier 2.
    Repaired,
    /// Tier 2 returned a result but with low confidence — marked as degraded.
    Degraded,
}

/// Trace data for a cascade repair attempt.
#[derive(Debug, Clone)]
pub struct CascadeTrace {
    /// Tier 1 lexical similarity score (always present).
    pub tier1_score: f64,
    /// Tier 2 semantic confidence (None if Tier 2 was skipped).
    pub tier2_score: Option<f32>,
    /// Tier 2 inference latency in milliseconds (0 if skipped).
    pub tier2_latency_ms: u64,
    /// Whether this result came from the cache.
    pub cache_hit: bool,
}

/// Result of an adaptive selector repair attempt.
#[derive(Debug, Clone)]
pub struct AdaptiveRepairOutcome {
    /// The best selector suggestion found.
    pub suggestion: SelectorSuggestion,
    /// Repair status indicating success level.
    pub status: RepairStatus,
    /// Optional trace data for observability.
    pub trace: Option<CascadeTrace>,
}

impl AdaptiveRepairOutcome {
    /// Create a successful repair outcome.
    pub fn repaired(suggestion: SelectorSuggestion, trace: CascadeTrace) -> Self {
        Self {
            suggestion,
            status: RepairStatus::Repaired,
            trace: Some(trace),
        }
    }

    /// Create a failed repair outcome (both tiers exhausted).
    pub fn failed(diagnostic: RepairFailureDiagnostic) -> Result<Self, SelectorErrorKind> {
        Err(SelectorErrorKind::RepairInconclusive(Box::new(diagnostic)))
    }
}

/// Options for the adaptive selector engine.
#[derive(Debug, Clone)]
pub struct AdaptiveSelectorOptions {
    /// Tier 1 score above which we trust lexical (skip Tier 2).
    pub lexical_threshold: f64,
    /// Tier 1 score below which we skip Tier 2 (go straight to semantic).
    /// Wait — actually below 0.70 we still go to Tier 2. This is the
    /// lower bound of the ambiguity band.
    pub escalation_lower_bound: f64,
    /// Tier 2 confidence threshold for accepting a semantic match.
    pub semantic_threshold: f32,
    /// Cache TTL (time-to-live) for cached repair outcomes.
    pub cache_ttl: Duration,
    /// Maximum cache entries before lazy eviction.
    pub max_cache_entries: usize,
    /// Semaphore permits for concurrent Tier 2 inference.
    pub max_concurrent_inference: usize,
}

impl Default for AdaptiveSelectorOptions {
    fn default() -> Self {
        Self {
            lexical_threshold: 0.85,
            escalation_lower_bound: 0.70,
            semantic_threshold: 0.75,
            cache_ttl: Duration::from_secs(30 * 60), // 30 minutes
            max_cache_entries: 10_000,
            max_concurrent_inference: 4,
        }
    }
}

/// Cached repair entry with expiration.
struct CachedEntry {
    outcome: AdaptiveRepairOutcome,
    expires_at: Instant,
}

/// 2-tier adaptive CSS selector repair engine.
///
/// Combines lexical (Tier 1) and semantic (Tier 2) selector suggestions
/// with a DashMap cache and semaphore-based backpressure for inference.
pub struct AdaptiveSelectorEngine {
    inspector: Arc<dyn DomInspectorPort>,
    semantic: Option<Arc<dyn SemanticInspectorPort>>,
    cache: DashMap<u64, CachedEntry>,
    inference_semaphore: Arc<Semaphore>,
    options: AdaptiveSelectorOptions,
}

impl AdaptiveSelectorEngine {
    /// Create a new engine with the given inspector and optional semantic provider.
    pub fn new(
        inspector: Arc<dyn DomInspectorPort>,
        semantic: Option<Arc<dyn SemanticInspectorPort>>,
        options: AdaptiveSelectorOptions,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(options.max_concurrent_inference));
        Self {
            inspector,
            semantic,
            cache: DashMap::new(),
            inference_semaphore: semaphore,
            options,
        }
    }

    /// Compute a structural hash from DOM tag counts for cache keying.
    #[instrument(skip(self))]
    fn structural_hash(&self, document: &scraper::Html) -> u64 {
        use std::hash::{Hash, Hasher};

        let report = self.inspector.inspect(document);
        // Build a deterministic string from sorted tag counts and hash it
        let mut tags: Vec<_> = report.tag_counts.iter().collect();
        tags.sort_by_key(|(tag, _)| tag.as_str());
        let mut combined = String::new();
        for (tag, count) in &tags {
            combined.push_str(tag);
            combined.push(':');
            combined.push_str(&count.to_string());
            combined.push(',');
        }
        // Use a simple FNV-1a hash for determinism
        let mut hasher = FnvHasher::default();
        combined.hash(&mut hasher);
        hasher.finish()
    }

    /// Attempt repair using Tier 1 (lexical) only — synchronous.
    ///
    /// Returns `Some(outcome)` if Tier 1 found a good match, `None` if
    /// Tier 2 should be attempted.
    #[instrument(skip(self, document), fields(failed_selector = %failed_selector))]
    pub fn repair_tier1(
        &self,
        document: &scraper::Html,
        failed_selector: &str,
    ) -> Option<AdaptiveRepairOutcome> {
        let structural_hash = self.structural_hash(document);

        // Check cache first
        let cache_key = self.cache_key(failed_selector, structural_hash);
        if let Some(entry) = self.cache.get(&cache_key) {
            if entry.expires_at > Instant::now() {
                debug!(cache_hit = true, "returning cached repair outcome");
                return Some(entry.outcome.clone());
            }
        }

        let suggestions = self.inspector.suggest(document, failed_selector);
        if suggestions.is_empty() {
            debug!("tier1: no suggestions from lexical inspector");
            return None;
        }

        let best = suggestions.into_iter().max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;

        debug!(score = best.score, "tier1_evaluated");

        if best.score > self.options.lexical_threshold {
            let trace = CascadeTrace {
                tier1_score: best.score,
                tier2_score: None,
                tier2_latency_ms: 0,
                cache_hit: false,
            };
            let outcome = AdaptiveRepairOutcome::repaired(best, trace);
            self.cache_insert(cache_key, outcome.clone());
            info!(new_selector = %outcome.suggestion.selector, method = "tier1_lexical", "repair_resolved");
            Some(outcome)
        } else if best.score <= self.options.escalation_lower_bound {
            // Below lower bound — Tier 2 will be tried by the async caller
            None
        } else {
            // In ambiguity band (0.70-0.85) — Tier 2 will be tried
            None
        }
    }

    /// Full async repair cascade: Tier 1 + optional Tier 2.
    ///
    /// # Errors
    ///
    /// Returns `SelectorErrorKind::RepairInconclusive` if both tiers fail.
    #[instrument(skip(self, document, html), fields(failed_selector = %failed_selector))]
    pub async fn repair(
        &self,
        document: &scraper::Html,
        html: &str,
        failed_selector: &str,
        domain: Option<&str>,
    ) -> Result<AdaptiveRepairOutcome, SelectorErrorKind> {
        let structural_hash = self.structural_hash(document);

        // Check cache
        let cache_key = self.cache_key(failed_selector, structural_hash);
        if let Some(entry) = self.cache.get(&cache_key) {
            if entry.expires_at > Instant::now() {
                let mut outcome = entry.outcome.clone();
                if let Some(ref mut trace) = outcome.trace {
                    trace.cache_hit = true;
                }
                debug!(cache_hit = true, "returning cached repair outcome");
                return Ok(outcome);
            }
        }

        // Tier 1: lexical
        let suggestions = self.inspector.suggest(document, failed_selector);
        let best_tier1 = suggestions.into_iter().max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let tier1_score = best_tier1.as_ref().map(|s| s.score).unwrap_or(0.0);
        debug!(score = tier1_score, "tier1_evaluated");

        // Fast path: high confidence → return immediately
        if tier1_score > self.options.lexical_threshold {
            if let Some(best) = best_tier1 {
                let trace = CascadeTrace {
                    tier1_score,
                    tier2_score: None,
                    tier2_latency_ms: 0,
                    cache_hit: false,
                };
                let outcome = AdaptiveRepairOutcome::repaired(best, trace);
                self.cache_insert(cache_key, outcome.clone());
                info!(new_selector = %outcome.suggestion.selector, method = "tier1_lexical", "repair_resolved");
                return Ok(outcome);
            }
        }

        // Tier 2: semantic (if available and in ambiguity zone)
        let semantic = match &self.semantic {
            Some(s) => s,
            None => {
                // No semantic provider — return Tier 1 best or fail
                return self
                    .handle_tier1_only(best_tier1, tier1_score, structural_hash, cache_key)
                    .await;
            },
        };

        // Check Tier 2 availability — skip if inference pool is saturated
        let _permit = match tokio::time::timeout(
            Duration::from_millis(100),
            self.inference_semaphore.acquire(),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => {
                warn!("inference semaphore closed, skipping tier2");
                return self
                    .handle_tier1_only(best_tier1, tier1_score, structural_hash, cache_key)
                    .await;
            },
            Err(_) => {
                warn!("tier2 inference pool saturated (semaphore timeout), skipping tier2");
                return self
                    .handle_tier1_only(best_tier1, tier1_score, structural_hash, cache_key)
                    .await;
            },
        };

        // Build semantic context
        let ctx = SemanticContext {
            target_text: html.to_owned(),
            dom_fragments: self.extract_dom_fragments(document),
            domain_hint: domain.map(|d| d.to_owned()),
        };

        let tier2_start = Instant::now();
        let tier2_result = semantic.find_semantic_match(ctx).await;
        let tier2_latency_ms = tier2_start.elapsed().as_millis() as u64;

        match tier2_result {
            Ok(Some(sem_match)) => {
                debug!(confidence = sem_match.confidence, "tier2_escalated");

                let trace = CascadeTrace {
                    tier1_score,
                    tier2_score: Some(sem_match.confidence),
                    tier2_latency_ms,
                    cache_hit: false,
                };

                let suggestion = SelectorSuggestion {
                    selector: sem_match.selector,
                    score: sem_match.confidence as f64,
                };

                let status = if sem_match.confidence >= self.options.semantic_threshold {
                    RepairStatus::Repaired
                } else {
                    RepairStatus::Degraded
                };

                let outcome = AdaptiveRepairOutcome {
                    suggestion,
                    status,
                    trace: Some(trace),
                };
                self.cache_insert(cache_key, outcome.clone());
                info!(
                    new_selector = %outcome.suggestion.selector,
                    final_score = sem_match.confidence,
                    method = "tier2_semantic",
                    "repair_resolved"
                );
                Ok(outcome)
            },
            Ok(None) => {
                // Tier 2 found nothing — use Tier 1 best or fail
                self.handle_tier1_only(best_tier1, tier1_score, structural_hash, cache_key)
                    .await
            },
            Err(e) => {
                warn!(error = ?e, "tier2 inference failed");
                self.handle_tier1_only(best_tier1, tier1_score, structural_hash, cache_key)
                    .await
            },
        }
    }

    /// Handle the case where only Tier 1 is available or Tier 2 failed.
    async fn handle_tier1_only(
        &self,
        best_tier1: Option<SelectorSuggestion>,
        tier1_score: f64,
        structural_hash: u64,
        cache_key: u64,
    ) -> Result<AdaptiveRepairOutcome, SelectorErrorKind> {
        if let Some(best) = best_tier1 {
            let trace = CascadeTrace {
                tier1_score,
                tier2_score: None,
                tier2_latency_ms: 0,
                cache_hit: false,
            };
            let outcome = AdaptiveRepairOutcome::repaired(best, trace);
            self.cache_insert(cache_key, outcome.clone());
            info!(new_selector = %outcome.suggestion.selector, method = "tier1_fallback", "repair_resolved");
            Ok(outcome)
        } else {
            let diagnostic = RepairFailureDiagnostic {
                tier1_best: None,
                tier2_best: None,
                candidates_evaluated: 0,
                structural_hash,
            };
            warn!("repair_failed");
            Err(SelectorErrorKind::RepairInconclusive(Box::new(diagnostic)))
        }
    }

    /// Extract text fragments from DOM for semantic comparison.
    fn extract_dom_fragments(&self, document: &scraper::Html) -> Vec<String> {
        let report = self.inspector.inspect(document);
        report
            .common_classes
            .iter()
            .map(|(class, _)| class.clone())
            .chain(report.common_ids.iter().map(|(id, _)| id.clone()))
            .collect()
    }

    /// Compute cache key from selector + structural hash.
    fn cache_key(&self, selector: &str, structural_hash: u64) -> u64 {
        use std::hash::{Hash, Hasher};

        let mut hasher = FnvHasher::default();
        selector.hash(&mut hasher);
        structural_hash.hash(&mut hasher);
        hasher.finish()
    }

    /// Insert into cache with lazy eviction when at capacity.
    fn cache_insert(&self, key: u64, outcome: AdaptiveRepairOutcome) {
        // Lazy eviction: if at capacity, remove expired entries
        if self.cache.len() >= self.options.max_cache_entries {
            let now = Instant::now();
            self.cache.retain(|_, entry| entry.expires_at > now);
        }

        self.cache.insert(
            key,
            CachedEntry {
                outcome,
                expires_at: Instant::now() + self.options.cache_ttl,
            },
        );
    }

    /// Get the number of cached entries (for monitoring).
    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

/// Minimal FNV-1a hasher for deterministic cache key computation.
#[derive(Default)]
struct FnvHasher(u64);

impl std::hash::Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        self.0 = h;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dom_inspector::DomStructureReport;

    /// Mock inspector for testing — returns configurable suggestions.
    struct MockInspector {
        suggestions: Vec<SelectorSuggestion>,
    }

    impl DomInspectorPort for MockInspector {
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

    #[tokio::test]
    async fn test_tier1_high_score_returns_immediately() {
        let inspector = Arc::new(MockInspector {
            suggestions: vec![SelectorSuggestion {
                selector: ".main-content".to_owned(),
                score: 0.92,
            }],
        });

        let engine =
            AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
        let html = "<div class='main-content'>Hello</div>";
        let document = scraper::Html::parse_document(html);

        let result = engine.repair(&document, html, ".content", None).await;
        assert!(result.is_ok());
        let outcome = result.unwrap();
        assert_eq!(outcome.status, RepairStatus::Repaired);
        assert_eq!(outcome.suggestion.selector, ".main-content");
        assert!(outcome.trace.as_ref().unwrap().tier2_score.is_none());
    }

    #[tokio::test]
    async fn test_tier1_low_score_no_semantic_returns_tier1_best() {
        let inspector = Arc::new(MockInspector {
            suggestions: vec![SelectorSuggestion {
                selector: ".similar".to_owned(),
                score: 0.65,
            }],
        });

        let engine =
            AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
        let html = "<div class='similar'>Hello</div>";
        let document = scraper::Html::parse_document(html);

        let result = engine.repair(&document, html, ".content", None).await;
        assert!(result.is_ok());
        let outcome = result.unwrap();
        assert_eq!(outcome.status, RepairStatus::Repaired);
        assert_eq!(outcome.suggestion.selector, ".similar");
    }

    #[tokio::test]
    async fn test_no_suggestions_returns_error() {
        let inspector = Arc::new(MockInspector {
            suggestions: vec![],
        });

        let engine =
            AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
        let html = "<div>Hello</div>";
        let document = scraper::Html::parse_document(html);

        let result = engine.repair(&document, html, ".content", None).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SelectorErrorKind::RepairInconclusive(_) => {},
            other => panic!("expected RepairInconclusive, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cache_hit_on_second_call() {
        let inspector = Arc::new(MockInspector {
            suggestions: vec![SelectorSuggestion {
                selector: ".cached".to_owned(),
                score: 0.90,
            }],
        });

        let engine =
            AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
        let html = "<div class='cached'>Hello</div>";
        let document = scraper::Html::parse_document(html);

        // Debug: compute hash twice to verify determinism
        let h1 = engine.structural_hash(&document);
        let h2 = engine.structural_hash(&document);
        assert_eq!(h1, h2, "structural_hash must be deterministic");
        let k1 = engine.cache_key(".target", h1);
        let k2 = engine.cache_key(".target", h2);
        assert_eq!(k1, k2, "cache_key must be deterministic");

        // First call — populates cache
        let r1 = engine
            .repair(&document, html, ".target", None)
            .await
            .unwrap();
        assert!(!r1.trace.as_ref().unwrap().cache_hit);
        assert_eq!(
            engine.cache_len(),
            1,
            "cache should have 1 entry after first call"
        );

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
}

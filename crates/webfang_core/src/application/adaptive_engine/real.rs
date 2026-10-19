//! Real implementation of the adaptive selector engine.
//!
//! This module contains the full implementation gated behind the
//! `adaptive-selectors` feature. It is re-exported by the parent module
//! when the feature is enabled.

use std::sync::Arc;
use std::time::{Duration, Instant};
use std::hash::{Hash, Hasher};

use dashmap::DashMap;
use tokio::sync::Semaphore;
use tracing::{debug, event, info, instrument, warn};

use crate::domain::dom_inspector::{
    DomInspectorPort, RepairFailureDiagnostic, SelectorErrorKind, SelectorSuggestion,
};
use crate::domain::semantic_inspector::{SemanticContext, SemanticInspectorPort, SemanticMatch};

use super::cache::{CachedEntry, FnvHasher};

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
        // `max(1)`: a zero-permit inference semaphore parks every adaptive
        // repair forever, and `max_concurrent_inference` is a plain `usize`
        // that deserializes from config, so 0 is representable (#1616 CC-L2).
        let semaphore = Arc::new(Semaphore::new(options.max_concurrent_inference.max(1)));
        Self {
            inspector,
            semantic,
            cache: DashMap::new(),
            inference_semaphore: semaphore,
            options,
        }
    }

    /// Whether a semantic (Tier 2) provider is wired into this engine.
    ///
    /// `false` means the engine degrades to Tier 1 (lexical) repair only —
    /// the expected state when the `ai` feature is off or no ONNX assets are
    /// available (`shared=None`).
    #[must_use]
    pub fn has_semantic_provider(&self) -> bool {
        self.semantic.is_some()
    }

    /// Sync-aware repair entry point — accepts raw HTML string (Send + Sync)
    /// instead of `&scraper::Html` (!Sync).
    ///
    /// Tier 1 (lexical) runs inside `spawn_blocking` to isolate the `!Sync`
    /// `scraper::Html` from the async runtime. Tier 2 (semantic) runs async
    /// on fragments extracted from the DOM.
    ///
    /// This is the recommended entry point for integration into async pipelines
    /// like `scrape_with_config` where `#[instrument]` requires `Sync`.
    ///
    /// # Errors
    ///
    /// Returns [`SelectorErrorKind`] if the repair cascade fails completely.
    #[instrument(skip(self), fields(failed_selector = %selector))]
    pub async fn select_sync_aware(
        &self,
        html_raw: String,
        selector: String,
        domain: Option<String>,
    ) -> Result<AdaptiveRepairOutcome, SelectorErrorKind> {
        // 0. Emit cascade_started event for observability
        event!(tracing::Level::DEBUG, event = "cascade_started", selector = %selector);

        // 1. Compute structural hash via spawn_blocking (HTML parsing is !Sync)
        let inspector = Arc::clone(&self.inspector);
        let selector_clone = selector.clone();
        let html_for_hash = html_raw.clone();

        let structural_hash = tokio::task::spawn_blocking(move || {
            let document = scraper::Html::parse_document(&html_for_hash);
            let report = inspector.inspect(&document);
            // Deterministic hash from sorted tag counts
            let mut tags: Vec<_> = report.tag_counts.iter().collect();
            tags.sort_by_key(|(tag, _)| tag.as_str());
            let mut combined = String::new();
            for (tag, count) in &tags {
                combined.push_str(tag);
                combined.push(':');
                combined.push_str(&count.to_string());
                combined.push(',');
            }
            use std::hash::{Hash, Hasher};
            let mut hasher = FnvHasher::default();
            combined.hash(&mut hasher);
            hasher.finish()
        })
        .await
        .map_err(|e| {
            SelectorErrorKind::BlockingTaskFailed(format!("tier1 structural hash task failed: {e}"))
        })?;

        // 2. Tier 1: lexical via spawn_blocking (HTML parsing is !Sync)
        let inspector = Arc::clone(&self.inspector);
        let html_for_t1 = html_raw.clone();

        let t1_suggestions = tokio::task::spawn_blocking(move || {
            let document = scraper::Html::parse_document(&html_for_t1);
            inspector.suggest(&document, &selector_clone)
        })
        .await
        .map_err(|e| {
            SelectorErrorKind::BlockingTaskFailed(format!("tier1 suggestion task failed: {e}"))
        })?;

        let best_tier1 = t1_suggestions.into_iter().max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let tier1_score = best_tier1.as_ref().map(|s| s.score).unwrap_or(0.0);

        // 3. Extract DOM fragments via spawn_blocking (HTML parsing is !Sync)
        let inspector = Arc::clone(&self.inspector);
        let html_for_fragments = html_raw.clone();

        let dom_fragments = tokio::task::spawn_blocking(move || {
            let document = scraper::Html::parse_document(&html_for_fragments);
            let report = inspector.inspect(&document);
            report
                .common_classes
                .iter()
                .map(|(class, _)| class.clone())
                .chain(report.common_ids.iter().map(|(id, _)| id.clone()))
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| {
            SelectorErrorKind::BlockingTaskFailed(format!(
                "dom fragment extraction task failed: {e}"
            ))
        })?;

        // 4. Run the shared cascade
        let ctx = SemanticContext {
            target_text: html_raw,
            dom_fragments,
            domain_hint: domain,
        };
        self.cascade(&selector, structural_hash, best_tier1, tier1_score, ctx)
            .await
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

    /// Run the full cascade: Tier 1 → (optional) Tier 2.
    async fn cascade(
        &self,
        selector: &str,
        structural_hash: u64,
        best_tier1: Option<SelectorSuggestion>,
        tier1_score: f64,
        ctx: SemanticContext,
    ) -> Result<AdaptiveRepairOutcome, SelectorErrorKind> {
        // Check cache first
        let cache_key = self.cache_key(selector, structural_hash);
        if let Some(cached) = self.cache_get(cache_key) {
            let mut outcome = cached;
            outcome.trace.as_mut().map(|t| t.cache_hit = true);
            return Ok(outcome);
        }

        // Fast path: high confidence Tier 1 → return immediately
        if let Some(ref best) = best_tier1 {
            if best.score > self.options.lexical_threshold {
                let trace = CascadeTrace {
                    tier1_score,
                    tier2_score: None,
                    tier2_latency_ms: 0,
                    cache_hit: false,
                };
                let outcome = AdaptiveRepairOutcome::repaired(best.clone(), trace);
                self.cache_insert(cache_key, outcome.clone());
                return Ok(outcome);
            }
        }

        // Tier 2: semantic (if available)
        let semantic = match &self.semantic {
            Some(s) => s,
            None => {
                // No semantic provider — fall back to Tier 1 best
                let trace = CascadeTrace {
                    tier1_score,
                    tier2_score: None,
                    tier2_latency_ms: 0,
                    cache_hit: false,
                };
                let outcome = if let Some(best) = best_tier1 {
                    AdaptiveRepairOutcome::repaired(best.clone(), trace)
                } else {
                    // No suggestions at all
                    let trace = CascadeTrace {
                        tier1_score: 0.0,
                        tier2_score: None,
                        tier2_latency_ms: 0,
                        cache_hit: false,
                    };
return Err(SelectorErrorKind::RepairInconclusive(
                        Box::new(RepairFailureDiagnostic {
                            tier1_best: None,
                            tier2_best: None,
                            candidates_evaluated: 0,
                            structural_hash,
                        }),
                    ));
                };
                self.cache_insert(cache_key, outcome.clone());
                return Ok(outcome);
            },
        };

        // Check Tier 2 availability — skip if inference pool is saturated
        let Some(_permit) = self.try_acquire_tier2().await else {
            // Pool saturated — fall back to Tier 1 best
            let trace = CascadeTrace {
                tier1_score,
                tier2_score: None,
                tier2_latency_ms: 0,
                cache_hit: false,
            };
            let outcome = if let Some(best) = best_tier1 {
                AdaptiveRepairOutcome::repaired(best.clone(), trace)
            } else {
return Err(SelectorErrorKind::RepairInconclusive(
                        Box::new(RepairFailureDiagnostic {
                            tier1_best: None,
                            tier2_best: None,
                            candidates_evaluated: 0,
                            structural_hash,
                        }),
                    ));
            };
            self.cache_insert(cache_key, outcome.clone());
            return Ok(outcome);
        };

        // Run Tier 2 on the precomputed semantic context
        let tier2_start = Instant::now();
        let tier2_result = semantic.find_semantic_match(ctx).await;
        let tier2_latency_ms = tier2_start.elapsed().as_millis() as u64;

        match tier2_result {
            Ok(Some(sem_match)) => {
                let tier2_score = sem_match.confidence;
                let suggestion = SelectorSuggestion {
                    selector: sem_match.selector,
                    score: tier2_score as f64,
                };
                let trace = CascadeTrace {
                    tier1_score,
                    tier2_score: Some(tier2_score),
                    tier2_latency_ms,
                    cache_hit: false,
                };
                let outcome = if tier2_score >= self.options.semantic_threshold {
                    AdaptiveRepairOutcome::repaired(suggestion, trace)
                } else {
                    AdaptiveRepairOutcome {
                        suggestion,
                        status: RepairStatus::Degraded,
                        trace: Some(trace),
                    }
                };
                self.cache_insert(cache_key, outcome.clone());
                Ok(outcome)
            },
            Ok(None) => {
                // Tier 2 returned no match — fall back to Tier 1
                let trace = CascadeTrace {
                    tier1_score,
                    tier2_score: None,
                    tier2_latency_ms,
                    cache_hit: false,
                };
                let outcome = if let Some(best) = best_tier1 {
                    AdaptiveRepairOutcome::repaired(best.clone(), trace)
                } else {
                    return Err(SelectorErrorKind::RepairInconclusive(
                        Box::new(RepairFailureDiagnostic {
                            tier1_best: None,
                            tier2_best: None,
                            candidates_evaluated: 0,
                            structural_hash,
                        }),
                    ));
                };
                self.cache_insert(cache_key, outcome.clone());
                Ok(outcome)
            },
            Err(e) => {
                warn!(error = ?e, "tier2 inference failed");
                // Tier 2 failed — fall back to Tier 1
                let trace = CascadeTrace {
                    tier1_score,
                    tier2_score: None,
                    tier2_latency_ms,
                    cache_hit: false,
                };
                let outcome = if let Some(best) = best_tier1 {
                    AdaptiveRepairOutcome::repaired(best.clone(), trace)
                } else {
                    return Err(SelectorErrorKind::RepairInconclusive(
                        Box::new(RepairFailureDiagnostic {
                            tier1_best: None,
                            tier2_best: None,
                            candidates_evaluated: 0,
                            structural_hash,
                        }),
                    ));
                };
                self.cache_insert(cache_key, outcome.clone());
                Ok(outcome)
            }
        }
    }

    /// Try to acquire a Tier 2 inference permit.
    ///
    /// Returns `None` if the semaphore is saturated (all permits in use).
    async fn try_acquire_tier2(&self) -> Option<tokio::sync::SemaphorePermit<'_>> {
        self.inference_semaphore.try_acquire().ok()
    }

    /// Compute cache key from selector + structural hash.
    fn cache_key(&self, selector: &str, structural_hash: u64) -> u64 {
        let mut hasher = FnvHasher::default();
        selector.hash(&mut hasher);
        structural_hash.hash(&mut hasher);
        hasher.finish()
    }

    /// Fetch a non-expired cached outcome, if present.
    ///
    /// Returns a clone of the stored outcome without mutating its trace; callers
    /// decide whether to flag the result as a cache hit.
    fn cache_get(&self, key: u64) -> Option<AdaptiveRepairOutcome> {
        let entry = self.cache.get(&key)?;
        if entry.expires_at > Instant::now() {
            Some(entry.outcome.clone())
        } else {
            None
        }
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
    #[allow(dead_code)]
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dom_inspector::SelectorSuggestion;
    use crate::domain::semantic_inspector::{SemanticContext, SemanticMatch, SemanticInspectorPort};
    use std::sync::Arc;

    struct MockInspector {
        suggestions: Vec<SelectorSuggestion>,
    }

    impl MockInspector {
        fn new(suggestions: Vec<SelectorSuggestion>) -> Self {
            Self { suggestions }
        }
    }

    impl DomInspectorPort for MockInspector {
        fn inspect(&self, _document: &scraper::Html) -> crate::domain::dom_inspector::DomStructureReport {
            crate::domain::dom_inspector::DomStructureReport::default()
        }

        fn suggest(
            &self,
            _document: &scraper::Html,
            _failed_selector: &str,
        ) -> Vec<SelectorSuggestion> {
            self.suggestions.clone()
        }
    }

    struct MockSemanticNoMatch;

    impl SemanticInspectorPort for MockSemanticNoMatch {
        fn find_semantic_match<'a>(
            &'a self,
            _ctx: SemanticContext,
        ) -> crate::domain::semantic_inspector::BoxFuture<
            'a,
            Result<Option<SemanticMatch>, SelectorErrorKind>,
        > {
            Box::pin(async { Ok(None) })
        }
    }

    #[tokio::test]
    async fn test_select_sync_aware_degraded_when_tier2_returns_none() {
        // Tier 1 returns low score (0.65 < 0.70 threshold), Tier 2 returns None
        // → should return Degraded with Tier 1 best suggestion
        let inspector = Arc::new(MockInspector {
            suggestions: vec![SelectorSuggestion {
                selector: ".low-score".to_owned(),
                score: 0.65,
            }],
        });
        let semantic = Arc::new(crate::domain::semantic_inspector::MockSemanticNoMatch);

        let engine = crate::application::adaptive_engine::real::AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            crate::application::adaptive_engine::real::AdaptiveSelectorOptions::default(),
        );
        let html = "<div>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".missing".to_owned(), None)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::domain::dom_inspector::SelectorErrorKind::RepairInconclusive(_) => {},
            other => panic!("expected RepairInconclusive, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_select_sync_aware_degraded_when_no_tier1_and_tier2_returns_none() {
        // Tier 1 returns empty, Tier 2 returns None → RepairInconclusive error
        let inspector = Arc::new(crate::domain::dom_inspector::MockInspector {
            suggestions: vec![],
        });
        let semantic = Arc::new(crate::domain::semantic_inspector::MockSemanticNoMatch);

        let engine = crate::application::adaptive_engine::real::AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            crate::application::adaptive_engine::real::AdaptiveSelectorOptions::default(),
        );
        let html = "<div>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".missing".to_owned(), None)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::domain::dom_inspector::SelectorErrorKind::RepairInconclusive(_) => {},
            other => panic!("expected RepairInconclusive, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_select_sync_aware_tier1_high_score_returns_immediately() {
        // Tier 1 returns high score (0.9 > 0.85 threshold) → immediate return
        let inspector = Arc::new(crate::domain::dom_inspector::MockInspector {
            suggestions: vec![crate::domain::dom_inspector::SelectorSuggestion {
                selector: ".high-score".to_owned(),
                score: 0.9,
            }],
        });
        let semantic = Arc::new(crate::domain::semantic_inspector::MockSemanticNoMatch);

        let engine = crate::application::adaptive_engine::real::AdaptiveSelectorEngine::new(
            inspector,
            Some(Arc::new(crate::domain::semantic_inspector::MockSemanticNoMatch)),
            crate::application::adaptive_engine::real::AdaptiveSelectorOptions::default(),
        );
        let html = "<div>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".content".to_owned(), None)
            .await;
        assert!(result.is_ok());
        let outcome = result.unwrap();
        assert_eq!(outcome.status, crate::application::adaptive_engine::real::RepairStatus::Repaired);
        assert_eq!(outcome.suggestion.selector, ".high-score");
    }

    /// Mock semantic inspector that returns a single match for `selector` at `confidence`.
    fn semantic_match(selector: &str, confidence: f32) -> Arc<crate::domain::semantic_inspector::MockSemanticMatch> {
        Arc::new(crate::domain::semantic_inspector::MockSemanticMatch(Some(crate::domain::semantic_inspector::SemanticMatch {
            selector: selector.to_owned(),
            confidence,
            source: crate::domain::semantic_inspector::TierSource::Semantic,
        })))
    }

    /// Tier 2 returns a confident match → repaired via
    /// method="tier2_semantic", trace carries the Tier 2 score.
    #[tokio::test]
    async fn test_repair_resolves_via_tier2_when_confidence_above_threshold() {
        // Tier 1 stays ambiguous (0.65 ≤ 0.85) so the cascade escalates to Tier 2.
        let inspector = ambiguous_tier1();
        let semantic = semantic_match(".semantic-hit", 0.9); // above semantic_threshold (0.75)

        let engine = crate::application::adaptive_engine::real::AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            crate::application::adaptive_engine::real::AdaptiveSelectorOptions::default(),
        );
        let html = "<div class='semantic-hit'>Hello</div>";
        let document = scraper::Html::parse_document(html);

        let result = engine.repair(&document, html, ".content", None).await;
        assert!(result.is_ok(), "tier2 repair should succeed");
        let outcome = result.unwrap();
        assert_eq!(outcome.status, crate::application::adaptive_engine::real::RepairStatus::Repaired);
        assert_eq!(outcome.suggestion.selector, ".semantic-hit");
        let trace = outcome
            .trace
            .as_ref()
            .expect("tier2 repair carries a trace");
        assert_eq!(
            trace.tier2_score,
            Some(0.9),
            "trace must record the tier2_semantic score"
        );
        assert!(!trace.cache_hit);
    }

    /// Tier 2 returns a low-confidence match → Degraded, but the Tier 2 score
    /// must still be visible in the trace (tier2_semantic ran).
    #[tokio::test]
    async fn test_repair_degraded_when_tier2_confidence_below_threshold() {
        let inspector = ambiguous_tier1();
        let semantic = semantic_match(".semantic-weak", 0.6); // below semantic_threshold (0.75)

        let engine = crate::application::adaptive_engine::real::AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            crate::application::adaptive_engine::real::AdaptiveSelectorOptions::default(),
        );
        let html = "<div class='semantic-weak'>Hello</div>";
        let document = scraper::Html::parse_document(html);

        let result = engine.repair(&document, html, ".content", None).await;
        assert!(result.is_ok(), "low-confidence tier2 still returns Ok");
        let outcome = result.unwrap();
        assert_eq!(
            outcome.status,
            crate::application::adaptive_engine::real::RepairStatus::Degraded,
            "confidence below 0.75 must mark the repair degraded"
        );
        assert_eq!(outcome.suggestion.selector, ".semantic-weak");
        assert_eq!(outcome.trace.as_ref().unwrap().tier2_score, Some(0.6));
    }

    #[test]
    fn has_semantic_provider_reflects_wiring() {
        let inspector = Arc::new(crate::domain::dom_inspector::MockInspector {
            suggestions: vec![],
        });

        let tier1_only = crate::application::adaptive_engine::real::AdaptiveSelectorEngine::new(
            inspector.clone(),
            None,
            crate::application::adaptive_engine::real::AdaptiveSelectorOptions::default(),
        );
        assert!(
            !tier1_only.has_semantic_provider(),
            "shared=None must leave a Tier-1-only engine"
        );

        let wired = crate::application::adaptive_engine::real::AdaptiveSelectorEngine::new(
            inspector,
            Some(Arc::new(crate::domain::semantic_inspector::MockSemanticNoMatch)),
            crate::application::adaptive_engine::real::AdaptiveSelectorOptions::default(),
        );
        assert!(
            wired.has_semantic_provider(),
            "a semantic provider must be visible on the engine"
        );
    }

    /// Mock semantic inspector returning a single match for `selector` at `confidence`.
    fn semantic_match(selector: &str, confidence: f32) -> Arc<crate::domain::semantic_inspector::MockSemanticMatch> {
        Arc::new(crate::domain::semantic_inspector::MockSemanticMatch(Some(crate::domain::semantic_inspector::SemanticMatch {
            selector: selector.to_owned(),
            confidence,
            source: crate::domain::semantic_inspector::TierSource::Semantic,
        })))
    }

    /// Ambiguous Tier 1 engine (score 0.65 ≤ 0.85) — the cascade must escalate
    /// to Tier 2 instead of settling for the lexical suggestion.
    fn ambiguous_tier1() -> Arc<crate::domain::dom_inspector::MockInspector> {
        Arc::new(crate::domain::dom_inspector::MockInspector {
            suggestions: vec![SelectorSuggestion {
                selector: ".ambiguous".to_owned(),
                score: 0.65,
            }],
        })
    }
}
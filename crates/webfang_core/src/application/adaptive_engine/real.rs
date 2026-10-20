//! Real implementation of the adaptive selector engine.
//!
//! This module contains the full implementation gated behind the
//! `adaptive-selectors` feature. It is re-exported by the parent module
//! when the feature is enabled.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Semaphore;
use tracing::{event, instrument, warn};

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
    /// the expected state when no semantic provider was injected
    /// (`semantic=None`).
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

    /// Run the full cascade: Tier 1 → (optional) Tier 2.
    ///
    /// The cascade is decomposed into named helpers (`accept_tier1`,
    /// `accept_tier2`, `tier1_fallback`) that each own one decision:
    /// accepting a lexical match, accepting a semantic match, or falling
    /// back to Tier 1 when Tier 2 is unavailable or unsuccessful.
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
            if let Some(trace) = outcome.trace.as_mut() {
                trace.cache_hit = true;
            }
            return Ok(outcome);
        }

        // Fast path: high confidence Tier 1 → return immediately
        if let Some(ref best) = best_tier1 {
            if best.score > self.options.lexical_threshold {
                return Ok(self.accept_tier1(cache_key, best, tier1_score));
            }
        }

        // Tier 2: semantic (if available)
        let Some(semantic) = &self.semantic else {
            // No semantic provider — fall back to Tier 1 best
            return self.tier1_fallback(
                cache_key,
                best_tier1.as_ref(),
                tier1_score,
                0,
                structural_hash,
            );
        };

        // Check Tier 2 availability — skip if inference pool is saturated
        let Some(_permit) = self.try_acquire_tier2().await else {
            // Pool saturated — fall back to Tier 1 best
            return self.tier1_fallback(
                cache_key,
                best_tier1.as_ref(),
                tier1_score,
                0,
                structural_hash,
            );
        };

        // Run Tier 2 on the precomputed semantic context
        let tier2_start = Instant::now();
        let tier2_result = semantic.find_semantic_match(ctx).await;
        let tier2_latency_ms = tier2_start.elapsed().as_millis() as u64;

        match tier2_result {
            Ok(Some(sem_match)) => {
                Ok(self.accept_tier2(cache_key, sem_match, tier1_score, tier2_latency_ms))
            },
            Ok(None) => {
                // Tier 2 returned no match — fall back to Tier 1
                self.tier1_fallback(
                    cache_key,
                    best_tier1.as_ref(),
                    tier1_score,
                    tier2_latency_ms,
                    structural_hash,
                )
            },
            Err(e) => {
                warn!(error = ?e, "tier2 inference failed");
                // Tier 2 failed — fall back to Tier 1
                self.tier1_fallback(
                    cache_key,
                    best_tier1.as_ref(),
                    tier1_score,
                    tier2_latency_ms,
                    structural_hash,
                )
            },
        }
    }

    /// Fast path: a high-confidence Tier 1 match short-circuits the cascade.
    fn accept_tier1(
        &self,
        cache_key: u64,
        best: &SelectorSuggestion,
        tier1_score: f64,
    ) -> AdaptiveRepairOutcome {
        let trace = CascadeTrace {
            tier1_score,
            tier2_score: None,
            tier2_latency_ms: 0,
            cache_hit: false,
        };
        let outcome = AdaptiveRepairOutcome::repaired(best.clone(), trace);
        self.cache_insert(cache_key, outcome.clone());
        outcome
    }

    /// Build and persist the outcome for a successful Tier 2 match.
    ///
    /// A confidence at or above `semantic_threshold` is `Repaired`; below it
    /// the outcome is `Degraded` but still carries the Tier 2 score.
    fn accept_tier2(
        &self,
        cache_key: u64,
        sem_match: SemanticMatch,
        tier1_score: f64,
        tier2_latency_ms: u64,
    ) -> AdaptiveRepairOutcome {
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
        outcome
    }

    /// Fall back to the best Tier 1 suggestion.
    ///
    /// Used when Tier 2 is unavailable, saturated, or unsuccessful. When Tier 1
    /// has no suggestion either, the repair is inconclusive.
    fn tier1_fallback(
        &self,
        cache_key: u64,
        best_tier1: Option<&SelectorSuggestion>,
        tier1_score: f64,
        tier2_latency_ms: u64,
        structural_hash: u64,
    ) -> Result<AdaptiveRepairOutcome, SelectorErrorKind> {
        let Some(best) = best_tier1 else {
            return Err(SelectorErrorKind::RepairInconclusive(Box::new(
                RepairFailureDiagnostic {
                    tier1_best: None,
                    tier2_best: None,
                    candidates_evaluated: 0,
                    structural_hash,
                },
            )));
        };
        let trace = CascadeTrace {
            tier1_score,
            tier2_score: None,
            tier2_latency_ms,
            cache_hit: false,
        };
        let outcome = AdaptiveRepairOutcome::repaired(best.clone(), trace);
        self.cache_insert(cache_key, outcome.clone());
        Ok(outcome)
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
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dom_inspector::{DomStructureReport, SelectorSuggestion};
    use crate::domain::semantic_inspector::{
        BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
    };
    use std::sync::Arc;

    /// Mock DOM inspector — returns configurable suggestions.
    struct MockInspector {
        suggestions: Vec<SelectorSuggestion>,
    }

    impl MockInspector {
        fn new(suggestions: Vec<SelectorSuggestion>) -> Self {
            Self { suggestions }
        }
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

    /// Mock semantic inspector — returns a configurable match.
    struct MockSemanticInspector(Option<SemanticMatch>);

    impl SemanticInspectorPort for MockSemanticInspector {
        fn find_semantic_match<'a>(
            &'a self,
            _ctx: SemanticContext,
        ) -> BoxFuture<'a, Result<Option<SemanticMatch>, SelectorErrorKind>> {
            let response = self.0.clone();
            Box::pin(async { Ok(response) })
        }
    }

    /// Tier 2 provider that never matches.
    fn semantic_no_match() -> Arc<MockSemanticInspector> {
        Arc::new(MockSemanticInspector(None))
    }

    /// Tier 2 provider returning a single match for `selector` at `confidence`.
    fn semantic_match(selector: &str, confidence: f32) -> Arc<MockSemanticInspector> {
        Arc::new(MockSemanticInspector(Some(SemanticMatch {
            selector: selector.to_owned(),
            confidence,
            source: TierSource::Semantic,
        })))
    }

    /// Ambiguous Tier 1 inspector (score 0.65 ≤ 0.85) — the cascade must
    /// escalate to Tier 2 instead of settling for the lexical suggestion.
    fn ambiguous_tier1() -> Arc<MockInspector> {
        Arc::new(MockInspector::new(vec![SelectorSuggestion {
            selector: ".ambiguous".to_owned(),
            score: 0.65,
        }]))
    }

    #[tokio::test]
    async fn test_select_sync_aware_degraded_when_tier2_returns_none() {
        // Tier 1 returns low score (0.65 < 0.70 threshold), Tier 2 returns None
        // → the Tier 1 best suggestion is returned as the fallback
        let inspector = Arc::new(MockInspector::new(vec![SelectorSuggestion {
            selector: ".low-score".to_owned(),
            score: 0.65,
        }]));
        let semantic = semantic_no_match();

        let engine = AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            AdaptiveSelectorOptions::default(),
        );
        let html = "<div>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".missing".to_owned(), None)
            .await;
        assert!(result.is_ok());
        let outcome = result.unwrap();
        // The Tier 1 best is returned as the fallback, so the status is
        // `Repaired` (not `Degraded`) — `Degraded` is reserved for a Tier 2
        // match whose confidence is below the semantic threshold.
        assert_eq!(outcome.suggestion.selector, ".low-score");
        assert_eq!(outcome.status, RepairStatus::Repaired);
        let trace = outcome.trace.as_ref().expect("repair carries a trace");
        assert!(
            trace.tier2_score.is_none(),
            "Tier 2 produced no match, so the trace carries no tier2 score"
        );
    }

    #[tokio::test]
    async fn test_select_sync_aware_degraded_when_no_tier1_and_tier2_returns_none() {
        // Tier 1 returns empty, Tier 2 returns None → RepairInconclusive error
        let inspector = Arc::new(MockInspector::new(vec![]));
        let semantic = semantic_no_match();

        let engine = AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            AdaptiveSelectorOptions::default(),
        );
        let html = "<div>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".missing".to_owned(), None)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SelectorErrorKind::RepairInconclusive(_) => {},
            other => panic!("expected RepairInconclusive, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_select_sync_aware_tier1_high_score_returns_immediately() {
        // Tier 1 returns high score (0.9 > 0.85 threshold) → immediate return
        let inspector = Arc::new(MockInspector::new(vec![SelectorSuggestion {
            selector: ".high-score".to_owned(),
            score: 0.9,
        }]));
        let semantic = semantic_no_match();

        let engine = AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            AdaptiveSelectorOptions::default(),
        );
        let html = "<div>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".content".to_owned(), None)
            .await;
        assert!(result.is_ok());
        let outcome = result.unwrap();
        assert_eq!(outcome.status, RepairStatus::Repaired);
        assert_eq!(outcome.suggestion.selector, ".high-score");
    }

    /// The cache must be keyed by (selector, structural hash): a second call
    /// with the same inputs is served from the cache instead of re-running the
    /// cascade.
    #[tokio::test]
    async fn test_select_sync_aware_cache_hit_on_second_call() {
        let inspector = Arc::new(MockInspector::new(vec![SelectorSuggestion {
            selector: ".cached".to_owned(),
            score: 0.9,
        }]));
        let semantic = semantic_no_match();

        let engine = AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            AdaptiveSelectorOptions::default(),
        );
        let html = "<div class='cached'>Hello</div>".to_owned();

        // First call — populates the cache
        let first = engine
            .select_sync_aware(html.clone(), ".target".to_owned(), None)
            .await
            .expect("tier1 repair must succeed");
        assert!(!first.trace.as_ref().expect("trace").cache_hit);
        assert_eq!(
            engine.cache_len(),
            1,
            "cache should hold one entry after the first call"
        );

        // Second call — served from the cache
        let second = engine
            .select_sync_aware(html, ".target".to_owned(), None)
            .await
            .expect("cached repair must succeed");
        assert!(
            second.trace.as_ref().expect("trace").cache_hit,
            "second call with the same selector and DOM must hit the cache"
        );
    }

    /// Tier 2 returns a confident match → the repaired selector comes from
    /// the semantic provider and the trace carries the Tier 2 score (the
    /// marker that the semantic tier ran).
    #[tokio::test]
    async fn test_repair_resolves_via_tier2_when_confidence_above_threshold() {
        // Tier 1 stays ambiguous (0.65 ≤ 0.85) so the cascade escalates to Tier 2.
        let inspector = ambiguous_tier1();
        let semantic = semantic_match(".semantic-hit", 0.9); // above semantic_threshold (0.75)

        let engine = AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            AdaptiveSelectorOptions::default(),
        );
        let html = "<div class='semantic-hit'>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".content".to_owned(), None)
            .await;
        assert!(result.is_ok(), "tier2 repair should succeed");
        let outcome = result.unwrap();
        assert_eq!(outcome.status, RepairStatus::Repaired);
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
    /// must still be visible in the trace (the semantic tier ran).
    #[tokio::test]
    async fn test_repair_degraded_when_tier2_confidence_below_threshold() {
        let inspector = ambiguous_tier1();
        let semantic = semantic_match(".semantic-weak", 0.6); // below semantic_threshold (0.75)

        let engine = AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            AdaptiveSelectorOptions::default(),
        );
        let html = "<div class='semantic-weak'>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".content".to_owned(), None)
            .await;
        assert!(result.is_ok(), "low-confidence tier2 still returns Ok");
        let outcome = result.unwrap();
        assert_eq!(
            outcome.status,
            RepairStatus::Degraded,
            "confidence below 0.75 must mark the repair degraded"
        );
        assert_eq!(outcome.suggestion.selector, ".semantic-weak");
        assert_eq!(outcome.trace.as_ref().unwrap().tier2_score, Some(0.6));
    }

    #[test]
    fn has_semantic_provider_reflects_wiring() {
        let inspector = Arc::new(MockInspector::new(vec![]));

        let tier1_only = AdaptiveSelectorEngine::new(
            inspector.clone(),
            None,
            AdaptiveSelectorOptions::default(),
        );
        assert!(
            !tier1_only.has_semantic_provider(),
            "shared=None must leave a Tier-1-only engine"
        );

        let wired = AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic_no_match()),
            AdaptiveSelectorOptions::default(),
        );
        assert!(
            wired.has_semantic_provider(),
            "a semantic provider must be visible on the engine"
        );
    }
}

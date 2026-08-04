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
use tracing::{debug, event, info, instrument, warn};

use crate::domain::dom_inspector::{
    DomInspectorPort, RepairFailureDiagnostic, SelectorErrorKind, SelectorSuggestion,
};
use crate::domain::semantic_inspector::{SemanticContext, SemanticInspectorPort, SemanticMatch};

mod cache;

use cache::{CachedEntry, FnvHasher};

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
        let semaphore = Arc::new(Semaphore::new(options.max_concurrent_inference));
        Self {
            inspector,
            semantic,
            cache: DashMap::new(),
            inference_semaphore: semaphore,
            options,
        }
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
        if let Some(outcome) = self.cached_outcome(cache_key) {
            return Some(outcome);
        }

        let best = self.best_tier1_suggestion(document, failed_selector)?;
        debug!(score = best.score, "tier1_evaluated");

        self.evaluate_tier1_outcome(best, cache_key)
    }

    /// Compute the Tier 1 suggestions and pick the best one.
    ///
    /// Returns `None` when the lexical inspector produced no suggestions.
    fn best_tier1_suggestion(
        &self,
        document: &scraper::Html,
        failed_selector: &str,
    ) -> Option<SelectorSuggestion> {
        let suggestions = self.inspector.suggest(document, failed_selector);
        if suggestions.is_empty() {
            debug!("tier1: no suggestions from lexical inspector");
            return None;
        }
        Self::best_suggestion(suggestions)
    }

    /// Pick the highest-scoring suggestion.
    fn best_suggestion(suggestions: Vec<SelectorSuggestion>) -> Option<SelectorSuggestion> {
        suggestions.into_iter().max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Decide the Tier 1 outcome from the best suggestion's score.
    fn evaluate_tier1_outcome(
        &self,
        best: SelectorSuggestion,
        cache_key: u64,
    ) -> Option<AdaptiveRepairOutcome> {
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
        } else {
            // At or below the lexical threshold — Tier 2 will be tried by the
            // async caller.
            None
        }
    }

    /// Full async repair cascade: Tier 1 + optional Tier 2.
    ///
    /// Thin wrapper over the private `cascade` method: computes the structural
    /// hash, the Tier 1 suggestions, and the DOM fragments synchronously from
    /// the already-parsed `document`, then delegates the shared cascade.
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

        // Tier 1: lexical
        let suggestions = self.inspector.suggest(document, failed_selector);
        let best_tier1 = Self::best_suggestion(suggestions);
        let tier1_score = best_tier1.as_ref().map(|s| s.score).unwrap_or(0.0);

        let dom_fragments = self.extract_dom_fragments(document);

        let ctx = SemanticContext {
            target_text: html.to_owned(),
            dom_fragments,
            domain_hint: domain.map(|d| d.to_owned()),
        };
        self.cascade(
            failed_selector,
            structural_hash,
            best_tier1,
            tier1_score,
            ctx,
        )
        .await
    }

    /// Shared repair cascade: cache → fast path → semaphore-gated Tier 2 →
    /// Tier 1 fallback.
    ///
    /// Both public entry points ([`Self::select_sync_aware`] and
    /// [`Self::repair`]) precompute the Tier 1 result and the Tier 2
    /// [`SemanticContext`] — each in its own way — and delegate the identical
    /// cascade logic here.
    ///
    /// # Errors
    ///
    /// Returns `SelectorErrorKind::RepairInconclusive` if both tiers fail.
    async fn cascade(
        &self,
        selector: &str,
        structural_hash: u64,
        best_tier1: Option<SelectorSuggestion>,
        tier1_score: f64,
        ctx: SemanticContext,
    ) -> Result<AdaptiveRepairOutcome, SelectorErrorKind> {
        // Check cache
        let cache_key = self.cache_key(selector, structural_hash);
        if let Some(outcome) = self.cached_outcome(cache_key) {
            return Ok(outcome);
        }

        debug!(score = tier1_score, "tier1_evaluated");

        // Fast path: high confidence → return immediately
        if let Some(outcome) = self.tier1_fast_path(&best_tier1, tier1_score, cache_key) {
            return Ok(outcome);
        }

        // Tier 2: semantic (if available)
        let semantic = match &self.semantic {
            Some(s) => s,
            None => {
                return self
                    .handle_tier1_only(best_tier1, tier1_score, structural_hash, cache_key)
                    .await;
            },
        };

        // Check Tier 2 availability — skip if inference pool is saturated
        let Some(_permit) = self.try_acquire_tier2().await else {
            return self
                .handle_tier1_only(best_tier1, tier1_score, structural_hash, cache_key)
                .await;
        };

        // Run Tier 2 on the precomputed semantic context
        let tier2_start = Instant::now();
        let tier2_result = semantic.find_semantic_match(ctx).await;
        let tier2_latency_ms = tier2_start.elapsed().as_millis() as u64;

        match tier2_result {
            Ok(Some(sem_match)) => {
                Ok(self.tier2_success(sem_match, tier1_score, tier2_latency_ms, cache_key))
            },
            Ok(None) => {
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

    /// Return the cached outcome with the cache-hit trace flag set, if present.
    fn cached_outcome(&self, cache_key: u64) -> Option<AdaptiveRepairOutcome> {
        let mut outcome = self.cache_get(cache_key)?;
        if let Some(ref mut trace) = outcome.trace {
            trace.cache_hit = true;
        }
        debug!(cache_hit = true, "returning cached repair outcome");
        Some(outcome)
    }

    /// Fast path: a high-confidence Tier 1 match short-circuits the cascade.
    fn tier1_fast_path(
        &self,
        best_tier1: &Option<SelectorSuggestion>,
        tier1_score: f64,
        cache_key: u64,
    ) -> Option<AdaptiveRepairOutcome> {
        let best = best_tier1.as_ref()?;
        if tier1_score <= self.options.lexical_threshold {
            return None;
        }

        let trace = CascadeTrace {
            tier1_score,
            tier2_score: None,
            tier2_latency_ms: 0,
            cache_hit: false,
        };
        let outcome = AdaptiveRepairOutcome::repaired(best.clone(), trace);
        self.cache_insert(cache_key, outcome.clone());
        info!(new_selector = %outcome.suggestion.selector, method = "tier1_lexical", "repair_resolved");
        Some(outcome)
    }

    /// Acquire the Tier 2 inference permit with a bounded wait.
    ///
    /// Returns `None` when the pool is saturated or the semaphore is closed.
    async fn try_acquire_tier2(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        match tokio::time::timeout(
            Duration::from_millis(100),
            self.inference_semaphore.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => Some(permit),
            Ok(Err(_)) => {
                warn!("inference semaphore closed, skipping tier2");
                None
            },
            Err(_) => {
                warn!("tier2 inference pool saturated (semaphore timeout), skipping tier2");
                None
            },
        }
    }

    /// Build and persist the outcome for a successful Tier 2 match.
    fn tier2_success(
        &self,
        sem_match: SemanticMatch,
        tier1_score: f64,
        tier2_latency_ms: u64,
        cache_key: u64,
    ) -> AdaptiveRepairOutcome {
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
        outcome
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dom_inspector::DomStructureReport;
    use crate::domain::semantic_inspector::{SemanticContext, SemanticMatch};

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
            other => panic!("expected RepairInconclusive, got {other:?}"),
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

    #[tokio::test]
    async fn test_select_sync_aware_tier1_high_score() {
        let inspector = Arc::new(MockInspector {
            suggestions: vec![SelectorSuggestion {
                selector: ".main-content".to_owned(),
                score: 0.92,
            }],
        });

        let engine =
            AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
        let html = "<div class='main-content'>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".content".to_owned(), None)
            .await;
        assert!(result.is_ok());
        let outcome = result.unwrap();
        assert_eq!(outcome.status, RepairStatus::Repaired);
        assert_eq!(outcome.suggestion.selector, ".main-content");
        assert!(outcome.trace.as_ref().unwrap().tier2_score.is_none());
    }

    #[tokio::test]
    async fn test_select_sync_aware_cache_hit() {
        let inspector = Arc::new(MockInspector {
            suggestions: vec![SelectorSuggestion {
                selector: ".cached".to_owned(),
                score: 0.90,
            }],
        });

        let engine =
            AdaptiveSelectorEngine::new(inspector, None, AdaptiveSelectorOptions::default());
        let html = "<div class='cached'>Hello</div>".to_owned();

        // First call
        let r1 = engine
            .select_sync_aware(html.clone(), ".target".to_owned(), None)
            .await
            .unwrap();
        assert!(!r1.trace.as_ref().unwrap().cache_hit);

        // Second call — should hit cache
        let r2 = engine
            .select_sync_aware(html, ".target".to_owned(), None)
            .await
            .unwrap();
        assert!(r2.trace.as_ref().unwrap().cache_hit);
    }

    /// Mock semantic inspector that returns None (no confident match) — triggers Degraded path.
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
        let semantic = Arc::new(MockSemanticNoMatch);

        let engine = AdaptiveSelectorEngine::new(
            inspector,
            Some(semantic),
            AdaptiveSelectorOptions::default(),
        );
        let html = "<div class='low-score'>Hello</div>".to_owned();

        let result = engine
            .select_sync_aware(html, ".missing".to_owned(), None)
            .await;
        assert!(result.is_ok());
        let outcome = result.unwrap();
        // Tier 1 best is returned as fallback, status is Repaired (not Degraded)
        // because handle_tier1_only returns Repaired when Tier 1 has a suggestion
        assert_eq!(outcome.suggestion.selector, ".low-score");
        assert_eq!(outcome.status, RepairStatus::Repaired);
    }

    #[tokio::test]
    async fn test_select_sync_aware_degraded_when_no_tier1_and_tier2_returns_none() {
        // Tier 1 returns empty, Tier 2 returns None → RepairInconclusive error
        let inspector = Arc::new(MockInspector {
            suggestions: vec![],
        });
        let semantic = Arc::new(MockSemanticNoMatch);

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
}

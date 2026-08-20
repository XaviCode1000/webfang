//! Pure structural scoring for extraction-quality hints (#792).
//!
//! Computes a 0..100 structural score over the last-resort extraction using
//! the cascade trace + extract result. Pure function, no I/O.
//!
//! This module is only compiled under the `adaptive-selectors` feature because
//! it depends on `CascadeTrace` from the adaptive engine.

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::CascadeTrace;
use crate::domain::dom_inspector::ExtractResult;
use crate::domain::extraction_quality::{ErrorHintConfig, StructuralScore};

/// Compute the structural score for a last-resort extraction.
///
/// Equal-weighted over the 3 active MVP factors (semantic_drift,
/// context_collapse, result_size). Pure — no I/O.
#[cfg(feature = "adaptive-selectors")]
#[must_use]
pub fn compute_structural_score(
    trace: &CascadeTrace,
    extract_result: &ExtractResult,
    cfg: &ErrorHintConfig,
) -> StructuralScore {
    let semantic_drift = semantic_drift_factor(trace, cfg);
    let context_collapse = context_collapse_factor(trace, cfg);
    let result_size = result_size_factor(extract_result, cfg);
    let total = (semantic_drift + context_collapse + result_size) / 3.0 * 100.0;
    StructuralScore {
        semantic_drift,
        context_collapse,
        result_size,
        total,
        active_factors: 3,
    }
}

/// Convenience: compute a quality hint from a cascade trace + extract result.
///
/// Encapsulates the full pipeline: trace → structural score → hint (if low).
/// Returns `None` when the score is above the threshold (healthy extraction).
#[cfg(feature = "adaptive-selectors")]
#[must_use]
pub fn compute_quality_hint(
    trace: &CascadeTrace,
    extract_result: &ExtractResult,
) -> Option<crate::domain::extraction_quality::ExtractionQualityHint> {
    let cfg = ErrorHintConfig::default();
    let score = compute_structural_score(trace, extract_result, &cfg);
    score.to_hint(&cfg)
}

/// Semantic drift factor: how close Tier 2 confidence is to the threshold.
/// `None` (Tier 2 skipped) is treated as neutral-healthy (1.0).
#[cfg(feature = "adaptive-selectors")]
fn semantic_drift_factor(trace: &CascadeTrace, cfg: &ErrorHintConfig) -> f64 {
    match trace.tier2_score {
        Some(t) => {
            let deficit = (f64::from(cfg.semantic_threshold) - f64::from(t)).max(0.0);
            1.0 - (deficit / f64::from(cfg.semantic_threshold)).min(1.0)
        },
        None => 1.0,
    }
}

/// Context collapse factor: how close Tier 1 lexical score is to the lower bound.
#[cfg(feature = "adaptive-selectors")]
fn context_collapse_factor(trace: &CascadeTrace, cfg: &ErrorHintConfig) -> f64 {
    let deficit = (cfg.lexical_escalation_lower_bound - trace.tier1_score).max(0.0);
    1.0 - (deficit / cfg.lexical_escalation_lower_bound).min(1.0)
}

/// Result size factor: how close the extracted HTML length is to the minimum.
#[cfg(feature = "adaptive-selectors")]
fn result_size_factor(extract_result: &ExtractResult, cfg: &ErrorHintConfig) -> f64 {
    let len = extract_result.as_html().len();
    let min = cfg.min_fallback_content;
    let deficit = min.saturating_sub(len);
    1.0 - (deficit as f64 / min as f64).min(1.0)
}

#[cfg(test)]
#[cfg(feature = "adaptive-selectors")]
mod tests {
    use super::*;
    use crate::application::adaptive_engine::CascadeTrace;
    use crate::domain::dom_inspector::ExtractResult;

    fn make_trace(tier1: f64, tier2: Option<f32>) -> CascadeTrace {
        CascadeTrace {
            tier1_score: tier1,
            tier2_score: tier2,
            tier2_latency_ms: 0,
            cache_hit: false,
        }
    }

    fn make_extract_result(len: usize) -> ExtractResult {
        ExtractResult::Fallback {
            html: "x".repeat(len),
            diagnostic: None,
        }
    }

    fn default_cfg() -> ErrorHintConfig {
        ErrorHintConfig::default()
    }

    #[test]
    fn score_semantic_drift_healthy_when_tier2_above_threshold() {
        let trace = make_trace(0.5, Some(0.80));
        let cfg = default_cfg(); // semantic_threshold = 0.75
        let factor = semantic_drift_factor(&trace, &cfg);
        assert!((factor - 1.0).abs() < 0.001);
    }

    #[test]
    fn score_semantic_drift_decreases_as_tier2_falls_below_threshold() {
        let trace = make_trace(0.5, Some(0.60));
        let cfg = default_cfg(); // semantic_threshold = 0.75
        let factor = semantic_drift_factor(&trace, &cfg);
        // deficit = 0.15, normalized = 0.15/0.75 = 0.2, factor = 0.8
        assert!((factor - 0.8).abs() < 0.001);
    }

    #[test]
    fn score_semantic_drift_neutral_when_tier2_none() {
        let trace = make_trace(0.5, None);
        let cfg = default_cfg();
        let factor = semantic_drift_factor(&trace, &cfg);
        assert!((factor - 1.0).abs() < 0.001);
    }

    #[test]
    fn score_context_collapse_healthy_when_tier1_above_lower_bound() {
        let trace = make_trace(0.80, None);
        let cfg = default_cfg(); // escalation_lower_bound = 0.70
        let factor = context_collapse_factor(&trace, &cfg);
        assert!((factor - 1.0).abs() < 0.001);
    }

    #[test]
    fn score_context_collapse_decreases_as_tier1_falls_below_lower_bound() {
        let trace = make_trace(0.55, None);
        let cfg = default_cfg(); // escalation_lower_bound = 0.70
        let factor = context_collapse_factor(&trace, &cfg);
        // deficit = 0.15, normalized = 0.15/0.70 ≈ 0.214, factor ≈ 0.786
        assert!((factor - 0.7857).abs() < 0.001);
    }

    #[test]
    fn score_result_size_healthy_when_content_above_min() {
        let result = make_extract_result(200);
        let cfg = default_cfg(); // min_fallback_content = 100
        let factor = result_size_factor(&result, &cfg);
        assert!((factor - 1.0).abs() < 0.001);
    }

    #[test]
    fn score_result_size_proportional_when_content_below_min() {
        let result = make_extract_result(50);
        let cfg = default_cfg(); // min_fallback_content = 100
        let factor = result_size_factor(&result, &cfg);
        assert!((factor - 0.5).abs() < 0.001);
    }

    #[test]
    fn score_result_size_zero_when_content_empty() {
        let result = make_extract_result(0);
        let cfg = default_cfg();
        let factor = result_size_factor(&result, &cfg);
        assert!((factor - 0.0).abs() < 0.001);
    }

    #[test]
    fn compute_structural_score_equal_weighted() {
        let trace = make_trace(0.55, Some(0.60));
        let result = make_extract_result(50);
        let cfg = default_cfg();
        let score = compute_structural_score(&trace, &result, &cfg);
        // semantic_drift = 0.8, context_collapse ≈ 0.7857, result_size = 0.5
        // total = (0.8 + 0.7857 + 0.5) / 3 * 100 ≈ 69.5
        assert!((score.total - 69.52).abs() < 0.1);
        assert_eq!(score.active_factors, 3);
    }

    #[test]
    fn compute_structural_score_low_total_when_all_factors_poor() {
        let trace = make_trace(0.30, Some(0.30));
        let result = make_extract_result(10);
        let cfg = default_cfg();
        let score = compute_structural_score(&trace, &result, &cfg);
        // Should be well below 40 threshold
        assert!(score.total < 40.0);
    }
}

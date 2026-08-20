//! Extraction quality scoring and honest error hints (#792).
//!
//! When the adaptive selector repair still fails or returns a low-quality
//! extraction, we compute a structural score over the last-resort extraction
//! and surface an honest, structured hint instead of a generic error.

use serde::{Deserialize, Serialize};

/// Configuration thresholds for extraction-quality hints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorHintConfig {
    /// Tier 2 confidence below which semantic drift is flagged.
    pub semantic_threshold: f32,
    /// Tier 1 score below which context collapse is flagged.
    pub lexical_escalation_lower_bound: f64,
    /// Minimum content chars for a healthy extraction.
    pub min_content: usize,
    /// Minimum fallback content bytes.
    pub min_fallback_content: usize,
    /// Structural score below which a hint is emitted (0..100).
    pub low_score_threshold: f64,
    /// Repeated-failure count that triggers anti-silent-failure (Slice B).
    pub fingerprint_failure_threshold: u32,
}

impl Default for ErrorHintConfig {
    fn default() -> Self {
        Self {
            semantic_threshold: 0.75,
            lexical_escalation_lower_bound: 0.70,
            min_content: 50,
            min_fallback_content: 100,
            low_score_threshold: 40.0,
            fingerprint_failure_threshold: 3,
        }
    }
}

/// Per-site fingerprint record (Slice B populates; None in Slice A).
///
/// Stored in the domain for Slice B's FingerprintRepository to persist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FingerprintRecord {
    /// Site base URL (e.g., `<https://example.com>`).
    pub site_base_url: String,
    /// Normalized failed selector signature.
    pub selector_signature: String,
    /// Structural score at failure time (0..100).
    pub score_at_failure: f64,
    /// Failure count for this signature on this site.
    pub failure_count: u32,
    /// Unix timestamp of last occurrence.
    pub last_seen: i64,
    /// Optional note from the last failure.
    pub last_note: Option<String>,
}

/// Structural score of a last-resort extraction (0..100, higher = healthier).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralScore {
    /// Semantic drift factor (0..1).
    pub semantic_drift: f64,
    /// Context collapse factor (0..1).
    pub context_collapse: f64,
    /// Result size factor (0..1).
    pub result_size: f64,
    /// Overall score (0..100), equal-weighted over active factors.
    pub total: f64,
    /// Number of active factors contributing to `total`.
    pub active_factors: u8,
}

impl StructuralScore {
    /// Build a hint when the total score is below the configured threshold.
    #[must_use]
    pub fn to_hint(&self, cfg: &ErrorHintConfig) -> Option<ExtractionQualityHint> {
        if self.total < cfg.low_score_threshold {
            Some(ExtractionQualityHint {
                score: self.clone(),
                message_es: self.message_es(),
                fingerprint: None,
            })
        } else {
            None
        }
    }

    fn message_es(&self) -> String {
        format!(
            "La extracción devolvió contenido de baja calidad (puntaje estructural {}/100): posible deriva semántica o contenido insuficiente.",
            self.total.round() as i64
        )
    }
}

/// Honest, structured hint emitted when extraction quality is low.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionQualityHint {
    /// The structural score that triggered the hint.
    pub score: StructuralScore,
    /// User-facing message in Spanish.
    pub message_es: String,
    /// Per-site fingerprint record (populated by Slice B; None in Slice A).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fingerprint: Option<FingerprintRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_score_to_hint_below_threshold_returns_some() {
        let cfg = ErrorHintConfig::default(); // low_score_threshold = 40.0
        let score = StructuralScore {
            semantic_drift: 0.3,
            context_collapse: 0.4,
            result_size: 0.5,
            total: 39.0,
            active_factors: 3,
        };
        let hint = score.to_hint(&cfg);
        assert!(hint.is_some());
        assert_eq!(hint.unwrap().score.total, 39.0);
    }

    #[test]
    fn structural_score_to_hint_at_threshold_returns_none() {
        let cfg = ErrorHintConfig::default(); // low_score_threshold = 40.0
        let score = StructuralScore {
            semantic_drift: 0.4,
            context_collapse: 0.4,
            result_size: 0.4,
            total: 40.0,
            active_factors: 3,
        };
        let hint = score.to_hint(&cfg);
        assert!(hint.is_none());
    }

    #[test]
    fn structural_score_to_hint_above_threshold_returns_none() {
        let cfg = ErrorHintConfig::default();
        let score = StructuralScore {
            semantic_drift: 0.5,
            context_collapse: 0.5,
            result_size: 0.5,
            total: 41.0,
            active_factors: 3,
        };
        let hint = score.to_hint(&cfg);
        assert!(hint.is_none());
    }

    #[test]
    fn error_hint_config_defaults_match_design() {
        let cfg = ErrorHintConfig::default();
        assert_eq!(cfg.semantic_threshold, 0.75);
        assert_eq!(cfg.lexical_escalation_lower_bound, 0.70);
        assert_eq!(cfg.min_content, 50);
        assert_eq!(cfg.min_fallback_content, 100);
        assert_eq!(cfg.low_score_threshold, 40.0);
        assert_eq!(cfg.fingerprint_failure_threshold, 3);
    }

    #[test]
    fn extraction_quality_hint_message_es_contains_score() {
        let cfg = ErrorHintConfig::default();
        let score = StructuralScore {
            semantic_drift: 0.2,
            context_collapse: 0.3,
            result_size: 0.4,
            total: 35.0,
            active_factors: 3,
        };
        let hint = score.to_hint(&cfg).expect("hint expected");
        assert!(hint.message_es.contains("35"));
        assert!(hint.message_es.contains("baja calidad"));
        assert!(hint.fingerprint.is_none());
    }
}

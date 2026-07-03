//! JavaScript rendering strategy — controls the fetch escalation path.
//!
//! Three strategies map to the three layers of the [`HybridRouter`]:
//!
//! - **Static** — wreq only (fast, no JS rendering)
//! - **Hybrid** — wreq → Obscura → Chromiumoxide (SPA-aware escalation)
//! - **Full** — Chromiumoxide only (always renders JS)
//!
//! [`HybridRouter`]: crate::infrastructure::downloader::hybrid_router::HybridRouter

use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// JavaScript rendering strategy for page fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum JsStrategy {
    /// Static HTTP only (wreq). Fastest, no JS rendering.
    Static,
    /// Hybrid 3-layer: wreq → Obscura → Chromiumoxide.
    Hybrid,
    /// Full JS rendering only (Chromiumoxide). Slowest, handles all SPAs.
    Full,
}

impl Default for JsStrategy {
    fn default() -> Self {
        Self::Static
    }
}

impl fmt::Display for JsStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static => write!(f, "static"),
            Self::Hybrid => write!(f, "hybrid"),
            Self::Full => write!(f, "full"),
        }
    }
}

impl std::str::FromStr for JsStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "static" => Ok(Self::Static),
            "hybrid" => Ok(Self::Hybrid),
            "full" => Ok(Self::Full),
            other => Err(format!(
                "unknown js strategy '{other}': expected static, hybrid, or full"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_js_strategy_default_is_static() {
        assert_eq!(JsStrategy::default(), JsStrategy::Static);
    }

    #[test]
    fn test_js_strategy_display() {
        assert_eq!(JsStrategy::Static.to_string(), "static");
        assert_eq!(JsStrategy::Hybrid.to_string(), "hybrid");
        assert_eq!(JsStrategy::Full.to_string(), "full");
    }

    #[test]
    fn test_js_strategy_from_str() {
        assert_eq!("static".parse::<JsStrategy>().unwrap(), JsStrategy::Static);
        assert_eq!("hybrid".parse::<JsStrategy>().unwrap(), JsStrategy::Hybrid);
        assert_eq!("full".parse::<JsStrategy>().unwrap(), JsStrategy::Full);
        assert_eq!("STATIC".parse::<JsStrategy>().unwrap(), JsStrategy::Static);
        assert!("invalid".parse::<JsStrategy>().is_err());
    }

    #[test]
    fn test_js_strategy_serde_roundtrip() {
        for strategy in [JsStrategy::Static, JsStrategy::Hybrid, JsStrategy::Full] {
            let json = serde_json::to_string(&strategy).unwrap();
            let deserialized: JsStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(strategy, deserialized);
        }
    }
}

//! Post-load settlement wait for the Chromium render path (F-52-b, #1277).
//!
//! `ChromiumoxideDownloader::fetch` used to call `page.content()` immediately
//! after navigation resolved, losing DOM mutations that arrive after the load
//! event (the fetch/XHR round-trip that defines an SPA). This type configures
//! the bounded wait inserted between navigation and capture:
//!
//! - **Idle** (default) — proceed once no network activity is observed for
//!   [`PostLoadWait::IDLE_WINDOW`], bounded by the fetch ceiling.
//! - **Fixed(ms)** — sleep `ms` after load (operator escape hatch).
//! - **None** — historical behavior: capture immediately.
//!
//! The wait is best-effort by contract: ceiling expiry or CDP-subscription
//! failure proceeds with the current DOM and logs, never fails the fetch.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Post-load settlement wait applied by the chromium render path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PostLoadWait {
    /// Wait for network-idle (quiet window [`PostLoadWait::IDLE_WINDOW`]),
    /// bounded by the fetch ceiling. Default.
    #[default]
    Idle,
    /// Fixed floor wait of N ms after load (1..=30_000).
    Fixed(u16),
    /// Historical behavior: capture immediately after navigation resolves.
    None,
}

impl PostLoadWait {
    /// Network-quiet window defining idle (F-52-b design decision Q2: global
    /// constant, deliberately NOT a CLI flag until trace evidence justifies
    /// per-site tuning).
    pub const IDLE_WINDOW: Duration = Duration::from_millis(500);

    /// Maximum fixed wait accepted from the operator (`--js-wait <ms>`).
    pub const MAX_FIXED_MS: u16 = 30_000;

    /// Whether this mode adds no wait (`None`).
    #[must_use]
    pub fn is_inert(&self) -> bool {
        matches!(self, Self::None)
    }
}

impl fmt::Display for PostLoadWait {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Fixed(ms) => write!(f, "{ms}"),
            Self::None => write!(f, "none"),
        }
    }
}

impl std::str::FromStr for PostLoadWait {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const VALID: &str = "expected `idle`, `none`, or a fixed wait in ms (1..=30000)";
        match s.to_lowercase().as_str() {
            "idle" => Ok(Self::Idle),
            "none" => Ok(Self::None),
            digits => digits.parse::<u16>().map_or_else(
                |_| Err(format!("invalid post-load wait '{s}': {VALID}",)),
                |ms| {
                    if (1..=Self::MAX_FIXED_MS).contains(&ms) {
                        Ok(Self::Fixed(ms))
                    } else {
                        Err(format!("invalid post-load wait '{s}': {VALID}",))
                    }
                },
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_idle() {
        assert_eq!(PostLoadWait::default(), PostLoadWait::Idle);
    }

    #[test]
    fn display_roundtrip() {
        assert_eq!(PostLoadWait::Idle.to_string(), "idle");
        assert_eq!(PostLoadWait::Fixed(400).to_string(), "400");
        assert_eq!(PostLoadWait::None.to_string(), "none");
    }

    #[test]
    fn from_str_keywords_case_insensitive() {
        assert_eq!("idle".parse::<PostLoadWait>().unwrap(), PostLoadWait::Idle);
        assert_eq!("IDLE".parse::<PostLoadWait>().unwrap(), PostLoadWait::Idle);
        assert_eq!("none".parse::<PostLoadWait>().unwrap(), PostLoadWait::None);
        assert_eq!("None".parse::<PostLoadWait>().unwrap(), PostLoadWait::None);
    }

    #[test]
    fn from_str_fixed_bounds() {
        assert_eq!("1".parse::<PostLoadWait>().unwrap(), PostLoadWait::Fixed(1));
        assert_eq!(
            "30000".parse::<PostLoadWait>().unwrap(),
            PostLoadWait::Fixed(30_000)
        );
        assert_eq!(
            "400".parse::<PostLoadWait>().unwrap(),
            PostLoadWait::Fixed(400)
        );
    }

    #[test]
    fn from_str_rejects_out_of_range_and_garbage() {
        for bad in ["0", "30001", "abc", "", "12ms", "-5", "1.5"] {
            let err = bad.parse::<PostLoadWait>().unwrap_err();
            assert!(
                err.contains("idle") && err.contains("none"),
                "error must name the valid forms, got: {err}"
            );
        }
    }

    #[test]
    fn is_inert_only_for_none() {
        assert!(!PostLoadWait::Idle.is_inert());
        assert!(!PostLoadWait::Fixed(100).is_inert());
        assert!(PostLoadWait::None.is_inert());
    }

    #[test]
    fn idle_window_is_500ms() {
        assert_eq!(PostLoadWait::IDLE_WINDOW, Duration::from_millis(500));
    }

    #[test]
    fn serde_roundtrip() {
        for mode in [
            PostLoadWait::Idle,
            PostLoadWait::Fixed(250),
            PostLoadWait::None,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let back: PostLoadWait = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, back);
        }
    }

    #[test]
    fn serde_idle_is_kebab() {
        assert_eq!(
            serde_json::to_string(&PostLoadWait::Idle).unwrap(),
            r#""idle""#
        );
    }
}

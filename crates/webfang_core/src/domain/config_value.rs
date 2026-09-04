//! Provenance-carrying configuration value (domain layer).
//!
//! Pairs a normalized configuration value with its [`ConfigSource`] so
//! pipeline stages can decide writes purely by source rank. Lives in
//! `domain` to keep the dependency direction intact: the core crate
//! depends on nothing internal.
//!
//! # Precedence
//!
//! [`ConfigSource`] variant declaration order **is** the precedence total
//! order. The enum derives `PartialOrd`/`Ord`, so `Default < ConfigFile <
//! Environment < Cli`. Reordering variants silently changes merge
//! semantics — the unit test `source_ordering_is_precedence` pins this.

// ---------------------------------------------------------------------------
// ConfigSource
// ---------------------------------------------------------------------------

/// Source of a configuration value, ordered by precedence.
///
/// **Variant order is precedence.** The enum derives `Ord`, so
/// `Default < ConfigFile < Environment < Cli`. Do not reorder
/// variants without updating the pipeline rank guards and the ordering
/// pin test. Higher-ranked sources strictly outrank lower-ranked ones
/// during normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigSource {
    /// Built-in default (lowest precedence).
    Default,
    /// Value loaded from the TOML config file.
    ConfigFile,
    /// Value supplied via a `WEBFANG_*` environment variable.
    Environment,
    /// Value supplied explicitly on the command line.
    Cli,
}

// ---------------------------------------------------------------------------
// ConfigValue<T>
// ---------------------------------------------------------------------------

/// A configuration value paired with its [`ConfigSource`] provenance.
///
/// The pipeline records one `ConfigValue` per contested field inside a
/// private `FieldBook`; stages write only when the incoming source
/// strictly outranks the recorded source (see [`ConfigValue::outranked_by`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValue<T> {
    /// The normalized value.
    pub value: T,
    /// Provenance of `value`.
    pub source: ConfigSource,
}

impl<T> ConfigValue<T> {
    /// Create a new provenance-carrying value.
    #[must_use]
    pub const fn new(value: T, source: ConfigSource) -> Self {
        Self { value, source }
    }

    /// Whether `incoming` strictly outranks the recorded source.
    ///
    /// A stage writes only when this returns `true`:
    /// `incoming > self.source` under the derived `Ord` of [`ConfigSource`].
    #[must_use]
    pub fn outranked_by(&self, incoming: ConfigSource) -> bool {
        incoming > self.source
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigSource, ConfigValue};

    #[test]
    fn source_ordering_is_precedence() {
        // Exhaustive 4-variant chain: derived Ord must yield
        // Default < ConfigFile < Environment < Cli
        assert!(
            ConfigSource::Default < ConfigSource::ConfigFile,
            "Default must be < ConfigFile"
        );
        assert!(
            ConfigSource::ConfigFile < ConfigSource::Environment,
            "ConfigFile must be < Environment"
        );
        assert!(
            ConfigSource::Environment < ConfigSource::Cli,
            "Environment must be < Cli"
        );

        // Pin the full sorted order equals declaration order.
        let mut variants = [
            ConfigSource::Cli,
            ConfigSource::Environment,
            ConfigSource::Default,
            ConfigSource::ConfigFile,
        ];
        variants.sort();
        assert_eq!(
            variants,
            [
                ConfigSource::Default,
                ConfigSource::ConfigFile,
                ConfigSource::Environment,
                ConfigSource::Cli,
            ]
        );
    }

    #[test]
    fn outranked_by_truth_table() {
        let variants = [
            ConfigSource::Default,
            ConfigSource::ConfigFile,
            ConfigSource::Environment,
            ConfigSource::Cli,
        ];
        for &current in &variants {
            for &incoming in &variants {
                let cv = ConfigValue::new((), current);
                let expected = incoming > current;
                assert_eq!(
                    cv.outranked_by(incoming),
                    expected,
                    "outranked_by mismatch: current={current:?} incoming={incoming:?} expected={expected}"
                );
            }
        }
    }

    #[test]
    fn config_value_new_accessors() {
        let cv = ConfigValue::new(42_u32, ConfigSource::Cli);
        assert_eq!(cv.value, 42_u32);
        assert_eq!(cv.source, ConfigSource::Cli);

        // Second type / source combination.
        let cv2 = ConfigValue::new("hello", ConfigSource::Cli);
        assert_eq!(cv2.value, "hello");
        assert_eq!(cv2.source, ConfigSource::Cli);
    }

    #[test]
    fn domain_isolation_no_internal_crate_deps() {
        // Production section of this file (before #[cfg(test)]) must not
        // reference internal crates. We split at the cfg marker so the
        // test's own literals do not self-trigger.
        let src = include_str!("config_value.rs");
        let prod_src = src.split("#[cfg(test)]").next().unwrap_or(src);
        for forbidden in [
            concat!("webfang", "_", "core"),
            concat!("webfang", "_", "ai"),
            concat!("webfang", "_", "tui"),
            concat!("webfang", "_", "cli"),
            concat!("webfang", "_", "mcp"),
            concat!("webfang", "_", "test_utils"),
        ] {
            assert!(
                !prod_src.contains(forbidden),
                "domain layer must not reference internal crate `{forbidden}`"
            );
        }
        if prod_src.contains("use crate::") {
            for line in prod_src.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("//") || trimmed.starts_with("//!") {
                    continue;
                }
                if trimmed.contains("use crate::") {
                    panic!("domain isolation: `use crate::` found in domain layer: {trimmed}");
                }
            }
        }
    }
}

//! Adaptive CSS selector repair engine — 2-tier cascade with caching.
//!
//! Tier 1 (lexical, Jaro-Winkler) runs synchronously via [`DomInspectorPort`].
//! Tier 2 (semantic, embeddings) runs asynchronously via [`SemanticInspectorPort`]
//! only when Tier 1 confidence is ambiguous (0.70–0.85). A [`DashMap`] cache
//! with TTL prevents redundant inference calls.
//!
//! When the `adaptive-selectors` feature is enabled, this module re-exports the
//! full implementation from the `real` submodule. When disabled, it provides a
//! zero-sized placeholder `AdaptiveSelectorEngine` that keeps public signatures
//! stable and nameable without leaking `()`.

#[cfg(feature = "adaptive-selectors")]
mod real;

/// The adaptive selector engine.
///
/// When the `adaptive-selectors` feature is enabled, this is the full
/// [`real::AdaptiveSelectorEngine`] implementation. When the feature is off,
/// this is a zero-sized placeholder that keeps public signatures stable
/// and nameable without leaking `()`.
///
/// This type is always available, so callers can write
/// `Option<&AdaptiveSelectorEngine>` in signatures without feature gates.
/// The real implementation is only available when the `adaptive-selectors`
/// feature is enabled; otherwise the type is a zero-sized placeholder that
/// implements `Default` so callers can write `AdaptiveSelectorEngine::default()`
/// in feature-agnostic code if needed.
#[cfg(feature = "adaptive-selectors")]
pub use real::AdaptiveSelectorEngine;

/// No-feature stand-in: keeps public signatures stable and nameable.
///
/// This zero-sized placeholder stands in for [`AdaptiveSelectorEngine`] when the
/// `adaptive-selectors` feature is off. It keeps public signatures stable and
/// nameable without leaking `()` into rustdoc or allowing accidental construction
/// of meaningful values. It implements `Default` so callers can write
/// `AdaptiveSelectorEngine::default()` in feature-agnostic code if needed.
#[cfg(not(feature = "adaptive-selectors"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct AdaptiveSelectorEngine;

// Cache machinery — always compiled when the feature is enabled, since the
// real implementation uses it internally.
#[cfg(feature = "adaptive-selectors")]
pub mod cache;

// Re-export the real implementation types when the feature is enabled
#[cfg(feature = "adaptive-selectors")]
pub use crate::application::adaptive_engine::real::{
    AdaptiveRepairOutcome,
    AdaptiveSelectorEngine as RealAdaptiveSelectorEngine,
    AdaptiveSelectorOptions,
    AdaptiveSelectorOptions as RealAdaptiveSelectorOptions,
    CascadeTrace,
    RepairStatus,
};

// Re-export the real types with their original names for internal use
#[cfg(feature = "adaptive-selectors")]
pub(crate) use crate::application::adaptive_engine::real::{
    AdaptiveRepairOutcome as InternalAdaptiveRepairOutcome,
    AdaptiveSelectorEngine as InternalAdaptiveSelectorEngine,
    AdaptiveSelectorOptions as InternalAdaptiveSelectorOptions,
    CascadeTrace as InternalCascadeTrace,
    RepairStatus as InternalRepairStatus,
};
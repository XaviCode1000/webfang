//! Domain budget model — Global→Domain→Operation→Asset concurrency tiers.
//!
//! ONE place in WebFang decides, clamps, or derives a concurrency number.
//! Enforcement adapters (semaphores, `buffer_unordered`, JoinSet gating,
//! governor permits) keep their mechanisms but derive every numeric bound
//! from this model. Dependencies point inward only: this module depends on
//! nothing outside `domain`.

pub(crate) mod clamp;
/// Canonical hardware-detection seam (`HardwareDetector`, Q2 UNIFY NOW).
pub mod detector;
/// Pure derivation fns: hardware snapshots → tier newtypes (no IO, no clock).
pub mod derivation;
/// Hardware-detector seam + pure derivation fns live in sibling modules;
/// tier newtypes and the tier aggregate are re-exported here.
pub mod tiers;

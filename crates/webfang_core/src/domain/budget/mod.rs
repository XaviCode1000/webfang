//! Domain budget model — Global→Domain→Operation→Asset concurrency tiers.
//!
//! ONE place in WebFang decides, clamps, or derives a concurrency number.
//! Enforcement adapters (semaphores, `buffer_unordered`, JoinSet gating,
//! governor permits) keep their mechanisms but derive every numeric bound
//! from this model. Dependencies point inward only: this module depends on
//! nothing outside `domain`.

pub(crate) mod clamp;

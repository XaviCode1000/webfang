//! Domain port for the system RAM-usage probe.
//!
//! Lives in `domain` (not `infrastructure`) because the autoscale loop in
//! [`crate::application::crawler::engine`] reads RAM pressure to throttle
//! crawl permits. The concrete probe is injected into the `Engine` via a
//! builder method; this module owns only the trait and the sealed-implementor
//! pattern.
//!
//! ADR-0012 sub-slice 3.B-1c.

/// RAM-usage reading as a percentage of total system memory, 0.0..=100.0.
///
/// `f32` (not `u8`) preserves the headroom `ResourceGovernor::ram_usage_percent`
/// truncates today (`as u8` rounds down), and keeps the door open for sub-percent
/// probes (cgroup-aware accounting) without an API break.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RamUsagePercent(pub f32);

impl RamUsagePercent {
    /// Saturating constructor — clamps out-of-range values to `[0.0, 100.0]`
    /// rather than `panic!` on bad probe output. Real sysinfo reads stay in
    /// range; clamped values only show up if a custom probe misbehaves.
    #[must_use]
    pub fn new_clamped(raw: f32) -> Self {
        Self(raw.clamp(0.0, 100.0))
    }

    /// Raw percent value (0.0..=100.0).
    #[must_use]
    pub fn as_percent(self) -> f32 {
        self.0
    }
}

// Standard sealed-trait pattern: `Sealed` is `pub(crate)` so it can only be
// `impl`'d by types in this crate. External crates can call `RamProbePort`
// methods but cannot add a new implementation.
mod sealed {
    pub(crate) trait Sealed {}
}
pub(crate) use sealed::Sealed;

/// Domain port that exposes the current system RAM-usage reading.
///
/// Object-safe: yes (single method, no generics, no `Self` in return,
/// no associated types). Implemented via the sealed [`Sealed`] trait so
/// only this crate can add a new production impl.
///
/// `Send + Sync` because the autoscale loop polls from a `tokio::spawn`-ed
/// background task on the multi-threaded runtime.
#[allow(private_bounds)] // sealed-trait pattern: `Sealed` is `pub(crate)` so the bound restricts impls to this crate, even though `RamProbePort` itself is `pub`.
pub trait RamProbePort: std::fmt::Debug + Send + Sync + Sealed {
    /// Return the current system RAM-usage percentage.
    ///
    /// Implementations MUST return a value in `0.0..=100.0`; the
    /// [`RamUsagePercent::new_clamped`] constructor enforces the invariant
    /// for implementations that build the newtype directly. The autoscale
    /// loop compares against the 80/90 thresholds from
    /// [`crate::domain::budget::derivation::RamThresholds`].
    fn ram_usage_percent(&self) -> RamUsagePercent;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Fake probe that returns a configurable reading. Doubles as the
    /// dyn-compatibility compile test: if `RamProbePort` ever stops being
    /// object-safe (associated type, generic method, `Self` in return)
    /// this `Arc<dyn RamProbePort>` line will fail to type-check.
    #[derive(Debug)]
    struct FakeProbe(RamUsagePercent);

    impl RamProbePort for FakeProbe {
        fn ram_usage_percent(&self) -> RamUsagePercent {
            self.0
        }
    }
    impl Sealed for FakeProbe {}

    #[test]
    fn new_clamped_clamps_below_zero() {
        assert_eq!(RamUsagePercent::new_clamped(-3.5).as_percent(), 0.0);
    }

    #[test]
    fn new_clamped_clamps_above_hundred() {
        assert_eq!(RamUsagePercent::new_clamped(150.0).as_percent(), 100.0);
    }

    #[test]
    fn new_clamped_passes_through_in_range() {
        assert_eq!(RamUsagePercent::new_clamped(73.2).as_percent(), 73.2);
    }

    #[test]
    fn trait_is_object_safe() {
        // Dyn-compat compile test: Arc<dyn RamProbePort> must compile.
        let probe: Arc<dyn RamProbePort> = Arc::new(FakeProbe(RamUsagePercent(42.0)));
        assert_eq!(probe.ram_usage_percent(), RamUsagePercent(42.0));
    }

    #[test]
    fn fake_probe_returns_configured_value() {
        let probe = FakeProbe(RamUsagePercent(85.5));
        assert_eq!(probe.ram_usage_percent().as_percent(), 85.5);
    }
}

//! Sysinfo-backed implementation of [`crate::domain::ram_probe_port::RamProbePort`].
//!
//! ADR-0012 sub-slice 3.B-1c: ports the autoscale loop in
//! [`crate::application::crawler::engine::Engine::with_autoscale`] to a
//! domain-owned seam. The loop no longer reaches into
//! `infrastructure::downloader::ResourceGovernor` directly; it holds an
//! `Arc<dyn RamProbePort>` and the production wiring (`cli`, gate-exempt)
//! injects this [`SystemRamProbe`].
//!
//! The conversion from `ResourceGovernor::ram_usage_percent`'s `u8` to
//! the new port's `f32` happens here once; the autoscale loop reads `f32`
//! and compares against the 80/90 thresholds (which stay `u8` in
//! `RamThresholds` — the comparison is well-defined because `f32::from(u8)`
//! is exact).

use std::fmt;

use sysinfo::System;

use crate::domain::ram_probe_port::{RamProbePort, RamUsagePercent, Sealed};

/// Production [`RamProbePort`] — reads `used_memory / total_memory` via
/// `sysinfo` on every call. The probe is intentionally stateless: each
/// `ram_usage_percent()` call creates a fresh `System` and refreshes it,
/// mirroring the existing `ResourceGovernor::ram_usage_percent()` static
/// method so behaviour is preserved end-to-end.
#[derive(Default)]
pub struct SystemRamProbe {
    // Reserved for future caching/state. Empty for now so the type stays
    // trivially constructible — the autoscale loop polls at 5s and a
    // cached value would add staleness for no measurable win.
    _private: (),
}

impl SystemRamProbe {
    /// Create a fresh probe. Zero-cost; the type currently carries no state.
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl fmt::Debug for SystemRamProbe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemRamProbe").finish_non_exhaustive()
    }
}

impl Sealed for SystemRamProbe {}

impl RamProbePort for SystemRamProbe {
    fn ram_usage_percent(&self) -> RamUsagePercent {
        let mut sys = System::new();
        sys.refresh_memory();
        let total = sys.total_memory();
        if total == 0 {
            return RamUsagePercent::new_clamped(0.0);
        }
        let used = sys.used_memory();
        // u64 → f32 preserves the full `used * 100 / total` integer-divide
        // semantics of `ResourceGovernor::ram_usage_percent`; cast to f32
        // before the divide so we keep the percent in floating point and
        // round through `new_clamped` to honour the trait contract.
        let pct = (used as f32) * 100.0 / (total as f32);
        RamUsagePercent::new_clamped(pct)
    }
}

#[cfg(test)]
#[cfg(not(miri))] // sysinfo calls sysconf(SC_PHYS_PAGES) — Miri cannot execute it
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn probe_is_object_safe() {
        // Compile test: Arc<dyn RamProbePort> must resolve through the
        // sealed marker. If `RamProbePort` ever stops being object-safe
        // (associated type, generic method, Self in return) this line
        // breaks before any of the engine wiring can fail.
        let probe: Arc<dyn RamProbePort> = Arc::new(SystemRamProbe::new());
        let reading = probe.ram_usage_percent();
        assert!(
            (0.0..=100.0).contains(&reading.as_percent()),
            "probe must return 0.0..=100.0, got {reading:?}",
        );
    }

    #[test]
    fn debug_is_implemented() {
        // Sanity: `#[derive(Default)]` plus the manual `Debug` impl must
        // both compile. The trait bound `RamProbePort: Debug` enforces this.
        let _ = format!("{:?}", SystemRamProbe::new());
    }
}

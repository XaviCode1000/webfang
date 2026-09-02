//! Sysinfo-backed [`RamProbePort`](crate::domain::ram_probe_port::RamProbePort)
//! implementation.
//!
//! ADR-0012 sub-slice 3.B-1c: ports the autoscale loop in
//! [`crate::application::crawler::engine::Engine::with_autoscale`] to a
//! domain-owned seam. The loop no longer reaches into
//! `infrastructure::downloader::ResourceGovernor` directly; it holds an
//! `Arc<dyn RamProbePort>` and the production wiring (`cli`, gate-exempt)
//! injects the probe defined here.
//!
//! ADR-0012-B cheap win: the `SystemRamProbe` **type** moved to
//! [`crate::domain::ram_probe_port`], so `Engine::new` can supply its default
//! probe through [`system_default`](crate::domain::ram_probe_port::system_default)
//! without naming an infrastructure concrete. This module keeps what actually
//! needs infrastructure — the `sysinfo` I/O behind `impl RamProbePort` — and
//! re-exports the type so the historical public path
//! `crate::infrastructure::downloader::system_ram_probe::SystemRamProbe` still
//! resolves. Same split as [`crate::infrastructure::ssrf`], which supplies the
//! trait impl for the domain-owned `DefaultSsrfGuard`.
//!
//! The conversion from `ResourceGovernor::ram_usage_percent`'s `u8` to
//! the new port's `f32` happens here once; the autoscale loop reads `f32`
//! and compares against the 80/90 thresholds (which stay `u8` in
//! `RamThresholds` — the comparison is well-defined because `f32::from(u8)`
//! is exact).

use sysinfo::System;

use crate::domain::ram_probe_port::{RamProbePort, RamUsagePercent, Sealed};

// The type is domain-owned since ADR-0012-B; only its behaviour lives here.
// Re-exported so every pre-existing public path and doc link keeps resolving.
pub use crate::domain::ram_probe_port::SystemRamProbe;

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
        // Sanity: the domain-owned type's manual `Debug` impl must be reachable
        // through this module's re-export. `RamProbePort: Debug` enforces it.
        let _ = format!("{:?}", SystemRamProbe::new());
    }
}

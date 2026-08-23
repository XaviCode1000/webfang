//! Canonical hardware-detection seam (Decision Q2 — UNIFY NOW).
//!
//! ONE seam answers "how many cores / how much RAM does this machine have"
//! process-wide, so `"auto"` means the same thing everywhere — including
//! cgroup-limited containers where `available_parallelism` and `num_cpus`
//! disagree.
//!
//! Miri safety follows the `with_max_instances` pattern:
//! [`SystemDetector`](detector::SystemDetector)
//! bodies that touch `sysinfo`/`sysconf` are gated `#[cfg(not(miri))]`;
//! [`FixedDetector`](detector::FixedDetector) provides explicit-value construction for tests and
//! Miri runs. All *derivation* logic consumes plain [`DetectedHw`](detector::DetectedHw) data and
//! is fully testable under Miri.

use std::num::NonZeroUsize;

/// Plain, IO-free snapshot of detected hardware.
///
/// Derivation functions take this plain data (never a live detector), which
/// keeps them pure and Miri-safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedHw {
    /// Logical parallelism available to the process (cores, cgroup-aware).
    pub parallelism: NonZeroUsize,
    /// Total system RAM in bytes, when detectable.
    pub total_ram_bytes: Option<u64>,
}

impl DetectedHw {
    /// Build a snapshot from explicit values (test/derivation entry point).
    #[must_use]
    pub const fn new(parallelism: NonZeroUsize, total_ram_bytes: Option<u64>) -> Self {
        Self {
            parallelism,
            total_ram_bytes,
        }
    }
}

/// The single canonical hardware-detection seam.
///
/// Dyn-compatible and `Send + Sync` so implementations can be shared across
/// threads and injected into every consumer (`dyn HardwareDetector`).
pub trait HardwareDetector: Send + Sync {
    /// Logical parallelism available to the process.
    fn parallelism(&self) -> NonZeroUsize;

    /// Total system RAM in bytes, `None` when not detectable.
    fn total_ram_bytes(&self) -> Option<u64>;

    /// Snapshot the detection result as plain data for pure derivations.
    fn detect(&self) -> DetectedHw {
        DetectedHw::new(self.parallelism(), self.total_ram_bytes())
    }
}

/// System-backed detector: `available_parallelism` + `sysinfo`.
///
/// The sysconf/sysinfo-touching bodies are compiled out under Miri; the
/// Miri-visible fallback returns minimal safe values (derivation tests use
/// [`FixedDetector`] instead).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDetector;

impl SystemDetector {
    /// Create the system-backed detector.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Process-wide canonical hardware parallelism (decision Q2 UNIFY NOW).
///
/// Every subsystem that previously called `num_cpus::get()` or read
/// `available_parallelism` directly MUST derive from this seam so that
/// "auto" means the same thing process-wide, cgroup limits included.
#[must_use]
pub fn system_parallelism() -> NonZeroUsize {
    SystemDetector.parallelism()
}

impl HardwareDetector for SystemDetector {
    fn parallelism(&self) -> NonZeroUsize {
        #[cfg(not(miri))]
        {
            std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
        }
        #[cfg(miri)]
        {
            // sysconf is unsupported under Miri; derivations under test use
            // FixedDetector with explicit values instead.
            NonZeroUsize::MIN
        }
    }

    fn total_ram_bytes(&self) -> Option<u64> {
        #[cfg(not(miri))]
        {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            Some(sys.total_memory())
        }
        #[cfg(miri)]
        {
            None
        }
    }
}

/// Explicit-value detector for tests and Miri runs (`with_max_instances`
/// pattern): callers hand in the exact values a real detector would return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedDetector {
    hw: DetectedHw,
}

impl FixedDetector {
    /// Construct from explicit values.
    ///
    /// Mirrors the `with_max_instances` seam pattern used by the resource
    /// governor: no sysinfo involvement, safe under Miri.
    #[must_use]
    pub const fn with_detection(parallelism: NonZeroUsize, total_ram_bytes: Option<u64>) -> Self {
        Self {
            hw: DetectedHw::new(parallelism, total_ram_bytes),
        }
    }

    /// The fixed snapshot.
    #[must_use]
    pub const fn detected(&self) -> &DetectedHw {
        &self.hw
    }
}

impl HardwareDetector for FixedDetector {
    fn parallelism(&self) -> NonZeroUsize {
        self.hw.parallelism
    }

    fn total_ram_bytes(&self) -> Option<u64> {
        self.hw.total_ram_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cores(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("test core counts are non-zero")
    }

    #[test]
    fn fixed_detector_returns_explicit_values() {
        let det = FixedDetector::with_detection(cores(8), Some(16_000_000_000));
        assert_eq!(det.parallelism().get(), 8);
        assert_eq!(det.total_ram_bytes(), Some(16_000_000_000));
    }

    #[test]
    fn fixed_detector_without_ram_reports_none() {
        let det = FixedDetector::with_detection(cores(2), None);
        assert_eq!(det.total_ram_bytes(), None);
    }

    #[test]
    fn detect_snapshot_is_plain_data() {
        let det = FixedDetector::with_detection(cores(4), Some(8_000_000_000));
        let hw = det.detect();
        assert_eq!(hw, DetectedHw::new(cores(4), Some(8_000_000_000)));
    }

    /// Trait-object usage through `dyn HardwareDetector` (injection seam).
    #[test]
    fn dyn_trait_object_dispatches() {
        let detectors: Vec<Box<dyn HardwareDetector>> = vec![
            Box::new(FixedDetector::with_detection(cores(3), None)),
            Box::new(FixedDetector::with_detection(cores(12), Some(4))),
        ];
        assert_eq!(detectors[0].parallelism().get(), 3);
        assert_eq!(detectors[1].total_ram_bytes(), Some(4));
    }

    /// `Send + Sync` bound proof: the seam must be shareable across threads.
    #[test]
    fn detector_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FixedDetector>();
        assert_send_sync::<SystemDetector>();
        assert_send_sync::<Box<dyn HardwareDetector>>();
    }

    /// Unification-map note: the 6 detection sites to migrate onto this seam
    /// in PR2 are config.rs `resolve`, autotuning.rs `detect_cpu_cores`,
    /// http_client/factory.rs pool_size, wreq_downloader.rs pool_size,
    /// ai/inference_engine.rs workers, cli/elastic.rs concurrency.
    #[test]
    fn system_detector_compiles_and_answers() {
        let det = SystemDetector::new();
        // Under both cfg(miri) and normal builds this must answer ≥ 1 core;
        // RAM may be None only under Miri.
        assert!(det.parallelism().get() >= 1);
        #[cfg(not(miri))]
        assert!(det.total_ram_bytes().is_some());
    }
}

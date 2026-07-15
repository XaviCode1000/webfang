//! Dedicated Rayon CPU pool for the elastic ingestion pipeline (Issue #51, PR3).
//!
//! Isolates CPU-bound work (semantic cleaning, ONNX inference) from the Tokio
//! async runtime via a dedicated, sized `rayon::ThreadPool` (frozen design
//! decision #4). Pool sizing honours `ElasticConfig.cpu_cores` with a
//! cooperative mono-core floor of 1 (frozen user decision #2).

use std::sync::Arc;

use crate::error::ScraperError;
use crate::infrastructure::autotuning::ElasticConfig;

/// Dedicated, isolated Rayon thread pool for CPU-bound ingestion work.
///
/// Wraps a `rayon::ThreadPool` in an `Arc` so it can be cheaply cloned and
/// shared across Tokio tasks (the bridge spawns a fresh blocking task per
/// dispatch). Created via [`RayonCpuPool::new`] or
/// [`RayonCpuPool::from_config`].
///
/// # Mono-core behaviour
///
/// When `cpu_cores == 1` the pool contains exactly one Rayon thread and the
/// OS cooperatively time-slices between Tokio and Rayon (frozen user decision
/// #2). A thread count of `0` is floored to `1` rather than panicking.
#[derive(Clone)]
pub struct RayonCpuPool {
    pool: Arc<rayon::ThreadPool>,
    threads: usize,
}

impl RayonCpuPool {
    /// Build a pool with `threads` worker threads.
    ///
    /// The thread count is floored to at least `1` (frozen user decision #2:
    /// mono-core cooperative scheduling) so `new(0)` yields a single-thread
    /// pool rather than panicking or erroring.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Ingestion`] if Rayon fails to build the pool
    /// (e.g. the host refuses to spawn the requested threads).
    pub fn new(threads: usize) -> Result<Self, ScraperError> {
        let threads = threads.max(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map_err(ScraperError::ingestion)?;
        Ok(Self {
            pool: Arc::new(pool),
            threads,
        })
    }

    /// Build a pool sized from a resolved [`ElasticConfig`]'s `cpu_cores`
    /// (frozen design: pool size = detected CPU cores, overridable via
    /// `--cpu-cores`).
    pub fn from_config(cfg: &ElasticConfig) -> Result<Self, ScraperError> {
        Self::new(cfg.cpu_cores)
    }

    /// Number of worker threads in this pool (guaranteed `>= 1`).
    #[must_use]
    pub fn thread_count(&self) -> usize {
        self.threads
    }

    /// Run `f` on the current thread with this pool installed as the active
    /// Rayon pool, so any nested `par_iter` / `rayon::spawn` routes to it
    /// (frozen design decision #3).
    ///
    /// The `Send` bounds mirror `rayon::ThreadPool::install`'s real signature
    /// (the design contract's `impl FnOnce() -> R` is shorthand).
    pub fn install<R>(&self, f: impl FnOnce() -> R + Send) -> R
    where
        R: Send,
    {
        self.pool.install(f)
    }
}

#[cfg(test)]
mod tests {
    use super::RayonCpuPool;
    use crate::infrastructure::autotuning::{ElasticConfig, ElasticOverrides};

    #[test]
    fn test_pool_creates_with_requested_thread_count() {
        let pool = RayonCpuPool::new(4).expect("pool of 4 threads should build");
        assert_eq!(pool.thread_count(), 4);
    }

    #[test]
    fn test_pool_thread_count_matches_resolved_cpu_cores() {
        let cfg = ElasticConfig::resolve(&ElasticOverrides::default());
        let pool = RayonCpuPool::from_config(&cfg).expect("pool from config should build");
        assert_eq!(pool.thread_count(), cfg.cpu_cores);
        assert!(pool.thread_count() >= 1);
    }

    #[test]
    fn test_pool_zero_threads_floors_to_one() {
        // Frozen user decision #2: pool size MUST be >= 1 (mono-core cooperative).
        // new(0) must NOT panic and must yield at least 1 thread.
        let pool = RayonCpuPool::new(0).expect("zero must floor to 1, not error");
        assert!(pool.thread_count() >= 1);
        assert_eq!(pool.thread_count(), 1);
    }

    #[test]
    fn test_pool_install_scopes_nested_rayon_to_dedicated_pool() {
        // Spec "CPU work dispatched to Rayon": install() must route nested rayon
        // operations to THIS pool. Prove it by reading the current pool's thread
        // count from inside the installed closure — it must equal the pool's size,
        // not the global rayon pool's size.
        let pool = RayonCpuPool::new(3).expect("pool of 3 threads should build");
        let observed = pool.install(rayon::current_num_threads);
        assert_eq!(observed, 3, "install() must scope nested rayon to the pool");
    }

    #[test]
    fn test_pool_mono_core_single_thread_works() {
        let pool = RayonCpuPool::new(1).expect("mono-core pool should build");
        assert_eq!(pool.thread_count(), 1);
        let sum = pool.install(|| (1..=100).sum::<u64>());
        assert_eq!(sum, 5050);
    }

    #[test]
    fn test_pool_install_returns_closure_value() {
        let pool = RayonCpuPool::new(2).expect("pool of 2 threads should build");
        let s = pool.install(|| String::from("processed"));
        assert_eq!(s, "processed");
    }
}

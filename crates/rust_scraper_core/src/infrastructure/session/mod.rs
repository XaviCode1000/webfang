//! Domain session pool for per-domain rate limiting.
//!
//! Tracks request timing per domain to prevent hammering a single host.

pub mod pool;

pub use pool::DomainSessionPool;

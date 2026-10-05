//! User-agent pool for WAF-evasion fetches.
//!
//! The pool is the hardcoded fallback list below, kept current by
//! maintainers alongside the Chrome TLS emulation profiles. UA provisioning
//! is deliberately pure and offline: the dormant network fetch to
//! `raw.githubusercontent.com` was removed in #1827 — no production path
//! ever called it (rotation reads the hardcoded pool), so webfang contacts
//! no third party to obtain user agents.
//!
//! # Examples
//!
//! ```
//! use webfang_core::infrastructure::user_agent::UserAgentCache;
//!
//! let agents = UserAgentCache::fallback_agents();
//! assert!(!agents.is_empty());
//! ```

/// Marker type for the user-agent pool API surface.
///
/// Historically this type carried a TTL cache and a network fetch from a
/// third-party list; both were removed (#1827) because no production path
/// ever called them — rotation reads the hardcoded pool. The type remains
/// so `UserAgentCache::fallback_agents()` and the `UserAgentProvider` impl
/// keep their call sites and re-exports stable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UserAgentCache;

impl UserAgentCache {
    /// Fallback: hardcoded list updated 2026
    /// Chrome 131 (Enero 2025) y Chrome 132 (Marzo 2026)
    ///
    /// This is the ONLY user-agent source: production rotation
    /// (`WreqDownloader`) and the `UserAgentProvider` port both read from
    /// here. Keeping it hardcoded keeps every fetch path offline and makes
    /// the rotation pool deterministic in tests.
    pub fn fallback_agents() -> Vec<String> {
        vec![
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36".to_string(),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:123.0) Gecko/20100101 Firefox/123.0".to_string(),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:123.0) Gecko/20100101 Firefox/123.0".to_string(),
        ]
    }
}

/// Get a random user agent from pool
///
/// # Arguments
///
/// * `pool` - Slice of user agent strings
///
/// # Returns
///
/// `Some` with a randomly selected user agent string, or `None` when the
/// pool is empty (#1109: the old unguarded `random_range(0..0)` + index
/// panicked and aborted the calling task).
///
/// # Examples
///
/// ```
/// use webfang_core::infrastructure::user_agent::get_random_user_agent_from_pool;
///
/// let agents = vec!["Chrome/131".to_string(), "Firefox/123".to_string()];
/// let ua = get_random_user_agent_from_pool(&agents).expect("non-empty pool");
/// assert!(ua == "Chrome/131" || ua == "Firefox/123");
/// ```
#[must_use]
pub fn get_random_user_agent_from_pool(pool: &[String]) -> Option<String> {
    use rand::Rng;
    if pool.is_empty() {
        return None;
    }
    let index = rand::rng().random_range(0..pool.len());
    Some(pool[index].clone())
}

// Domain port shim — preserves `webfang_core::infrastructure::user_agent::*` API
pub use crate::domain::user_agent::{
    fallback_agents as domain_fallback_agents,
    get_random_user_agent_from_pool as domain_get_random, UserAgentPool, UserAgentProvider,
};

impl crate::domain::user_agent::UserAgentProvider for UserAgentCache {
    fn load(&self) -> Vec<String> {
        Self::fallback_agents()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_agents_chrome_version() {
        let agents = UserAgentCache::fallback_agents();
        assert!(!agents.is_empty());
        for agent in &agents {
            assert!(
                agent.contains("Chrome/13") || agent.contains("Firefox/"),
                "Agent '{agent}' should contain Chrome/13x or Firefox/"
            );
        }
    }

    #[test]
    fn test_fallback_agents_are_unique() {
        let agents = UserAgentCache::fallback_agents();
        let mut unique_agents = agents.clone();
        unique_agents.sort();
        unique_agents.dedup();
        assert_eq!(
            agents.len(),
            unique_agents.len(),
            "Fallback agents should be unique"
        );
    }

    #[test]
    fn test_get_random_user_agent_from_pool() {
        let pool = vec!["Agent1".to_string(), "Agent2".to_string()];
        let ua = get_random_user_agent_from_pool(&pool).expect("non-empty pool");
        assert!(ua == "Agent1" || ua == "Agent2");
    }

    /// Reproduction guard for #1109: an empty pool used to panic inside
    /// `random_range(0..0)`. The test compiles against both signatures
    /// (`let _` binds `String` or `Option<String>` alike): it panicked on
    /// unmodified main; the strengthened assertion pins the `None` contract.
    #[test]
    fn empty_pool_does_not_panic() {
        let ua = get_random_user_agent_from_pool(&[]);
        assert_eq!(ua, None, "empty pool must yield None, never a panic");
    }
}

//! User-Agent module with TTL-based caching
//!
//! Provides lazy-loaded user agents with 1-year cache validity.
//! Following rust-skills: perf-cache-with-ttl, err-graceful-degradation, config-externalize
//!
//! # Cache Strategy
//!
//! 1. Check cache at `~/.cache/webfang/user_agents.json`
//! 2. Extract Chrome year from cached version → if year >= current_year - 1 → USE cache
//! 3. If cache is old → download from API → save cache
//! 4. If download fails → fallback to hardcoded 2026 list
//!
//! # Examples
//!
//! ```no_run
//! use webfang_core::infrastructure::user_agent::UserAgentCache;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let agents = UserAgentCache::load().await;
//! assert!(!agents.is_empty());
//! # }
//! ```

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tracing;
use wreq::Client;
use wreq_util::Profile;

/// API URL for fresh user agents
const UA_LIST_URL: &str =
    "https://raw.githubusercontent.com/user-agents-api/data/main/user-agents.json";

/// Minimum acceptable Chrome version (2025+)
/// Chrome 131 = Enero 2025, Chrome 132 = Marzo 2026
const MIN_CHROME_VERSION: u32 = 131;

/// Cache metadata
#[derive(Debug, Deserialize, Serialize)]
pub struct UserAgentCache {
    agents: Vec<String>,
    chrome_version: u32,
    downloaded_at: DateTime<Utc>,
}

impl UserAgentCache {
    /// Get cache file path: ~/.cache/webfang/user_agents.json
    fn cache_path() -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("webfang")
            .join("user_agents.json")
    }

    /// Load UAs: cache if valid, else fetch fresh (uses Chrome145 TLS profile)
    ///
    /// # Returns
    ///
    /// `Vec<String>` - List of user agent strings (Chrome 131+ or fallback)
    ///
    /// # Errors
    ///
    /// Returns fallback agents if:
    /// - Cache read fails
    /// - API download fails
    /// - Cache is older than 1 year
    pub async fn load() -> Vec<String> {
        Self::load_with_profile(Profile::Chrome145).await
    }

    /// Load UAs with a specific TLS emulation profile
    ///
    /// Same as [`load()`](Self::load) but uses the given profile for the HTTP client
    /// when fetching fresh agents from the API.
    ///
    /// #1103: every filesystem touch in this module goes through `tokio::fs`
    /// (zero `std::fs`), so a cold or stale cache never blocks a Tokio worker
    /// on disk I/O during startup.
    pub async fn load_with_profile(profile: Profile) -> Vec<String> {
        // Fresh cache hit short-circuits the network fetch.
        if let Some(agents) = Self::fresh_cached_agents().await {
            return agents;
        }

        // Fetch fresh
        match Self::fetch_and_cache(profile).await {
            Ok(agents) => agents,
            Err(e) => {
                tracing::warn!("Failed to fetch user agents: {}", e);
                Self::fallback_agents()
            },
        }
    }

    /// Return cached user agents when the cache is fresh (Chrome version
    /// within one year of the current date); `None` when the cache is stale,
    /// missing, or unreadable.
    ///
    /// Chrome-version → calendar-year mapping: Chrome 120 = 2023,
    /// Chrome 131 = 2025, Chrome 132 = 2026
    /// (`chrome_year = 2023 + (chrome_version - 120)`).
    async fn fresh_cached_agents() -> Option<Vec<String>> {
        let cache = Self::load_from_cache().await.ok()?;
        let cache_chrome_year = 2023 + (cache.chrome_version - 120) as i32;
        let current_year = Utc::now().year();
        if cache_chrome_year >= current_year - 1 {
            tracing::info!("Using cached user agents (Chrome {})", cache.chrome_version);
            Some(cache.agents)
        } else {
            tracing::warn!(
                "Cached user agents outdated (Chrome {}), fetching fresh...",
                cache.chrome_version
            );
            None
        }
    }

    /// Load user agents from cache file
    async fn load_from_cache() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let content = tokio::fs::read_to_string(Self::cache_path()).await?;
        let cache: Self = serde_json::from_str(&content)?;
        Ok(cache)
    }

    /// Fetch user agents from API and save to cache
    async fn fetch_and_cache(
        profile: Profile,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let builder = Client::builder()
            .emulation(profile)
            .timeout(Duration::from_secs(5));
        // Same layered SSRF contract as every other production client,
        // obtained through the domain `SsrfGuard` port (#703): literal-IP
        // redirect guard + connect-time validating resolver.
        let client = crate::domain::ssrf_guard::ssrf_guard()
            .secure_client(builder)
            .build()?;

        // Fetch from API
        let agents = match client.get(UA_LIST_URL).send().await {
            Ok(resp) if resp.status().is_success() => {
                // Extract JSON from response
                let json: serde_json::Value = resp.json().await?;

                // Filter Chrome 131+ UAs
                json.as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .filter(|ua| {
                                ua.contains("Chrome/") && {
                                    ua.split("Chrome/")
                                        .nth(1)
                                        .and_then(|s| s.split('.').next())
                                        .and_then(|v| v.parse::<u32>().ok())
                                        .map(|ver| ver >= MIN_CHROME_VERSION)
                                        .unwrap_or(false)
                                }
                            })
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_else(Self::fallback_agents)
            },
            _ => Self::fallback_agents(),
        };

        // Extract Chrome version from first UA
        let chrome_version = agents
            .first()
            .and_then(|ua| ua.split("Chrome/").nth(1))
            .and_then(|s| s.split('.').next())
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(MIN_CHROME_VERSION);

        // Save cache (best-effort — see `save_cache`).
        Self::save_cache(&agents, chrome_version).await;

        tracing::info!(
            "Cached {} user agents (Chrome {})",
            agents.len(),
            chrome_version
        );

        Ok(agents)
    }

    /// Persist the fetched agents to the cache file via `tokio::fs` (#1103).
    ///
    /// Best-effort by design: read-only filesystems and containers must not
    /// fail the load — but failures are logged at debug, never silently
    /// dropped.
    async fn save_cache(agents: &[String], chrome_version: u32) {
        let cache = UserAgentCache {
            agents: agents.to_vec(),
            chrome_version,
            downloaded_at: Utc::now(),
        };

        if let Some(parent) = Self::cache_path().parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                tracing::debug!(
                    error = %e,
                    "user-agent cache dir creation failed (best-effort)"
                );
            }
        }

        if let Ok(json) = serde_json::to_string_pretty(&cache) {
            if let Err(e) = tokio::fs::write(Self::cache_path(), json).await {
                tracing::debug!(error = %e, "user-agent cache write failed (best-effort)");
            }
        }
    }

    /// Fallback: hardcoded list updated 2026
    /// Chrome 131 (Enero 2025) y Chrome 132 (Marzo 2026)
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

/// Legacy function for backward compatibility (DEPRECATED)
///
/// # Deprecated
///
/// Since 0.4.0: Use [`UserAgentCache::load()`] instead for TTL-based caching.
#[deprecated(since = "0.4.0", note = "Use UserAgentCache::load() instead")]
#[must_use]
pub fn get_random_user_agent() -> String {
    // Fallback directly (no cache). The hardcoded fallback pool is non-empty,
    // so the None arm is unreachable; `unwrap_or_default` keeps the path total
    // without reintroducing a panic (#1109).
    let agents = UserAgentCache::fallback_agents();
    get_random_user_agent_from_pool(&agents).unwrap_or_default()
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

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_user_agent_cache_load() {
        let agents = UserAgentCache::load().await;
        assert!(!agents.is_empty());
        // At least one should contain Chrome/13x or Firefox
        assert!(agents
            .iter()
            .any(|ua| ua.contains("Chrome/") || ua.contains("Firefox/")));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_user_agent_cache_load_with_profile() {
        let agents = UserAgentCache::load_with_profile(Profile::Chrome131).await;
        assert!(!agents.is_empty());
        assert!(agents
            .iter()
            .any(|ua| ua.contains("Chrome/") || ua.contains("Firefox/")));
    }

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

    #[test]
    fn test_cache_path_construction() {
        let path = UserAgentCache::cache_path();
        // Should end with webfang/user_agents.json
        assert!(path.ends_with("user_agents.json"));
        assert!(path.to_string_lossy().contains("webfang"));
    }

    /// #1103 — a fresh cache must be served from disk through `tokio::fs`
    /// without touching the network: write a valid cache file under an
    /// isolated `XDG_CACHE_HOME`, then `load_with_profile` must return
    /// exactly the cached agents (a network round-trip would return the
    /// API list or the fallback, never the marker agent).
    #[cfg_attr(
        miri,
        ignore = "filesystem + env-var cache round-trip unsupported by Miri"
    )]
    #[tokio::test]
    async fn test_fresh_cache_served_from_disk_off_the_executor() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        // Env hermeticity (#1126): the cache path reads XDG_CACHE_HOME, so the
        // mutation must serialize on the shared ENV_LOCK. `EnvGuard::with`
        // holds the lock for the whole body (including the awaited blocking
        // read) and restores the original value on drop — which runs before
        // the TempDir drops, undoing setup in reverse order.
        let _guard = webfang_test_utils::EnvGuard::with(&[(
            "XDG_CACHE_HOME",
            tmp.path().to_str().expect("utf-8 temp path"),
        )]);
        let dir = tmp.path().join("webfang");
        std::fs::create_dir_all(&dir).expect("cache dir");
        let cache = UserAgentCache {
            agents: vec!["Mozilla/5.0 (Test) Chrome/999.0.0.0 Safari/537.36".to_string()],
            chrome_version: 999,
            downloaded_at: Utc::now(),
        };
        std::fs::write(
            dir.join("user_agents.json"),
            serde_json::to_string(&cache).expect("serialize cache"),
        )
        .expect("write cache");

        let agents = UserAgentCache::load_with_profile(Profile::Chrome145).await;

        assert_eq!(
            agents, cache.agents,
            "fresh cache must be served from disk, not the network"
        );
    }
}

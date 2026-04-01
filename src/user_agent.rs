//! User-Agent module with TTL-based caching
//!
//! Provides lazy-loaded user agents with 1-year cache validity.
//! Following rust-skills: perf-cache-with-ttl, err-graceful-degradation, config-externalize
//!
//! # Cache Strategy
//!
//! 1. Check cache at `~/.cache/rust_scraper/user_agents.json`
//! 2. Extract Chrome year from cached version → if year >= current_year - 1 → USE cache
//! 3. If cache is old → download from API → save cache
//! 4. If download fails → fallback to hardcoded 2026 list
//!
//! # Examples
//!
//! ```no_run
//! use rust_scraper::user_agent::UserAgentCache;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let agents = UserAgentCache::load().await;
//! assert!(!agents.is_empty());
//! # }
//! ```

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tracing;
use wreq::Client;
use wreq_util::Emulation;

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
    /// Get cache file path: ~/.cache/rust_scraper/user_agents.json
    fn cache_path() -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rust_scraper")
            .join("user_agents.json")
    }

    /// Load UAs: cache if valid, else fetch fresh
    ///
    /// # Returns
    ///
    /// Vec<String> - List of user agent strings (Chrome 131+ or fallback)
    ///
    /// # Errors
    ///
    /// Returns fallback agents if:
    /// - Cache read fails
    /// - API download fails
    /// - Cache is older than 1 year
    pub async fn load() -> Vec<String> {
        let current_year = Utc::now().year();

        // Try load from cache
        if let Ok(cache) = Self::load_from_cache() {
            // Chrome 120 = 2023, Chrome 131 = 2025, Chrome 132 = 2026
            // Formula: chrome_year = 2023 + (chrome_version - 120)
            let cache_chrome_year = 2023 + (cache.chrome_version - 120) as i32;

            // Cache valid if <= 1 year old
            if cache_chrome_year >= current_year - 1 {
                tracing::info!("Using cached user agents (Chrome {})", cache.chrome_version);
                return cache.agents;
            }

            tracing::warn!(
                "Cached user agents outdated (Chrome {}), fetching fresh...",
                cache.chrome_version
            );
        }

        // Fetch fresh
        match Self::fetch_and_cache().await {
            Ok(agents) => agents,
            Err(e) => {
                tracing::warn!("Failed to fetch user agents: {}", e);
                Self::fallback_agents()
            }
        }
    }

    /// Load user agents from cache file
    fn load_from_cache() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let content = fs::read_to_string(Self::cache_path())?;
        let cache: Self = serde_json::from_str(&content)?;
        Ok(cache)
    }

    /// Fetch user agents from API and save to cache
    async fn fetch_and_cache() -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let client = Client::builder()
            .emulation(Emulation::Chrome131)
            .timeout(Duration::from_secs(5))
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
            }
            _ => Self::fallback_agents(),
        };

        // Extract Chrome version from first UA
        let chrome_version = agents
            .first()
            .and_then(|ua| ua.split("Chrome/").nth(1))
            .and_then(|s| s.split('.').next())
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(MIN_CHROME_VERSION);

        // Save cache (ignore errors - read-only FS, containers, etc.)
        let cache = UserAgentCache {
            agents: agents.clone(),
            chrome_version,
            downloaded_at: Utc::now(),
        };

        if let Some(parent) = Self::cache_path().parent() {
            let _ = fs::create_dir_all(parent); // Ignore errors
        }

        // Silently ignore write errors (read-only FS, containers, etc.)
        if let Ok(json) = serde_json::to_string_pretty(&cache) {
            let _ = fs::write(Self::cache_path(), json);
        }

        tracing::info!(
            "Cached {} user agents (Chrome {})",
            agents.len(),
            chrome_version
        );

        Ok(agents)
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
/// A randomly selected user agent string
///
/// # Examples
///
/// ```
/// use rust_scraper::user_agent::get_random_user_agent_from_pool;
///
/// let agents = vec!["Chrome/131".to_string(), "Firefox/123".to_string()];
/// let ua = get_random_user_agent_from_pool(&agents);
/// assert!(ua == "Chrome/131" || ua == "Firefox/123");
/// ```
#[must_use]
pub fn get_random_user_agent_from_pool(pool: &[String]) -> String {
    let rand_idx = rand::random::<usize>() % pool.len();
    pool[rand_idx].clone()
}

/// Legacy function for backward compatibility (DEPRECATED)
///
/// # Deprecated
///
/// Since 0.4.0: Use [`UserAgentCache::load()`] instead for TTL-based caching.
#[deprecated(since = "0.4.0", note = "Use UserAgentCache::load() instead")]
#[must_use]
pub fn get_random_user_agent() -> String {
    // Fallback directly (no cache)
    let agents = UserAgentCache::fallback_agents();
    get_random_user_agent_from_pool(&agents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_user_agent_cache_load() {
        let agents = UserAgentCache::load().await;
        assert!(!agents.is_empty());
        // At least one should contain Chrome/13x or Firefox
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
                "Agent '{}' should contain Chrome/13x or Firefox/",
                agent
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
        let ua = get_random_user_agent_from_pool(&pool);
        assert!(ua == "Agent1" || ua == "Agent2");
    }

    #[test]
    fn test_cache_path_construction() {
        let path = UserAgentCache::cache_path();
        // Should end with rust_scraper/user_agents.json
        assert!(path.ends_with("user_agents.json"));
        assert!(path.to_string_lossy().contains("rust_scraper"));
    }
}

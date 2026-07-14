//! Robots.txt utilities
//!
//! Functions for handling robots.txt rules:
//! - Parsing Crawl-delay directives
//! - Fetching and caching robots.txt rules
//! - Checking URL permissions
//!
//! Extracted from discovery.rs to keep it orchestration-only.

use std::sync::Arc;

use dashmap::DashMap;
use robotstxt::DefaultMatcher;

/// Parsed robots.txt rules for a domain.
///
/// Following **api-non-exhaustive**: can add fields without breaking changes.
/// Following **own-arc-shared**: wrapped in `Arc` for cache sharing.
#[derive(Debug, Clone)]
pub struct RobotsRules {
    /// Raw robots.txt content for the robotstxt matcher.
    pub content: String,
    /// Parsed Crawl-delay in seconds, if present.
    pub crawl_delay_secs: Option<f64>,
}

/// Cache of robots.txt rules keyed by domain.
///
/// Using `DashMap` for lock-free concurrent reads during crawl.
/// No TTL — robots.txt rarely changes during a single crawl session.
pub type RobotsCache = DashMap<String, Arc<RobotsRules>>;

/// Create a new empty robots.txt cache.
#[must_use]
pub fn new_robots_cache() -> RobotsCache {
    DashMap::new()
}

/// Parse Crawl-delay from raw robots.txt content.
///
/// Searches for `Crawl-delay:` directives (case-insensitive) and returns
/// the first valid numeric value found.
///
/// # Arguments
///
/// * `content` - Raw robots.txt content
///
/// # Returns
///
/// Parsed Crawl-delay in seconds, or None if not found
///
/// # Examples
///
/// ```
/// use rust_scraper::infrastructure::crawler::robots_utils::parse_crawl_delay;
///
/// assert_eq!(parse_crawl_delay("User-agent: *\nCrawl-delay: 5\n"), Some(5.0));
/// assert_eq!(parse_crawl_delay("User-agent: *\n"), None);
/// ```
pub fn parse_crawl_delay(content: &str) -> Option<f64> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.to_lowercase().starts_with("crawl-delay:") {
            if let Some(val_str) = trimmed.split(':').nth(1) {
                if let Ok(val) = val_str.trim().parse::<f64>() {
                    return Some(val);
                }
            }
        }
    }
    None
}

/// Fetch and cache robots.txt rules for a domain.
///
/// On cache miss, fetches `robots.txt` from the domain root using wreq.
/// Parses the content and caches the result. Returns `None` if fetching
/// or parsing fails (fail-open: treat as all-allowed).
///
/// # Arguments
///
/// * `domain` - Domain to fetch robots.txt for
/// * `cache` - Shared robots.txt rules cache
///
/// # Returns
///
/// Parsed robots.txt rules, or None if unavailable
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::infrastructure::crawler::robots_utils::{new_robots_cache, fetch_robots_rules};
///
/// # #[tokio::main]
/// # async fn main() {
/// let cache = new_robots_cache();
/// let rules = fetch_robots_rules("example.com", &cache).await;
/// # }
/// ```
pub async fn fetch_robots_rules(domain: &str, cache: &RobotsCache) -> Option<Arc<RobotsRules>> {
    if let Some(rules) = cache.get(domain) {
        return Some(Arc::clone(rules.value()));
    }

    let robots_url = format!("https://{domain}/robots.txt");
    tracing::debug!("Fetching robots.txt from {}", robots_url);

    let content = match wreq::get(&robots_url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("Failed to read robots.txt body for {}: {}", domain, e);
                return None;
            },
        },
        Ok(resp) => {
            tracing::debug!(
                "robots.txt for {} returned status {}, treating as all-allowed",
                domain,
                resp.status()
            );
            return None;
        },
        Err(e) => {
            tracing::warn!("Failed to fetch robots.txt for {}: {}", domain, e);
            return None;
        },
    };

    let crawl_delay = parse_crawl_delay(&content);
    let rules = Arc::new(RobotsRules {
        content,
        crawl_delay_secs: crawl_delay,
    });
    cache.insert(domain.to_string(), Arc::clone(&rules));
    Some(rules)
}

/// Check if a URL is allowed by the site's robots.txt.
///
/// Fetches robots.txt on first encounter (cached per domain).
/// Uses the `robotstxt` crate's `DefaultMatcher` for path matching.
/// Fail-open: if robots.txt cannot be fetched, the URL is allowed.
///
/// # Arguments
///
/// * `url` - The full URL to check
/// * `domain` - The domain key for cache lookup
/// * `cache` - Shared robots.txt rules cache
///
/// # Returns
///
/// `true` if the URL is allowed by robots.txt (or if robots.txt is unavailable).
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::infrastructure::crawler::robots_utils::{new_robots_cache, is_allowed_by_robots};
///
/// # #[tokio::main]
/// # async fn main() {
/// let cache = new_robots_cache();
/// assert!(is_allowed_by_robots("https://example.com/page", "example.com", &cache).await);
/// # }
/// ```
pub async fn is_allowed_by_robots(url: &str, domain: &str, cache: &RobotsCache) -> bool {
    let rules = match fetch_robots_rules(domain, cache).await {
        Some(r) => r,
        None => return true, // fail-open
    };

    let mut matcher = DefaultMatcher::default();
    matcher.one_agent_allowed_by_robots(&rules.content, "*", url)
}

/// Get the crawl-delay for a domain in seconds, if configured.
///
/// Returns `None` if no Crawl-delay directive was found.
///
/// # Arguments
///
/// * `domain` - Domain to get crawl-delay for
/// * `cache` - Shared robots.txt rules cache
///
/// # Returns
///
/// Crawl-delay in seconds, or None if not configured
///
/// # Examples
///
/// ```
/// use rust_scraper::infrastructure::crawler::robots_utils::{new_robots_cache, get_crawl_delay};
/// use std::sync::Arc;
/// use rust_scraper::infrastructure::crawler::robots_utils::RobotsRules;
///
/// let cache = new_robots_cache();
/// cache.insert("example.com".to_string(), Arc::new(RobotsRules {
///     content: String::new(),
///     crawl_delay_secs: Some(5.0),
/// }));
///
/// assert_eq!(get_crawl_delay("example.com", &cache), Some(5.0));
/// assert_eq!(get_crawl_delay("unknown.com", &cache), None);
/// ```
pub fn get_crawl_delay(domain: &str, cache: &RobotsCache) -> Option<f64> {
    cache.get(domain).and_then(|r| r.crawl_delay_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_crawl_delay_basic() {
        let robots_body = "\
User-agent: *
Crawl-delay: 5
Disallow: /tmp/";

        assert_eq!(parse_crawl_delay(robots_body), Some(5.0));
    }

    #[test]
    fn test_parse_crawl_delay_none() {
        let no_delay = "User-agent: *\nDisallow: /";
        assert_eq!(parse_crawl_delay(no_delay), None);
    }

    #[test]
    fn test_parse_crawl_delay_fractional() {
        let fractional = "User-agent: *\nCrawl-delay: 0.5\n";
        assert_eq!(parse_crawl_delay(fractional), Some(0.5));
    }

    #[test]
    fn test_parse_crawl_delay_case_insensitive() {
        let robots_body = "user-agent: *\nCrawl-Delay: 10\n";
        assert_eq!(parse_crawl_delay(robots_body), Some(10.0));

        let robots_body_upper = "User-Agent: *\nCRAWL-DELAY: 3\n";
        assert_eq!(parse_crawl_delay(robots_body_upper), Some(3.0));
    }

    #[tokio::test]
    async fn test_robots_cache_hit() {
        let cache = new_robots_cache();
        let rules = Arc::new(RobotsRules {
            content: "User-agent: *\nDisallow: /private/\n".to_string(),
            crawl_delay_secs: Some(2.0),
        });
        cache.insert("example.com".to_string(), rules);

        // Should allow public URL
        assert!(is_allowed_by_robots("https://example.com/public", "example.com", &cache).await);
        // Should disallow private URL
        assert!(
            !is_allowed_by_robots("https://example.com/private/secret", "example.com", &cache)
                .await
        );
    }

    #[test]
    fn test_get_crawl_delay_returns_cached_value() {
        let cache = new_robots_cache();
        let rules = Arc::new(RobotsRules {
            content: String::new(),
            crawl_delay_secs: Some(7.5),
        });
        cache.insert("slow-site.com".to_string(), rules);

        assert_eq!(get_crawl_delay("slow-site.com", &cache), Some(7.5));
        assert_eq!(get_crawl_delay("unknown.com", &cache), None);
    }

    #[test]
    fn test_robots_txt_empty_disallow_all() {
        let robots_body = "User-agent: *\nDisallow: /\n";
        let mut matcher = DefaultMatcher::default();
        assert!(
            !matcher.one_agent_allowed_by_robots(robots_body, "*", "https://example.com/anything"),
            "Disallow: / should block everything"
        );
    }

    #[test]
    fn test_robots_txt_empty_permissive() {
        let robots_body = "User-agent: *\n";
        let mut matcher = DefaultMatcher::default();
        assert!(
            matcher.one_agent_allowed_by_robots(robots_body, "*", "https://example.com/anything"),
            "Empty robots.txt (no Disallow) should allow everything"
        );
    }
}

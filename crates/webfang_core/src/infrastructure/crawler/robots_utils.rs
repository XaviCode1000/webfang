//! Robots.txt utilities
//!
//! Components for handling robots.txt rules:
//! - Parsing Crawl-delay directives
//! - Fetching and caching robots.txt rules with a TLS-fingerprinted client
//! - Checking URL permissions
//!
//! The [`RobotsFetcher`] owns a shared `wreq::Client` built with the caller's
//! TLS/HTTP2 emulation profile (#337). Previously robots.txt was fetched with a
//! bare `wreq::get()`, which spun up a throwaway client carrying the *default*
//! TLS fingerprint — a bot signal for WAFs that compared it against the main
//! page downloader's fingerprint. Routing the fetch through an emulated client
//! keeps the robots.txt fingerprint consistent with the rest of the crawl.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use robotstxt::DefaultMatcher;
use url::Url;
use wreq::Client;
use wreq_util::Profile;

use crate::infrastructure::error::InfraError;

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
/// use webfang_core::infrastructure::crawler::robots_utils::parse_crawl_delay;
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

/// Build the robots.txt URL for a page URL, preserving its scheme and port.
///
/// The previous implementation hardcoded `https://{domain}/robots.txt`, which
/// broke robots enforcement for plain-HTTP sites and non-standard ports (the
/// port was dropped and the scheme forced to https). Deriving the URL from the
/// page's own origin fixes both. Falls back to the https form if parsing fails.
fn robots_txt_url(url: &str, domain: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => format!("{}/robots.txt", parsed.origin().ascii_serialization()),
        Err(_) => format!("https://{domain}/robots.txt"),
    }
}

/// Fetches and caches robots.txt rules using a TLS-fingerprinted HTTP client.
///
/// The internal `wreq::Client` is built once with the caller's TLS/HTTP2
/// emulation [`Profile`] and shared via `Arc` (#337). This keeps the robots.txt
/// fingerprint consistent with the main page downloader instead of leaking the
/// default fingerprint through a throwaway `wreq::get()` client.
///
/// Following **own-arc-shared**: the client is `Arc`-wrapped so the fetcher can
/// be shared cheaply across spawned crawl tasks.
pub struct RobotsFetcher {
    /// Shared HTTP client built with the configured TLS emulation profile.
    client: Arc<Client>,
    /// Per-domain robots.txt rules cache (lock-free, shared across tasks).
    cache: RobotsCache,
}

impl RobotsFetcher {
    /// Create a new fetcher whose client uses the given TLS/HTTP2 `profile`.
    ///
    /// The client is built once and shared via `Arc` for connection pooling. It
    /// mirrors the main downloader's fingerprint and enables gzip/brotli plus a
    /// bounded redirect policy so the robots.txt fetch is indistinguishable from
    /// a regular page fetch (#337).
    ///
    /// # Arguments
    ///
    /// * `profile` - TLS/HTTP2 fingerprint emulation preset applied to the client
    /// * `timeout_secs` - Request timeout in seconds; the connect timeout is
    ///   clamped to `min(timeout_secs, 10)`
    ///
    /// # Errors
    ///
    /// Returns [`InfraError::Network`] if the wreq client cannot be built.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use webfang_core::infrastructure::crawler::robots_utils::RobotsFetcher;
    ///
    /// let fetcher = RobotsFetcher::new(wreq_util::Profile::Chrome145, 30).unwrap();
    /// ```
    pub fn new(profile: Profile, timeout_secs: u64) -> Result<Self, InfraError> {
        let connect_timeout_secs = timeout_secs.min(10);

        let client = Client::builder()
            .emulation(profile)
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(connect_timeout_secs))
            .gzip(true)
            .brotli(true)
            // SSRF guard (#703): default 10-hop limit + stops redirects that
            // target a literal forbidden IP. Hostname targets are validated
            // at entry by the async SSRF guard.
            .redirect(crate::infrastructure::ssrf::redirect_policy())
            .build()
            .map_err(|e| InfraError::Network(Box::new(e)))?;

        tracing::debug!(
            "RobotsFetcher created: timeout={}s, connect_timeout={}s",
            timeout_secs,
            connect_timeout_secs
        );

        Ok(Self {
            client: Arc::new(client),
            cache: new_robots_cache(),
        })
    }

    /// Fetch and cache robots.txt rules for a domain.
    ///
    /// On cache miss, fetches `robots.txt` from the domain root using the shared
    /// emulated client. Parses the content and caches the result. Returns `None`
    /// if fetching or parsing fails (fail-open: treat as all-allowed).
    ///
    /// # Arguments
    ///
    /// * `domain` - Cache key for the site (typically the bare host)
    /// * `url` - A page URL on the site, used to derive the robots.txt origin
    ///
    /// # Returns
    ///
    /// Parsed robots.txt rules, or None if unavailable
    async fn fetch_rules(&self, domain: &str, url: &str) -> Option<Arc<RobotsRules>> {
        if let Some(rules) = self.cache.get(domain) {
            return Some(Arc::clone(rules.value()));
        }

        let robots_url = robots_txt_url(url, domain);
        tracing::debug!("Fetching robots.txt from {}", robots_url);

        let content = self.fetch_robots_content(domain, &robots_url).await?;

        let crawl_delay = parse_crawl_delay(&content);
        let rules = Arc::new(RobotsRules {
            content,
            crawl_delay_secs: crawl_delay,
        });
        self.cache.insert(domain.to_string(), Arc::clone(&rules));
        Some(rules)
    }

    /// Fetch the raw robots.txt content, or `None` if unavailable (fail-open).
    async fn fetch_robots_content(&self, domain: &str, robots_url: &str) -> Option<String> {
        let resp = match self.client.get(robots_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("Failed to fetch robots.txt for {}: {}", domain, e);
                return None;
            },
        };

        if !resp.status().is_success() {
            tracing::debug!(
                "robots.txt for {} returned status {}, treating as all-allowed",
                domain,
                resp.status()
            );
            return None;
        }

        self.read_robots_body(domain, resp).await
    }

    /// Read the body of a successful robots.txt response.
    async fn read_robots_body(&self, domain: &str, resp: wreq::Response) -> Option<String> {
        match resp.text().await {
            Ok(text) => Some(text),
            Err(e) => {
                tracing::warn!("Failed to read robots.txt body for {}: {}", domain, e);
                None
            },
        }
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
    ///
    /// # Returns
    ///
    /// `true` if the URL is allowed by robots.txt (or if robots.txt is unavailable).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use webfang_core::infrastructure::crawler::robots_utils::RobotsFetcher;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let fetcher = RobotsFetcher::new(wreq_util::Profile::Chrome145, 30).unwrap();
    /// assert!(fetcher.is_allowed("https://example.com/page", "example.com").await);
    /// # }
    /// ```
    pub async fn is_allowed(&self, url: &str, domain: &str) -> bool {
        let rules = match self.fetch_rules(domain, url).await {
            Some(r) => r,
            None => return true, // fail-open
        };

        let mut matcher = DefaultMatcher::default();
        matcher.one_agent_allowed_by_robots(&rules.content, "*", url)
    }

    /// Get the crawl-delay for a domain in seconds, if configured.
    ///
    /// Returns `None` if the domain has not been fetched yet or no Crawl-delay
    /// directive was present in its robots.txt.
    ///
    /// # Arguments
    ///
    /// * `domain` - Domain to get crawl-delay for
    ///
    /// # Returns
    ///
    /// Crawl-delay in seconds, or None if not configured
    pub fn get_crawl_delay(&self, domain: &str) -> Option<f64> {
        self.cache.get(domain).and_then(|r| r.crawl_delay_secs)
    }
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

    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[test]
    fn test_fetcher_new_builds_client_for_profiles() {
        let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
        // A freshly built fetcher has no cached rules for any domain.
        assert_eq!(fetcher.get_crawl_delay("example.com"), None);

        // The constructor honors the caller's profile selection.
        assert!(RobotsFetcher::new(Profile::Firefox135, 5).is_ok());
    }

    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[tokio::test]
    async fn test_robots_cache_hit() {
        let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
        fetcher.cache.insert(
            "example.com".to_string(),
            Arc::new(RobotsRules {
                content: "User-agent: *\nDisallow: /private/\n".to_string(),
                crawl_delay_secs: Some(2.0),
            }),
        );

        // Should allow public URL
        assert!(
            fetcher
                .is_allowed("https://example.com/public", "example.com")
                .await
        );
        // Should disallow private URL
        assert!(
            !fetcher
                .is_allowed("https://example.com/private/secret", "example.com")
                .await
        );
    }

    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[test]
    fn test_get_crawl_delay_returns_cached_value() {
        let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
        fetcher.cache.insert(
            "slow-site.com".to_string(),
            Arc::new(RobotsRules {
                content: String::new(),
                crawl_delay_secs: Some(7.5),
            }),
        );

        assert_eq!(fetcher.get_crawl_delay("slow-site.com"), Some(7.5));
        assert_eq!(fetcher.get_crawl_delay("unknown.com"), None);
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

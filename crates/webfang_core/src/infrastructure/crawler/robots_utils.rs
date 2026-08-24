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
//!
//! Failed outcomes are cached too (#794): a 404 / non-2xx / network error
//! stores a fail-open [`RobotsCacheEntry::AllowAll`] decision per domain, so
//! the robots.txt of a site without one is fetched once per crawl instead of
//! once per checked URL.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use robotstxt::DefaultMatcher;
use tokio::sync::OnceCell;
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

/// Cached robots.txt decision per domain (#794).
///
/// Using `DashMap` for lock-free concurrent reads during crawl.
/// No TTL — robots.txt rarely changes during a single crawl session.
/// Each domain maps to a [`OnceCell`]-guarded decision: the first check for a
/// domain initializes it with exactly one fetch (success → `Rules`, any
/// failure → fail-open `AllowAll`), and concurrent first-checks share that
/// initialization instead of stampeding. The decision lives as long as the
/// owning [`RobotsFetcher`] — one crawl/session.
pub type RobotsCache = DashMap<String, Arc<OnceCell<Arc<RobotsCacheEntry>>>>;

/// Cached robots.txt outcome for one domain (#794).
///
/// Previously the cache only stored successful fetches, so a 404 or fetch
/// failure left the map empty forever and every [`RobotsFetcher::is_allowed`]
/// call re-fetched robots.txt (459 fetches for a 5-page crawl on sites
/// without robots.txt). Caching the fail-open decision makes "no rules" a
/// remembered state, distinct from "never fetched".
#[derive(Debug, Clone)]
pub enum RobotsCacheEntry {
    /// Successfully fetched and parsed robots.txt — real rules to match.
    Rules(Arc<RobotsRules>),
    /// robots.txt unavailable (404 / non-2xx / network / body-read error).
    /// Fail-open is kept and cached for the lifetime of the cache, which
    /// lives as long as its owning [`RobotsFetcher`] — one crawl/session.
    AllowAll,
}

/// Create an initialized cache cell holding `entry` (test helper).
#[cfg(test)]
fn init_cache_cell(entry: RobotsCacheEntry) -> Arc<OnceCell<Arc<RobotsCacheEntry>>> {
    let cell = OnceCell::new();
    let _ = cell.set(Arc::new(entry));
    Arc::new(cell)
}

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

/// Structured robots.txt fetch failure — reasons the domain is cached as
/// fail-open (`AllowAll`) instead of being re-fetched (#794).
#[derive(Debug, Clone)]
struct RobotsFetchFailure {
    /// Machine-readable failure reason: `network_error`, `http_status:<code>`,
    /// or `body_read_error`. Recorded on the `robots_txt_negative_cached`
    /// tracing event so trace.jsonl shows *why* a domain went fail-open.
    reason: String,
}

/// Label for a cache entry in tracing fields (`rules` / `allow_all`).
fn entry_label(entry: &RobotsCacheEntry) -> &'static str {
    match entry {
        RobotsCacheEntry::Rules(_) => "rules",
        RobotsCacheEntry::AllowAll => "allow_all",
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
    /// Per-domain robots.txt cache (lock-free, shared across tasks). Each
    /// domain maps to a [`OnceCell`]: the first check initializes it — exactly
    /// one fetch per domain, with concurrent first-checks sharing the same
    /// initialization — and every later check reads the cached decision (#794).
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
            // target a literal forbidden IP (belt-and-suspenders). Hostname
            // targets — including every redirect hop — are enforced at connect
            // time by the validating DNS resolver below.
            .redirect(crate::infrastructure::ssrf::redirect_policy())
            .dns_resolver(crate::infrastructure::ssrf::ValidatingResolver::new())
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

    /// Create a fetcher with the default production TLS fingerprint
    /// (`Profile::Chrome145`), matching `HttpClientConfig::default()`.
    ///
    /// Lets callers build a robots.txt fetcher without depending on
    /// `wreq-util` themselves (e.g. the MCP crate, which must not add that
    /// dependency — issue #705).
    ///
    /// # Errors
    ///
    /// Returns [`InfraError::Network`] if the wreq client cannot be built.
    pub fn with_default_profile(timeout_secs: u64) -> Result<Self, InfraError> {
        Self::new(Profile::Chrome145, timeout_secs)
    }

    /// Resolve the cached robots.txt decision for a domain, fetching on first
    /// access and caching the outcome — including failed outcomes (#794).
    ///
    /// On cache miss, fetches `robots.txt` from the domain root using the shared
    /// emulated client. A successful parse is cached as [`RobotsCacheEntry::Rules`].
    /// Any failure (non-2xx, network error, body-read error) is cached as
    /// [`RobotsCacheEntry::AllowAll`] (fail-open, remembered) so later calls for
    /// the same domain never re-fetch. Previously only successes reached the
    /// cache, so 404/error domains were re-fetched on every `is_allowed` call —
    /// 459 robots fetches for a 5-page crawl (issue #794).
    ///
    /// # Concurrency
    ///
    /// Each domain maps to a [`OnceCell`]: the first check to win the `entry`
    /// slot inserts it, then `get_or_init` runs exactly one fetch while every
    /// concurrent first-check for that domain awaits the same initialization.
    /// The shard guard is dropped before the fetch (no lock held across
    /// `.await`), and `OnceCell` keeps the exactly-once guarantee even then.
    /// Fetch count is therefore exactly one per domain per fetcher lifetime.
    ///
    /// # Arguments
    ///
    /// * `domain` - Cache key for the site (typically the bare host)
    /// * `url` - A page URL on the site, used to derive the robots.txt origin
    async fn resolve_entry(&self, domain: &str, url: &str) -> Arc<RobotsCacheEntry> {
        let cell: Arc<OnceCell<Arc<RobotsCacheEntry>>> = self
            .cache
            .entry(domain.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .value()
            .clone();

        if let Some(entry) = cell.get().cloned() {
            tracing::trace!(
                domain = %domain,
                entry = entry_label(&entry),
                "robots_txt_cache_hit"
            );
            return entry;
        }
        cell.get_or_init(|| self.fetch_or_allow_all(domain, url))
            .await
            .clone()
    }

    /// Fetch and parse robots.txt for a domain, or build the fail-open
    /// decision when the fetch fails. This is the per-domain single-flight
    /// initializer run by [`OnceCell::get_or_init`] — exactly once.
    async fn fetch_or_allow_all(&self, domain: &str, url: &str) -> Arc<RobotsCacheEntry> {
        let robots_url = robots_txt_url(url, domain);
        tracing::debug!("Fetching robots.txt from {}", robots_url);

        match self.fetch_robots_content(domain, &robots_url).await {
            Ok(content) => {
                let crawl_delay_secs = parse_crawl_delay(&content);
                Arc::new(RobotsCacheEntry::Rules(Arc::new(RobotsRules {
                    content,
                    crawl_delay_secs,
                })))
            },
            Err(failure) => {
                tracing::debug!(
                    domain = %domain,
                    reason = %failure.reason,
                    "robots_txt_negative_cached"
                );
                Arc::new(RobotsCacheEntry::AllowAll)
            },
        }
    }

    /// Fetch the raw robots.txt content, or a structured failure reason (#794).
    async fn fetch_robots_content(
        &self,
        domain: &str,
        robots_url: &str,
    ) -> Result<String, RobotsFetchFailure> {
        let resp = match self.client.get(robots_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("Failed to fetch robots.txt for {}: {}", domain, e);
                return Err(RobotsFetchFailure {
                    reason: "network_error".to_string(),
                });
            },
        };

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            tracing::debug!(
                "robots.txt for {} returned status {}, treating as all-allowed",
                domain,
                status
            );
            return Err(RobotsFetchFailure {
                reason: format!("http_status:{status}"),
            });
        }

        self.read_robots_body(domain, resp).await
    }

    /// Read the body of a successful robots.txt response.
    async fn read_robots_body(
        &self,
        domain: &str,
        resp: wreq::Response,
    ) -> Result<String, RobotsFetchFailure> {
        match resp.text().await {
            Ok(text) => Ok(text),
            Err(e) => {
                tracing::warn!("Failed to read robots.txt body for {}: {}", domain, e);
                Err(RobotsFetchFailure {
                    reason: "body_read_error".to_string(),
                })
            },
        }
    }

    /// Check if a URL is allowed by the site's robots.txt.
    ///
    /// Fetches robots.txt on first encounter (cached per domain, including
    /// failed outcomes — see [`RobotsCacheEntry`]). Uses the `robotstxt` crate's
    /// `DefaultMatcher` for path matching. Fail-open: if robots.txt cannot be
    /// fetched, the URL is allowed, and that decision is cached so later calls
    /// do not re-fetch (#794).
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
        let entry = self.resolve_entry(domain, url).await;
        match entry.as_ref() {
            RobotsCacheEntry::Rules(rules) => {
                let mut matcher = DefaultMatcher::default();
                matcher.one_agent_allowed_by_robots(&rules.content, "*", url)
            },
            RobotsCacheEntry::AllowAll => true,
        }
    }

    /// Get the crawl-delay for a domain in seconds, if configured.
    ///
    /// Returns `None` if the domain has not been fetched yet, if the cached
    /// decision is a negative `AllowAll` entry (no robots.txt ⇒ no Crawl-delay
    /// directive can exist), or if no Crawl-delay directive was present in its
    /// robots.txt.
    ///
    /// # Arguments
    ///
    /// * `domain` - Domain to get crawl-delay for
    ///
    /// # Returns
    ///
    /// Crawl-delay in seconds, or None if not configured
    pub fn get_crawl_delay(&self, domain: &str) -> Option<f64> {
        self.cache
            .get(domain)
            .and_then(|cell| match cell.value().get()?.as_ref() {
                RobotsCacheEntry::Rules(rules) => rules.crawl_delay_secs,
                RobotsCacheEntry::AllowAll => None,
            })
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

    /// Seed a cached `Rules` decision for a domain, bypassing the fetch.
    fn seed_rules(fetcher: &RobotsFetcher, domain: &str, content: &str, crawl_delay: Option<f64>) {
        fetcher.cache.insert(
            domain.to_string(),
            init_cache_cell(RobotsCacheEntry::Rules(Arc::new(RobotsRules {
                content: content.to_string(),
                crawl_delay_secs: crawl_delay,
            }))),
        );
    }

    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[tokio::test]
    async fn test_robots_cache_hit() {
        let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
        seed_rules(
            &fetcher,
            "example.com",
            "User-agent: *\nDisallow: /private/\n",
            Some(2.0),
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
        seed_rules(&fetcher, "slow-site.com", "", Some(7.5));

        assert_eq!(fetcher.get_crawl_delay("slow-site.com"), Some(7.5));
        assert_eq!(fetcher.get_crawl_delay("unknown.com"), None);
    }

    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[test]
    fn test_get_crawl_delay_is_none_for_negative_entry() {
        let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
        fetcher.cache.insert(
            "no-robots.com".to_string(),
            init_cache_cell(RobotsCacheEntry::AllowAll),
        );

        assert_eq!(fetcher.get_crawl_delay("no-robots.com"), None);
    }

    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[tokio::test]
    async fn test_cached_allow_all_entry_allows_without_fetch() {
        // A cached negative entry must be served from the cache — the URL used
        // points at a host with no routes, so any fetch attempt would fail
        // (and be observable via timing); correctness here is enforced by the
        // entry being served synchronously from the map.
        let fetcher = RobotsFetcher::new(Profile::Chrome145, 1).expect("client should build");
        fetcher.cache.insert(
            "dead.example".to_string(),
            init_cache_cell(RobotsCacheEntry::AllowAll),
        );

        assert!(
            fetcher
                .is_allowed("http://dead.example/anything", "dead.example")
                .await
        );
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

// ============================================================================
// Task 5.1 memory probe — robots cache growth (BEFORE numbers).
// ============================================================================
#[cfg(test)]
mod memory_probe_tests {
    use super::*;
    use crate::infrastructure::observability::memory_probe;

    #[test]
    fn probe_robots_cache_growth_5k_hosts() {
        const N: usize = 5_000;
        let cache = new_robots_cache();
        let before = memory_probe::rss_bytes();

        for i in 0..N {
            // AllowAll is the real cached fail-open state for hosts without
            // robots.txt; Rules entries are strictly larger so this is a
            // conservative lower bound per host.
            cache.insert(
                format!("probe-{i}.example.com"),
                init_cache_cell(RobotsCacheEntry::AllowAll),
            );
        }

        let after = memory_probe::rss_bytes();
        assert_eq!(cache.len(), N);
        memory_probe::append_report(
            "BEFORE — robots cache",
            &format!(
                "entries={} (AllowAll lower bound) rss_before={} rss_after={} delta={}",
                cache.len(),
                memory_probe::fmt_rss(before),
                memory_probe::fmt_rss(after),
                memory_probe::fmt_rss(after.and_then(|a| before.map(|b| a.saturating_sub(b)))),
            ),
        );
    }
}

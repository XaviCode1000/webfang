//! Site configuration with builder pattern
//!
//! Configuration for crawling a specific site.

use std::num::NonZeroUsize;

use url::Url;
use wreq_util::Profile;

use crate::domain::budget::BudgetOverrides;
use crate::domain::pattern_matching::matches_pattern;
use crate::domain::ValidUrl;

/// Default crawl concurrency (3) as a non-zero value — the literal is
/// checked at compile time; `concurrency: 0` is unrepresentable (#1132).
const fn default_concurrency() -> NonZeroUsize {
    match NonZeroUsize::new(3) {
        Some(v) => v,
        None => panic!("default concurrency literal is non-zero"),
    }
}

/// Sitemap discovery mode as a type-state (#1162, resto de #1132; adopted #1190).
///
/// The single home of the `use_sitemap: bool` + `sitemap_url: Option<String>`
/// pair — where `false + Some(..)` was silently ignored and
/// `true + Some("not-a-url")` travelled unchecked into `resolve_sitemap_url`
/// (an explicit URL passed through without parsing, so it failed late at
/// fetch/parse time). The pair is deleted: build the mode with
/// [`SitemapConfig::resolve`] (which parses the explicit URL NOW, at the
/// domain boundary) and read it through [`CrawlerConfig::sitemap_config`].
///
/// Not to be confused with the sitemap-*parser* `SitemapConfig`
/// (pagination/batch knobs in `crate::domain::crawler_port`): this one is
/// the domain discovery-mode decision.
///
/// `pub(crate)` on purpose: the type lives in the private `site::crawler_config`
/// module, so a wider visibility would leak through `CrawlerConfig`'s public
/// interface (`private_interfaces`). Intra-crate consumers resolve it from
/// the projection (CLI) or match on it from the config (discovery).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SitemapConfig {
    /// Sitemap discovery off; the crawler uses DOM link extraction.
    Disabled,
    /// Sitemap discovery on; `url` is an explicitly configured sitemap
    /// ([`ValidUrl`] — scheme-hardened, credentials stripped) or `None` for
    /// auto-discovery from the seed.
    Enabled { url: Option<ValidUrl> },
}

impl SitemapConfig {
    /// Collapse the raw pair into the validated mode.
    ///
    /// An explicit URL always implies intent (mirrors the CLI preflight
    /// coercion `sitemap_url.is_some() → use_sitemap = true`): it is parsed
    /// NOW via [`ValidUrl::parse`] instead of failing late in the crawl.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::InvalidUrl`](crate::ScraperError::InvalidUrl)
    /// (mensaje en español) when the explicit URL is not a valid http(s) URL.
    pub(crate) fn resolve(
        use_sitemap: bool,
        sitemap_url: Option<&str>,
    ) -> crate::error::Result<Self> {
        match (use_sitemap, sitemap_url) {
            (false, None) => Ok(Self::Disabled),
            (_, Some(raw)) => {
                let url = ValidUrl::parse(raw).map_err(|e| {
                    crate::ScraperError::invalid_url(format!(
                        "URL de sitemap inválida {raw:?}: {e}"
                    ))
                })?;
                Ok(Self::Enabled { url: Some(url) })
            },
            (true, None) => Ok(Self::Enabled { url: None }),
        }
    }

    /// Whether sitemap discovery is on.
    #[must_use]
    pub(crate) fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }
}

/// Crawler configuration with builder pattern
///
/// Following **api-builder**: Provides fluent builder API.
/// Following **api-non-exhaustive**: Can evolve without breaking changes.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CrawlerConfig {
    /// Seed URL to start crawling from
    pub seed_url: Url,
    /// Maximum depth to crawl (0 = only seed)
    pub max_depth: u8,
    /// Maximum number of pages to crawl
    pub max_pages: usize,
    /// URL patterns to include (glob-style)
    pub include_patterns: Vec<String>,
    /// URL patterns to exclude (glob-style)
    pub exclude_patterns: Vec<String>,
    /// Concurrency level (number of parallel requests).
    ///
    /// `NonZeroUsize` makes a zero spawn bound — which previously hung the
    /// crawl in `buffer_unordered(0)` — unrepresentable (#1132).
    pub concurrency: NonZeroUsize,
    /// Delay between requests in milliseconds (rate limiting)
    pub delay_ms: u64,
    /// User agent string
    pub user_agent: String,
    /// Timeout for each request in seconds
    pub timeout_secs: u64,
    /// Sitemap discovery mode (FASE 3), resolved at the projection boundary
    /// (#1190): the raw `use_sitemap: bool` + `sitemap_url: Option<String>`
    /// pair is deleted — `false + Some(..)` can no longer be named, and an
    /// explicit URL is parsed once via [`SitemapConfig::resolve`] instead of
    /// failing late at fetch time. Private because the mode type is
    /// `pub(crate)`; read it through [`CrawlerConfig::sitemap_config`].
    sitemap: SitemapConfig,
    /// Skip robots.txt enforcement.
    pub ignore_robots: bool,
    /// TLS/HTTP2 fingerprint emulation preset applied to the crawl HTTP client.
    ///
    /// Threaded from the CLI `--h2-profile` value. Defaults to
    /// [`Profile::Chrome145`], preserving the historical crawl fingerprint.
    pub tls_emulation: Profile,
    /// Operator-level budget overrides (design D4) forwarded into the crawl
    /// Engine. `Engine::new` feeds them to `BudgetModel::build`, so an explicit
    /// `--concurrency` / `--rate-limit-burst` reaches the scheduler spawn bound
    /// and rate-limiter burst instead of being silently dropped (bug R2-1).
    /// The default reproduces the auto-derived numbers exactly.
    pub budget_overrides: BudgetOverrides,
}

impl CrawlerConfig {
    /// Create a new config with seed URL
    ///
    /// Following **api-builder**: Returns builder for fluent configuration.
    pub fn builder(seed_url: Url) -> CrawlerConfigBuilder {
        CrawlerConfigBuilder::new(seed_url)
    }

    /// Create a new config with default values
    pub fn new(seed_url: Url) -> Self {
        Self {
            seed_url,
            max_depth: 3,
            max_pages: 100,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            concurrency: default_concurrency(), // Hardware-aware: nproc - 1 for 4C CPU
            delay_ms: 500,                      // Hardware-aware: 500ms for HDD
            user_agent: "webfang/2.0.0 (High-Performance Extraction)".to_string(),
            timeout_secs: 30,
            sitemap: SitemapConfig::Disabled,
            ignore_robots: false,
            tls_emulation: Profile::Chrome145,
            budget_overrides: BudgetOverrides::default(),
        }
    }

    /// Read the resolved sitemap mode (#1190).
    ///
    /// Infallible: the config stores the validated [`SitemapConfig`]
    /// directly — parsing happens once at the projection boundary
    /// ([`SitemapConfig::resolve`]), never here.
    #[must_use]
    pub(crate) fn sitemap_config(&self) -> &SitemapConfig {
        &self.sitemap
    }

    /// Check if a URL matches the include patterns
    #[inline]
    #[must_use]
    pub fn matches_include(&self, url: &str) -> bool {
        if self.include_patterns.is_empty() {
            return true;
        }
        self.include_patterns
            .iter()
            .any(|pattern| matches_pattern(url, pattern))
    }

    /// Check if a URL matches the exclude patterns
    #[inline]
    #[must_use]
    pub fn matches_exclude(&self, url: &str) -> bool {
        self.exclude_patterns
            .iter()
            .any(|pattern| matches_pattern(url, pattern))
    }
}

/// Builder for CrawlerConfig
///
/// Following **api-builder** and **api-must-use**.
#[derive(Debug)]
#[must_use]
pub struct CrawlerConfigBuilder {
    seed_url: Url,
    max_depth: u8,
    max_pages: usize,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
    concurrency: NonZeroUsize,
    delay_ms: u64,
    user_agent: String,
    timeout_secs: u64,
    sitemap: SitemapConfig,
    ignore_robots: bool,
    tls_emulation: Profile,
    budget_overrides: BudgetOverrides,
}

impl CrawlerConfigBuilder {
    /// Create a new builder with seed URL
    pub fn new(seed_url: Url) -> Self {
        Self {
            seed_url,
            max_depth: 3,
            max_pages: 100,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            concurrency: default_concurrency(),
            delay_ms: 500,
            user_agent: "webfang/2.0.0 (High-Performance Extraction)".to_string(),
            timeout_secs: 30,
            sitemap: SitemapConfig::Disabled,
            ignore_robots: false,
            tls_emulation: Profile::Chrome145,
            budget_overrides: BudgetOverrides::default(),
        }
    }

    /// Set maximum crawl depth
    pub fn max_depth(mut self, depth: u8) -> Self {
        self.max_depth = depth;
        self
    }

    /// Set maximum number of pages
    pub fn max_pages(mut self, pages: usize) -> Self {
        self.max_pages = pages;
        self
    }

    /// Add an include pattern
    pub fn include_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.include_patterns.push(pattern.into());
        self
    }

    /// Add multiple include patterns
    pub fn include_patterns(mut self, patterns: Vec<String>) -> Self {
        self.include_patterns.extend(patterns);
        self
    }

    /// Add an exclude pattern
    pub fn exclude_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.exclude_patterns.push(pattern.into());
        self
    }

    /// Add multiple exclude patterns
    pub fn exclude_patterns(mut self, patterns: Vec<String>) -> Self {
        self.exclude_patterns.extend(patterns);
        self
    }

    /// Set concurrency level.
    ///
    /// The parameter is a [`NonZeroUsize`]: zero — the value that used to
    /// produce a dead `buffer_unordered(0)` spawn bound — cannot even be
    /// named here (#1132).
    pub fn concurrency(mut self, level: NonZeroUsize) -> Self {
        self.concurrency = level;
        self
    }

    /// Set delay between requests in milliseconds
    pub fn delay_ms(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }

    /// Set user agent string
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    /// Set request timeout in seconds
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set the sitemap discovery mode, already resolved through
    /// [`SitemapConfig::resolve`] (#1190).
    ///
    /// `pub(crate)` like the mode itself: the raw bool+Option pair is
    /// deleted, so there is no `use_sitemap`/`sitemap_url` setter to reach
    /// for — resolve first, then set the validated mode (or use
    /// [`Self::auto_sitemap`] for URL-less discovery).
    pub(crate) fn sitemap(mut self, sitemap: SitemapConfig) -> Self {
        self.sitemap = sitemap;
        self
    }

    /// Enable sitemap auto-discovery (no explicit URL).
    ///
    /// Fully `pub` (unlike `Self::sitemap`): the signature names no
    /// `pub(crate)` type, so external adapters can enable URL-less discovery
    /// without resolving the boundary themselves.
    pub fn auto_sitemap(mut self) -> Self {
        self.sitemap = SitemapConfig::Enabled { url: None };
        self
    }

    /// Skip robots.txt enforcement.
    pub fn ignore_robots(mut self, ignore: bool) -> Self {
        self.ignore_robots = ignore;
        self
    }

    /// Set the TLS/HTTP2 fingerprint emulation preset.
    pub fn tls_emulation(mut self, profile: Profile) -> Self {
        self.tls_emulation = profile;
        self
    }

    /// Set the operator-level budget overrides forwarded into the Engine.
    pub fn budget_overrides(mut self, overrides: BudgetOverrides) -> Self {
        self.budget_overrides = overrides;
        self
    }

    /// Build the final [`CrawlerConfig`], consuming this builder.
    ///
    /// No coercion lives here anymore (#1190): the sitemap mode arrives
    /// already resolved — `SitemapConfig::resolve` is the single home of
    /// the `Some(url) implies intent` rule.
    #[must_use]
    pub fn build(self) -> CrawlerConfig {
        CrawlerConfig {
            seed_url: self.seed_url,
            max_depth: self.max_depth,
            max_pages: self.max_pages.max(1),
            include_patterns: self.include_patterns,
            exclude_patterns: self.exclude_patterns,
            concurrency: self.concurrency,
            delay_ms: self.delay_ms,
            user_agent: self.user_agent,
            timeout_secs: self.timeout_secs.max(1),
            sitemap: self.sitemap,
            ignore_robots: self.ignore_robots,
            tls_emulation: self.tls_emulation,
            budget_overrides: self.budget_overrides,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: non-zero literal (the production API is typed on
    /// `NonZeroUsize`, so a raw `0` cannot even be named at the call site).
    fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("test literal is non-zero")
    }

    #[test]
    fn test_crawler_config_builder() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .max_depth(5)
            .max_pages(500)
            .concurrency(nz(5))
            .delay_ms(1000)
            .include_pattern("*.example.com/*".to_string())
            .exclude_pattern("*/admin/*".to_string())
            .build();

        assert_eq!(config.max_depth, 5);
        assert_eq!(config.max_pages, 500);
        assert_eq!(config.concurrency.get(), 5);
        assert_eq!(config.delay_ms, 1000);
        assert_eq!(config.include_patterns.len(), 1);
        assert_eq!(config.exclude_patterns.len(), 1);
    }

    // -- Default values for all fields --

    #[test]
    fn test_all_defaults() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::new(seed);

        assert_eq!(config.max_depth, 3);
        assert_eq!(config.max_pages, 100);
        assert_eq!(config.concurrency.get(), 3);
        assert_eq!(config.delay_ms, 500);
        assert_eq!(
            config.user_agent,
            "webfang/2.0.0 (High-Performance Extraction)"
        );
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.sitemap_config(), &SitemapConfig::Disabled);
        assert!(!config.ignore_robots);
        assert_eq!(config.tls_emulation, Profile::Chrome145);
        assert!(config.include_patterns.is_empty());
        assert!(config.exclude_patterns.is_empty());
    }

    // -- matches_include tests --

    #[test]
    fn matches_include_empty_patterns_returns_true() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::new(seed);
        assert!(config.matches_include("https://example.com/anything"));
    }

    #[test]
    fn matches_include_host_pattern() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .include_pattern("example.com".to_string())
            .build();

        assert!(config.matches_include("https://example.com/page"));
        assert!(!config.matches_include("https://other.com/page"));
    }

    #[test]
    fn matches_include_path_pattern() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .include_pattern("/blog/*".to_string())
            .build();

        assert!(config.matches_include("https://example.com/blog/post"));
        assert!(!config.matches_include("https://example.com/about"));
    }

    #[test]
    fn matches_include_subdomain_wildcard() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .include_pattern("*.example.com/*".to_string())
            .build();

        assert!(config.matches_include("https://blog.example.com/post"));
        assert!(!config.matches_include("https://evil.com/page"));
    }

    #[test]
    fn matches_include_multiple_patterns() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .include_pattern("/blog/*".to_string())
            .include_pattern("/docs/*".to_string())
            .build();

        assert!(config.matches_include("https://example.com/blog/post"));
        assert!(config.matches_include("https://example.com/docs/guide"));
        assert!(!config.matches_include("https://example.com/about"));
    }

    // -- matches_exclude tests --

    #[test]
    fn matches_exclude_empty_patterns_returns_false() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::new(seed);
        assert!(!config.matches_exclude("https://example.com/anything"));
    }

    #[test]
    fn matches_exclude_path_pattern() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .exclude_pattern("/admin/*".to_string())
            .build();

        assert!(config.matches_exclude("https://example.com/admin/settings"));
        assert!(!config.matches_exclude("https://example.com/blog/post"));
    }

    #[test]
    fn matches_exclude_host_pattern() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .exclude_pattern("cdn.example.com".to_string())
            .build();

        assert!(config.matches_exclude("https://cdn.example.com/static.js"));
        assert!(!config.matches_exclude("https://example.com/page"));
    }

    #[test]
    fn matches_exclude_multiple_patterns() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .exclude_pattern("/admin/*".to_string())
            .exclude_pattern("/api/*".to_string())
            .build();

        assert!(config.matches_exclude("https://example.com/admin/users"));
        assert!(config.matches_exclude("https://example.com/api/v1/data"));
        assert!(!config.matches_exclude("https://example.com/blog/post"));
    }

    // -- Builder with all options --

    #[test]
    fn builder_all_options() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed.clone())
            .max_depth(10)
            .max_pages(1000)
            .concurrency(nz(8))
            .delay_ms(200)
            .user_agent("custom-bot/1.0".to_string())
            .timeout_secs(60)
            .sitemap(
                SitemapConfig::resolve(true, Some("https://example.com/sitemap.xml"))
                    .expect("test URL resolves"),
            )
            .ignore_robots(true)
            .tls_emulation(Profile::Chrome131)
            .include_pattern("/blog/*".to_string())
            .exclude_pattern("/admin/*".to_string())
            .build();

        assert_eq!(config.seed_url, seed);
        assert_eq!(config.max_depth, 10);
        assert_eq!(config.max_pages, 1000);
        assert_eq!(config.concurrency.get(), 8);
        assert_eq!(config.delay_ms, 200);
        assert_eq!(config.user_agent, "custom-bot/1.0");
        assert_eq!(config.timeout_secs, 60);
        match config.sitemap_config() {
            SitemapConfig::Enabled { url } => assert_eq!(
                url.as_ref().map(ValidUrl::as_str),
                Some("https://example.com/sitemap.xml")
            ),
            SitemapConfig::Disabled => panic!("explicit mode must enable"),
        }
        assert!(config.ignore_robots);
        assert_eq!(config.tls_emulation, Profile::Chrome131);
        assert_eq!(config.include_patterns.len(), 1);
        assert_eq!(config.exclude_patterns.len(), 1);
    }

    #[test]
    fn builder_tls_emulation_defaults_to_chrome145_and_is_settable() {
        let seed = Url::parse("https://example.com").unwrap();

        let default_config = CrawlerConfig::builder(seed.clone()).build();
        assert_eq!(default_config.tls_emulation, Profile::Chrome145);

        let custom_config = CrawlerConfig::builder(seed)
            .tls_emulation(Profile::Firefox135)
            .build();
        assert_eq!(custom_config.tls_emulation, Profile::Firefox135);
    }

    #[test]
    fn builder_include_patterns_bulk() {
        let seed = Url::parse("https://example.com").unwrap();
        let patterns = vec![
            "/blog/*".to_string(),
            "/docs/*".to_string(),
            "/guides/*".to_string(),
        ];
        let config = CrawlerConfig::builder(seed)
            .include_patterns(patterns)
            .build();

        assert_eq!(config.include_patterns.len(), 3);
    }

    #[test]
    fn builder_exclude_patterns_bulk() {
        let seed = Url::parse("https://example.com").unwrap();
        let patterns = vec!["/admin/*".to_string(), "/internal/*".to_string()];
        let config = CrawlerConfig::builder(seed)
            .exclude_patterns(patterns)
            .build();

        assert_eq!(config.exclude_patterns.len(), 2);
    }

    // -- CrawlerConfig Debug --

    #[test]
    fn config_debug_output() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::new(seed);
        let dbg = format!("{config:?}");
        assert!(dbg.contains("CrawlerConfig"));
        assert!(dbg.contains("example.com"));
    }

    // -- CrawlerConfig Clone --

    #[test]
    fn config_clone_produces_equal_copy() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .max_depth(7)
            .max_pages(200)
            .build();
        let cloned = config.clone();

        assert_eq!(config.max_depth, cloned.max_depth);
        assert_eq!(config.max_pages, cloned.max_pages);
        assert_eq!(config.seed_url, cloned.seed_url);
    }

    #[test]
    fn builder_clamps_timeout_secs_to_minimum_one() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed).timeout_secs(0).build();
        assert_eq!(config.timeout_secs, 1);
    }

    #[test]
    fn builder_clamps_max_pages_to_minimum_one() {
        // Bug #780: max_pages 0 reached ResultsCollector::new(0) →
        // mpsc::channel(0) panic (SIGABRT). The builder is the last line of
        // defense for programmatic/config-file paths that bypass CLI parsing.
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed).max_pages(0).build();
        assert_eq!(config.max_pages, 1);
    }

    /// #1132 reproduction: `concurrency: 0` used to be representable —
    /// `CrawlerConfig::builder(seed).concurrency(0).build()` compiled and
    /// produced a config whose zero spawn bound hung the crawl (no clamp
    /// existed for this field, unlike max_pages/timeout_secs). The field is
    /// now `NonZeroUsize`: the only way to name the value is through a
    /// fallible constructor, and 0 is rejected there — the invalid state
    /// cannot reach `build()` at all.
    ///
    /// #1190: the raw pair these tests used to pin is deleted — the
    /// rejection contract moved end-to-end into the discovery entries
    /// (`sitemap_discovery` tests drive the invalid/non-http cases
    /// through `crawl_with_sitemap`, the projection cases through the
    /// orchestrator build fns). What stays here is the replacement
    /// surface: absence of the pair is enforced at compile time (any
    /// reader of the deleted fields or setters fails to build), and
    /// this test pins that every mode builds only from the boundary.
    #[test]
    fn issue_1190_sitemap_modes_build_only_from_boundary() {
        let seed = Url::parse("https://example.com").unwrap();
        let off = CrawlerConfig::builder(seed.clone()).build();
        assert_eq!(off.sitemap_config(), &SitemapConfig::Disabled);
        assert!(!off.sitemap_config().is_enabled());

        let explicit = CrawlerConfig::builder(seed.clone())
            .sitemap(
                SitemapConfig::resolve(true, Some("https://example.com/sitemap.xml"))
                    .expect("test URL resolves"),
            )
            .build();
        assert!(
            matches!(
                explicit.sitemap_config(),
                SitemapConfig::Enabled { url: Some(_) }
            ),
            "explicit resolve must enable with a URL"
        );

        let auto = CrawlerConfig::builder(seed).auto_sitemap().build();
        assert!(
            matches!(auto.sitemap_config(), SitemapConfig::Enabled { url: None }),
            "auto_sitemap must enable without a URL"
        );
    }

    #[test]
    fn issue_1132_zero_concurrency_is_unrepresentable() {
        assert!(
            NonZeroUsize::new(0).is_none(),
            "0 must not be nameable as a concurrency level"
        );
        // The default is non-zero by construction (compile-time literal).
        let seed = Url::parse("https://example.com").unwrap();
        assert_eq!(CrawlerConfig::new(seed).concurrency.get(), 3);
    }
}

//! CLI discovery phase — turns one seed URL into the final scrape list.
//!
//! Extracted from `cli/orchestrator.rs` as part of the composition-root
//! decomposition (issue #1619, finding F1). Everything between "we have a
//! `CrawlOptions`" and "here are the URLs, the scraper config and the bodies
//! captured on the way" lives here: the sitemap/DOM discovery arms, the seed
//! pattern guard (`plan_urls`), the scraper-config build and the shared
//! crawler-config projection.
//!
//! The module owns no new policy. The sitemap coercion
//! (`resolve_sitemap_projection`) moves with it, since it is the input to
//! every discovery arm here; the persistence-mode resolver and the asset H2
//! profile parser stay in [`crate::cli::orchestrator`], as does the exit-code
//! mapping in [`crate::cli::exit_codes`] — so discovery keeps one
//! implementation of each rule and the fetch guard chain is untouched: this
//! phase performs discovery only and hands off before any page fetch.

use tracing::{info, instrument};

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::{CapturedPage, InMemoryContentSink};
use crate::cli::error::CliExit;
use crate::cli::orchestrator::{parse_asset_h2_profile, resolve_persistence_root};
use crate::cli::parse::parse_asset_naming;
use crate::cli::url_discovery::{discover_urls, discover_urls_unified, DiscoveryRetry};
use crate::domain;
use crate::domain::config::ScraperConfig;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::persistence::PersistenceMode;
use crate::domain::site::SitemapConfig;
use crate::CrawlerConfig;

/// Resolve the CLI-projected sitemap pair into the domain boundary (#1190).
///
/// The single home (with [`SitemapConfig::resolve`]) of the
/// `sitemap_url.is_some() → enabled` coercion: the preflight book, the
/// `webfang_cli` projection, and the builder coercion all collapsed here,
/// so an explicit but invalid URL fails as `CliExit::ConfigError`
/// (Spanish, typed) before any discovery starts.
pub(crate) fn resolve_sitemap_projection(opts: &CrawlOptions) -> Result<SitemapConfig, CliExit> {
    SitemapConfig::resolve(opts.crawl.use_sitemap, opts.crawl.sitemap_url.as_deref())
        .map_err(|e| CliExit::ConfigError(e.to_string()))
}

/// Build a `CrawlerConfig` for URL discovery (shared by dry-run, prepare, and batch).
pub(crate) fn build_crawler_config_for_discovery(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
) -> Result<CrawlerConfig, CliExit> {
    let crawler_config = CrawlerConfig::builder(opts.url.as_url().clone())
        .max_pages(opts.crawl.max_pages)
        .max_depth(opts.crawl.max_depth)
        .include_patterns(opts.crawl.include_patterns.clone())
        .exclude_patterns(opts.crawl.exclude_patterns.clone())
        .ignore_robots(opts.crawl.ignore_robots)
        .sitemap(resolve_sitemap_projection(opts)?)
        .timeout_secs(opts.network.timeout_secs)
        .delay_ms(opts.network.delay_ms)
        // Bug R2-1: recursive URL discovery runs the real crawl Engine, so
        // the operator overrides must ride on the config or the Engine
        // silently re-derives the auto tiers.
        .budget_overrides(opts.budget_overrides)
        .tls_emulation(tls_emulation)
        .build();
    Ok(crawler_config)
}

/// Run sitemap discovery and map its terminal states to `CliExit` (#1439
/// extraction: keeps `prepare_phase` under the `too_many_lines` ratchet).
///
/// "Site has no sitemap" and "sitemap empty" are discovery states, not
/// infrastructure failures (#695): exit 2 lets automation distinguish them
/// from a real network outage (exit 69). Exit 2 also fires when the sitemap —
/// the source of truth in this mode — yields zero URLs.
async fn discover_sitemap_urls(
    crawler_config: &CrawlerConfig,
    opts: &CrawlOptions,
    root_correlation: &domain::CorrelationId,
) -> Result<Vec<url::Url>, CliExit> {
    match discover_urls(crawler_config, opts, root_correlation).await {
        Err(crate::error::ScraperError::SitemapNotFound(_)) => Err(CliExit::EmptyDiscovery(
            "No URLs discovered: sitemap not found".into(),
        )),
        Err(crate::error::ScraperError::SitemapEmpty) => Err(CliExit::EmptyDiscovery(
            "No URLs discovered: sitemap is empty".into(),
        )),
        Err(e) => Err(CliExit::NetworkError(format!("URL discovery failed: {e}"))),
        Ok(urls) if urls.is_empty() => Err(CliExit::EmptyDiscovery(
            "No URLs discovered from sitemaps".into(),
        )),
        Ok(urls) => Ok(urls),
    }
}

/// Prepare scraper config and discover URLs.
///
/// Returns the initial `ScraperConfig` (before asset/download wiring) and
/// the list of URLs to scrape.  On discovery failure, returns the
/// appropriate `CliExit` error.
pub(crate) async fn prepare_phase(
    opts: &CrawlOptions,
    persistence_mode: &PersistenceMode,
    root_correlation: &domain::CorrelationId,
) -> Result<PrepareResult, CliExit> {
    // Discovery-captured bodies (F-05, #1229): filled by the DOM branch
    // below, reused by the scrape phase instead of refetching.
    let mut captured_pages: Vec<CapturedPage> = Vec::new();
    let urls_to_scrape = if opts.crawl.single_page {
        // F-35 (#1216): single-page mode never runs discovery, so the seed
        // pattern guard needs a patterns-only config — no TLS/sitemap
        // projection involved, keeping `--h2-profile` semantics unchanged here.
        let seed_guard = CrawlerConfig::builder(opts.url.as_url().clone())
            .include_patterns(opts.crawl.include_patterns.clone())
            .exclude_patterns(opts.crawl.exclude_patterns.clone())
            .build();
        // Short local keeps the arg-span under rustfmt's fn_call_width,
        // so the call stays single-line and prepare_phase under the
        // clippy too_many_lines ratchet ceiling (#516).
        let seed = opts.url.as_url().clone();
        plan_urls(true, false, seed, Vec::new(), &seed_guard)
    } else {
        // Honor `--h2-profile` for URL discovery (#312): an unknown profile is a
        // config error (exit 78), consistent with the scrape and batch phases.
        let tls_emulation = HttpClientConfig::profile_from_name(&opts.network.h2_profile)
            .map_err(|e| CliExit::ConfigError(e.to_string()))?;

        let crawler_config = build_crawler_config_for_discovery(opts, tls_emulation)?;

        // Sitemap mode is the source of truth (depth-agnostic XML), so keep the
        // existing single-pass sitemap discovery. DOM mode must run the recursive
        // crawl Engine so `--max-depth` is honored (bug #651): the legacy
        // `discover_urls_single_fetch` path did one fetch and silently ignored depth.
        let discovered_urls = if opts.crawl.use_sitemap {
            discover_sitemap_urls(&crawler_config, opts, root_correlation).await?
        } else {
            // Recursive BFS discovery respects max_depth/max_pages/robots/
            // patterns; the existing scrape_phase + export_phase still own
            // content extraction and on-disk output.
            let (urls, pages) = discover_dom_with_capture(
                &crawler_config,
                opts,
                persistence_mode,
                root_correlation,
            )
            .await?;
            captured_pages = pages;
            urls
        };

        plan_urls(
            false,
            opts.crawl.use_sitemap,
            opts.url.as_url().clone(),
            discovered_urls,
            &crawler_config,
        )
    };

    // Budget model built ONCE at flow entry (design D4): operator overrides
    // plus the canonical detector seam feed every downstream bound.
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );

    let mut scraper_config = ScraperConfig::default()
        .with_output_dir(resolve_persistence_root(opts))
        // Scraper + asset-download bounds derive from the model's Operation.crawl
        // and Asset tiers (task 2.5b); explicit flags arrive via BudgetOverrides.
        .with_scraper_concurrency(budget.crawl().get())
        .with_max_pages(opts.crawl.max_pages)
        .with_selector(opts.crawl.selector.clone())
        .with_ignore_waf(opts.crawl.ignore_waf)
        .with_dom_preprune(opts.crawl.dom_preprune);

    if opts.network.download_images {
        scraper_config = scraper_config.with_images();
    }
    if opts.network.download_documents {
        scraper_config = scraper_config.with_documents();
    }

    // Wire asset download config from CLI args
    // NOTE: crawl include/exclude patterns are intentionally NOT forwarded to
    // asset config — assets have their own filter scope (#639).
    scraper_config =
        scraper_config.with_asset_h2_profile(parse_asset_h2_profile(&opts.network.h2_profile));
    scraper_config = scraper_config.with_asset_naming(parse_asset_naming(&opts.asset_naming));
    scraper_config = scraper_config.with_download_concurrency(budget.asset().get());
    // Effective asset-tier bound logged at INFO so operators (and behavioral
    // tests) can verify an explicit `--download-concurrency` reached this
    // enforcement site (#897 item 5). Structured field — never interpolate
    // values into the message (m1).
    info!(
        asset_concurrency = budget.asset().get(),
        "Asset downloads wired"
    );
    scraper_config = scraper_config.with_max_file_size(opts.network.max_file_size);
    scraper_config = scraper_config.with_download_timeout(opts.network.download_timeout_secs);

    // Create shared Downloader once for connection pooling across all page scrapes.
    // Q3 MEASURE FIRST: the dedup cache is the only structure whose measured
    // growth crossed the 50 MB materiality line; its capacity derives from the
    // Asset tier like every other budget-model bound.
    // Single graph (#1149): the ephemeral asset downloader is built through
    // the `Container` factory — fresh and bounded per run, never the MCP
    // server's long-lived shared downloader (#1120).
    let shared_downloader = if scraper_config.has_downloads() {
        match crate::application::container::Container::build_ephemeral_asset_downloader(
            &scraper_config,
            budget.asset().get(),
        ) {
            Ok(dl) => Some(std::sync::Arc::new(dl)),
            Err(e) => {
                return Err(CliExit::IoError(format!(
                    "No se pudo crear el descargador de assets: {e}"
                )));
            },
        }
    } else {
        None
    };

    Ok(PrepareResult {
        urls_to_scrape,
        scraper_config,
        shared_downloader,
        captured_pages,
    })
}

/// Run unified DOM discovery with a bounded capture sink (F-05, #1229).
///
/// Returns the discovered URLs plus the bodies captured during discovery
/// for the scrape phase to reuse instead of refetching — one HTTP request
/// per page. Unified DOM discovery (F-14, #1232) runs the recursive Engine;
/// F-35 (#1216): the config is cloned because `plan_urls` reuses it for
/// the seed pattern guard. `root_correlation` (#1439) is the CLI run-root,
/// propagated verbatim so the Engine and the CLI share one trace identity.
///
/// # Errors
///
/// Returns [`CliExit::NetworkError`] when the Engine discovery fails.
async fn discover_dom_with_capture(
    crawler_config: &CrawlerConfig,
    opts: &CrawlOptions,
    persistence_mode: &PersistenceMode,
    root_correlation: &domain::CorrelationId,
) -> Result<(Vec<url::Url>, Vec<CapturedPage>), CliExit> {
    let capture_sink = std::sync::Arc::new(InMemoryContentSink::new());
    let cfg = crawler_config.clone();
    // dry-run-fail-fast: the real DOM path keeps full operator retry
    // semantics (`Operator`) — only the dry-run preview runs fail-fast.
    match discover_urls_unified(
        cfg,
        opts,
        persistence_mode,
        Some(capture_sink),
        root_correlation,
        DiscoveryRetry::Operator,
    )
    .await
    {
        Err(e) => Err(CliExit::NetworkError(format!("URL discovery failed: {e}"))),
        Ok(output) => Ok((output.urls, output.pages)),
    }
}

pub(crate) struct PrepareResult {
    pub(crate) urls_to_scrape: Vec<url::Url>,
    pub(crate) scraper_config: ScraperConfig,
    pub(crate) shared_downloader: Option<std::sync::Arc<crate::adapters::downloader::Downloader>>,
    /// Bodies captured during DOM discovery (F-05, #1229): the scrape phase
    /// reuses them instead of refetching. Empty for single-page, sitemap,
    /// and dry-run shapes.
    pub(crate) captured_pages: Vec<CapturedPage>,
}

/// Plan the final scrape list from discovery output.
///
/// F-35 (#1216): `--include-pattern` / `--exclude-pattern` apply to the seed
/// URL itself, not just discovered pages. The CLI default path uses the crawl
/// Engine only for discovery and then scrapes this planned list directly, so
/// the Engine's own seed guard (`engine.rs`, #634) is bypassed here — the
/// planning boundary must enforce the same
/// [`crate::application::url_filter::is_allowed`] predicate. The Engine guard
/// stays intact for direct Engine consumers (batch, MCP) and for discovery
/// filtering: the predicate lives in exactly one function, enforced at both
/// boundaries (defense in depth, not duplicated logic). Dropping the
/// unconditional insert instead was rejected: the `single_page` path never
/// runs the Engine, and `plan_urls` cannot assume every discovery backend
/// returns the seed, so an explicit guard keeps this boundary total.
#[instrument(
    skip(seed_url, discovered_urls, crawler_config),
    fields(seed_url = %seed_url)
)]
fn plan_urls(
    single_page: bool,
    use_sitemap: bool,
    seed_url: url::Url,
    discovered_urls: Vec<url::Url>,
    crawler_config: &CrawlerConfig,
) -> Vec<url::Url> {
    // Single source of truth for "may the seed be scraped" (F-35, #1216).
    let seed_allowed =
        crate::application::url_filter::is_allowed(seed_url.as_str(), crawler_config);
    if !seed_allowed {
        info!(
            seed_url = %seed_url,
            "Seed URL excluded by pattern filters — it will not be scraped"
        );
    }
    if single_page {
        if seed_allowed {
            vec![seed_url]
        } else {
            Vec::new()
        }
    } else if use_sitemap {
        // Sitemap is the source of truth — do not inject the seed URL.
        // Discovery already applied the pattern filters.
        discovered_urls
    } else {
        // DOM discovery: re-inject the seed ONLY when the patterns allow it,
        // so it gets crawled even when link extraction only returns children.
        let mut urls = discovered_urls;
        if seed_allowed {
            if !urls.contains(&seed_url) {
                urls.insert(0, seed_url);
            }
        } else {
            // Defensive: strip the seed if a discovery backend returned it
            // despite the filters — an excluded seed must never be scraped.
            urls.retain(|url| *url != seed_url);
        }
        urls
    }
}

#[cfg(test)]
mod tests {
    use super::{build_crawler_config_for_discovery, plan_urls};
    use crate::application::crawl_options::CrawlOptions;
    use crate::cli::error::CliExit;
    use crate::CrawlerConfig;

    #[test]
    fn discovery_config_propagates_budget_overrides() {
        // Bug R2-1: recursive URL discovery runs the real crawl Engine via
        // crawl_site; the operator overrides staged on CrawlOptions must be
        // carried onto the config so the Engine honors them.
        let mut opts = CrawlOptions::default();
        opts.budget_overrides.crawl = crate::domain::budget::tiers::CrawlConcurrency::new(6).ok();
        opts.budget_overrides.rate_burst = crate::domain::budget::tiers::BurstPermits::new(11).ok();

        let config = build_crawler_config_for_discovery(&opts, wreq_util::Profile::Chrome145)
            .expect("valid test projection must build");

        assert_eq!(
            config.budget_overrides.crawl.map(|c| c.get()),
            Some(6),
            "explicit --concurrency must reach the discovery Engine"
        );
        assert_eq!(
            config.budget_overrides.rate_burst.map(|b| b.get()),
            Some(11),
            "explicit --rate-limit-burst must reach the discovery Engine"
        );
    }

    #[test]
    fn discovery_config_invalid_sitemap_url_is_config_error() {
        // End-to-end projection rejection: an explicit but invalid URL
        // fails HERE (Spanish, typed) instead of travelling into
        // discovery and failing late at fetch/parse time.
        let mut opts = CrawlOptions::default();
        opts.crawl.use_sitemap = true;
        opts.crawl.sitemap_url = Some("not-a-url".to_string());

        let err = build_crawler_config_for_discovery(&opts, wreq_util::Profile::Chrome145)
            .expect_err("invalid sitemap URL must fail the projection");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("sitemap") && msg.contains("inválida"),
                "rejection must name the sitemap URL in Spanish, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    #[test]
    fn discovery_config_explicit_url_implies_enabled() {
        // The `Some(url) implies intent` coercion lives in the single
        // domain rule now: `false + Some(valid)` projects to enabled.
        let mut opts = CrawlOptions::default();
        opts.crawl.sitemap_url = Some("https://example.com/sitemap.xml".to_string());

        let config = build_crawler_config_for_discovery(&opts, wreq_util::Profile::Chrome145)
            .expect("valid sitemap URL must project");
        assert!(
            config.sitemap_config().is_enabled(),
            "explicit URL must imply intent through the projection"
        );
    }

    fn permissive_guard(seed: &url::Url) -> CrawlerConfig {
        CrawlerConfig::new(seed.clone())
    }

    #[test]
    fn plan_urls_single_page_returns_seed_only() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let discovered = vec![
            url::Url::parse("https://example.com/about").unwrap(),
            url::Url::parse("https://example.com/blog").unwrap(),
        ];
        let guard = permissive_guard(&seed);

        let result = plan_urls(true, false, seed.clone(), discovered, &guard);

        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_single_page_excluded_seed_yields_empty() {
        // F-35 (#1216): single-page mode never runs the Engine, so the
        // patterns-only guard must still refuse an excluded seed.
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let guard = CrawlerConfig::builder(seed.clone())
            .exclude_pattern("/article")
            .build();

        let result = plan_urls(true, false, seed, Vec::new(), &guard);

        assert!(
            result.is_empty(),
            "excluded single-page seed must yield zero URLs, got {result:?}"
        );
    }

    #[test]
    fn plan_urls_dom_mode_prepends_seed() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let discovered = vec![
            url::Url::parse("https://example.com/a").unwrap(),
            url::Url::parse("https://example.com/b").unwrap(),
            url::Url::parse("https://example.com/c").unwrap(),
        ];

        let guard = permissive_guard(&seed);
        let result = plan_urls(false, false, seed.clone(), discovered.clone(), &guard);

        // DOM mode: an allowed seed is prepended when absent so it gets scraped.
        let mut expected = vec![seed];
        expected.extend(discovered);
        assert_eq!(result, expected);
    }

    #[test]
    fn plan_urls_sitemap_mode_does_not_prepend_seed() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let discovered = vec![
            url::Url::parse("https://example.com/a").unwrap(),
            url::Url::parse("https://example.com/b").unwrap(),
        ];

        let guard = permissive_guard(&seed);
        let result = plan_urls(false, true, seed, discovered.clone(), &guard);

        // Sitemap mode: the sitemap is the source of truth — seed is NOT injected.
        assert_eq!(result, discovered);
    }

    #[test]
    fn plan_urls_dom_mode_empty_discovered() {
        // F-35 (#1216, INVERTED): the old assertion pinned the bug — "the
        // seed is always included in DOM mode". An excluded seed must NOT
        // be re-injected; an excluded seed with empty discovery yields zero URLs.
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let guard = CrawlerConfig::builder(seed.clone())
            .exclude_pattern("/article")
            .build();

        let result = plan_urls(false, false, seed, Vec::new(), &guard);

        assert!(
            result.is_empty(),
            "excluded seed must yield zero URLs, got {result:?}"
        );
    }

    #[test]
    fn plan_urls_dom_mode_empty_discovered_allowed_seed_still_included() {
        // Companion to the inversion above: an ALLOWED seed with empty
        // discovery is still re-injected (e.g. a link-less article page).
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let guard = permissive_guard(&seed);

        let result = plan_urls(false, false, seed.clone(), Vec::new(), &guard);

        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_dom_mode_include_mismatch_drops_seed() {
        // F-35 (#1216): a seed matching no include-pattern yields zero URLs.
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let guard = CrawlerConfig::builder(seed.clone())
            .include_pattern("/nothing-here/*")
            .build();

        let result = plan_urls(false, false, seed, Vec::new(), &guard);

        assert!(
            result.is_empty(),
            "seed matching no include-pattern must yield zero URLs, got {result:?}"
        );
    }

    #[test]
    fn plan_urls_dom_mode_excluded_seed_stripped_from_discovered() {
        // F-35 (#1216): even if a discovery backend returned the excluded
        // seed, the planning boundary strips it; allowed URLs pass through.
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let child = url::Url::parse("https://example.com/other").unwrap();
        let guard = CrawlerConfig::builder(seed.clone())
            .exclude_pattern("/article")
            .build();

        let result = plan_urls(false, false, seed, vec![child.clone()], &guard);

        assert_eq!(result, vec![child]);
    }

    #[test]
    fn plan_urls_single_page_ignores_many_discovered() {
        let seed = url::Url::parse("https://example.com/only").unwrap();
        let discovered: Vec<_> = (0..100)
            .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
            .collect();
        let guard = permissive_guard(&seed);

        let result = plan_urls(true, false, seed.clone(), discovered, &guard);

        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_dom_mode_preserves_order() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let urls: Vec<_> = (0..10)
            .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
            .collect();

        let guard = permissive_guard(&seed);
        let result = plan_urls(false, false, seed.clone(), urls.clone(), &guard);

        // Discovered order is preserved; an allowed seed is prepended when absent.
        let mut expected = vec![seed];
        expected.extend(urls);
        assert_eq!(result, expected);
    }

    // ===== Asset pattern decoupling tests (#639) =====

    /// Regression test for #639: crawl include/exclude patterns must NOT
    /// be forwarded to asset download config — assets have their own scope.
    #[tokio::test]
    async fn crawl_patterns_not_forwarded_to_asset_config() {
        use crate::application::crawl_options::{CrawlLimits, NetworkOptions};
        use crate::cli::discovery_phase::prepare_phase;

        let url = crate::domain::ValidUrl::parse("https://example.com").expect("valid url");
        let opts = CrawlOptions {
            url,
            crawl: CrawlLimits {
                include_patterns: vec!["/catalogue/*".to_string()],
                exclude_patterns: vec!["/media/*".to_string()],
                single_page: true, // Skip network discovery
                ..Default::default()
            },
            network: NetworkOptions::default(),
            ..Default::default()
        };

        let default_state_dir = crate::cli::scrape_flow::resolve_default_state_dir();
        let persistence_mode = opts.crawl.persistence_mode(&default_state_dir);
        let result = prepare_phase(
            &opts,
            &persistence_mode,
            &crate::domain::CorrelationId::new(),
        )
        .await;
        assert!(
            result.is_ok(),
            "prepare_phase must succeed: {:?}",
            result.err()
        );
        let prepare = result.unwrap();

        assert!(
            prepare.scraper_config.asset_include_patterns.is_empty(),
            "crawl include_patterns must NOT leak into asset config, got: {:?}",
            prepare.scraper_config.asset_include_patterns
        );
        assert!(
            prepare.scraper_config.asset_exclude_patterns.is_empty(),
            "crawl exclude_patterns must NOT leak into asset config, got: {:?}",
            prepare.scraper_config.asset_exclude_patterns
        );
    }
}

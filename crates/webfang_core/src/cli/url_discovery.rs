//! URL discovery logic extracted from orchestrator.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tracing::info;
#[cfg(feature = "ui")]
use tracing::warn;
use url::Url;

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::crawl_site;
use crate::application::discover_urls_for_tui;
use crate::cli::SelectedUrls;
use crate::error::Result as ScraperResult;
use crate::CrawlerConfig;

/// Build the discovery progress spinner, or `None` when quiet is enabled.
fn build_discovery_progress_bar(opts: &CrawlOptions, message: &str) -> Option<ProgressBar> {
    if opts.export.quiet {
        return None;
    }
    let pb = ProgressBar::new_spinner();
    pb.set_draw_target(ProgressDrawTarget::stderr());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    // The spinner template is a hardcoded constant; parsing cannot fail.
    #[allow(clippy::expect_used)]
    let style = ProgressStyle::default_spinner()
        .template("{spinner} {msg}")
        .expect("valid spinner template");
    pb.set_style(style);
    pb.set_message(message.to_owned());
    Some(pb)
}

/// Discover URLs with progress bar.
///
/// Returns `Err` on network/timeout errors instead of silently swallowing them.
pub async fn discover_urls(
    crawler_config: &CrawlerConfig,
    opts: &CrawlOptions,
) -> ScraperResult<Vec<Url>> {
    let discovery_pb = build_discovery_progress_bar(opts, "Discovering URLs...");

    let discovered_urls = match discover_urls_for_tui(opts.url.as_str(), crawler_config).await {
        Ok(urls) => urls,
        Err(e) => {
            // Treat "no URLs found" as empty discovery (technical success),
            // not as a network error. Only propagate real errors (timeouts, etc.).
            let msg = e.to_string();
            if msg.contains("no URLs found") {
                if let Some(pb) = discovery_pb.as_ref() {
                    pb.finish_with_message("No URLs found");
                }
                Vec::new()
            } else {
                if let Some(pb) = discovery_pb.as_ref() {
                    pb.finish_with_message("Discovery failed");
                }
                return Err(e);
            }
        },
    };

    if let Some(pb) = discovery_pb {
        pb.finish_with_message(format!("Found {} URLs", discovered_urls.len()).to_owned());
    }

    Ok(discovered_urls)
}

/// Recursively discover URLs by running the real crawl Engine (BFS).
///
/// The default (non-interactive, non-sitemap) DOM crawl path previously called
/// `discover_urls_for_tui`, which performs a SINGLE fetch and one round of link
/// extraction — so `--max-depth` was silently ignored and every crawl behaved
/// like depth 1 (bug #651). This routes discovery through [`crawl_site`], the
/// same recursive engine the batch and MCP paths use, so `max_depth`,
/// `max_pages`, robots, and include/exclude patterns are all honored.
///
/// The Engine returns a metadata-only `CrawlResult` (the set of fetched URLs);
/// the rich content extraction and on-disk export stay in the CLI's existing
/// `scrape_phase` / `export_phase`, which consume this URL list exactly as the
/// old single-level discovery produced it — so output location and format are
/// unchanged. Interactive TUI selection keeps using `discover_urls_for_tui` at
/// depth 1 and is untouched by this path.
pub async fn discover_urls_recursive(
    crawler_config: CrawlerConfig,
    opts: &CrawlOptions,
) -> ScraperResult<Vec<Url>> {
    let discovery_pb = build_discovery_progress_bar(opts, "Discovering URLs (recursive)...");

    // The Engine itself respects max_depth etc.; map its error to the same
    // error type `discover_urls` used to surface.
    let result = crawl_site(crawler_config).await?;

    let discovered_urls: Vec<Url> = result.urls.into_iter().map(|d| d.url).collect();

    if let Some(pb) = discovery_pb {
        pb.finish_with_message(format!("Found {} URLs", discovered_urls.len()).to_owned());
    }

    Ok(discovered_urls)
}

/// Select URLs via TUI, quick-save, or headless mode.
#[allow(dead_code)] // pub(crate) Phase 0 triage — internal API surface
pub(crate) async fn select_urls(
    discovered_urls: &[Url],
    opts: &CrawlOptions,
    vault_path: &Option<std::path::PathBuf>,
) -> SelectedUrls {
    if opts.export.quick_save && vault_path.is_some() {
        info!("Quick-save mode: bypassing TUI, will save to vault _inbox");
        SelectedUrls::Urls(discovered_urls.to_vec())
    } else if opts.crawl.interactive {
        // Interactive TUI selection lives in the `webfang_tui` crate,
        // which `webfang_core` cannot depend on (cyclic dependency).
        // This code path is currently unreachable from core; fall back to
        // scraping all discovered URLs.
        #[cfg(feature = "ui")]
        {
            warn!(
                "Interactive TUI selector is unavailable from core; using all {} discovered URLs",
                discovered_urls.len()
            );
            SelectedUrls::Urls(discovered_urls.to_vec())
        }
        // When `ui` is OFF, interactive mode falls back to batch (all URLs).
        // Spec S2.3 — no run_selector call without the TUI feature.
        #[cfg(not(feature = "ui"))]
        {
            info!(
                "Interactive mode requested but TUI is unavailable (ui feature off) — using all {} discovered URLs",
                discovered_urls.len()
            );
            SelectedUrls::Urls(discovered_urls.to_vec())
        }
    } else {
        info!(
            "Headless mode: will scrape all {} URLs",
            discovered_urls.len()
        );
        SelectedUrls::Urls(discovered_urls.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // T-2.1: discover_urls returns Result (compile-time + runtime verification)
    #[cfg_attr(
        miri,
        ignore = "btls/wreq FFI (BoringSSL TLS_method) not supported by Miri"
    )]
    #[tokio::test]
    async fn discover_urls_returns_result_type() {
        let seed_url = url::Url::parse("https://localhost:1").unwrap();
        let config = CrawlerConfig::builder(seed_url).build();
        let opts = CrawlOptions {
            url: url::Url::parse("https://localhost:1").unwrap(),
            ..Default::default()
        };

        let result = discover_urls(&config, &opts).await;
        // Should return Err for unreachable host, proving Result return type
        assert!(result.is_err(), "Expected Err for unreachable host");
    }
}

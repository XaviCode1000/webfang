//! URL discovery logic extracted from orchestrator.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tracing::{info, warn};
use url::Url;

use crate::application::crawl_options::CrawlOptions;
use crate::application::discover_urls_for_tui;
use crate::cli::SelectedUrls;
use crate::CrawlerConfig;

/// Discover URLs with progress bar.
pub async fn discover_urls(crawler_config: &CrawlerConfig, opts: &CrawlOptions) -> Vec<Url> {
    let discovery_pb = if !opts.export.quiet {
        let pb = ProgressBar::new_spinner();
        pb.set_draw_target(ProgressDrawTarget::stderr());
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner} {msg}")
                .expect("valid spinner template"),
        );
        pb.set_message("Discovering URLs...");
        Some(pb)
    } else {
        None
    };

    let discovered_urls = match discover_urls_for_tui(opts.url.as_str(), crawler_config).await {
        Ok(urls) => urls,
        Err(e) => {
            if let Some(pb) = discovery_pb.as_ref() {
                pb.finish_with_message("Discovery failed");
            }
            warn!("URL discovery failed: {}", e);
            Vec::new()
        },
    };

    if let Some(pb) = discovery_pb {
        pb.finish_with_message(format!("Found {} URLs", discovered_urls.len()).to_owned());
    }

    discovered_urls
}

/// Select URLs via TUI, quick-save, or headless mode.
pub async fn select_urls(
    discovered_urls: &[Url],
    opts: &CrawlOptions,
    vault_path: &Option<std::path::PathBuf>,
) -> SelectedUrls {
    if opts.export.quick_save && vault_path.is_some() {
        info!("Quick-save mode: bypassing TUI, will save to vault _inbox");
        SelectedUrls::Urls(discovered_urls.to_vec())
    } else if opts.crawl.interactive {
        // Interactive TUI selection lives in the `rust_scraper_tui` crate,
        // which `rust_scraper_core` cannot depend on (cyclic dependency).
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

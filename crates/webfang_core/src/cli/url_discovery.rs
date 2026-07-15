//! URL discovery logic extracted from orchestrator.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tracing::info;
#[cfg(feature = "ui")]
use tracing::warn;
use url::Url;

use crate::application::crawl_options::CrawlOptions;
use crate::application::discover_urls_for_tui;
use crate::cli::SelectedUrls;
use crate::CrawlerConfig;

/// Discover URLs with progress bar.
///
/// Returns `Err` on network/timeout errors instead of silently swallowing them.
pub async fn discover_urls(
    crawler_config: &CrawlerConfig,
    opts: &CrawlOptions,
) -> anyhow::Result<Vec<Url>> {
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

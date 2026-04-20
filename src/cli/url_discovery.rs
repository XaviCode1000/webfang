//! URL discovery logic extracted from orchestrator.

use url::Url;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tracing::{info, warn};

use crate::Args;
use crate::CrawlerConfig;
use crate::application::discover_urls_for_tui;
use crate::cli::preflight;
use crate::cli::SelectedUrls;
use crate::adapters;
use crate::CliExit;

/// Discover URLs with progress bar.
pub async fn discover_urls(crawler_config: &CrawlerConfig, args: &Args) -> Vec<Url> {
    let target_url = args.url.as_ref().expect("url required");

    let discovery_pb = if !args.quiet {
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

    let discovered_urls = match discover_urls_for_tui(target_url, crawler_config).await {
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
    args: &Args,
    vault_path: &Option<std::path::PathBuf>,
) -> SelectedUrls {
    let ok = preflight::icon("✅", "OK");

    if args.quick_save && vault_path.is_some() {
        info!("Quick-save mode: bypassing TUI, will save to vault _inbox");
        SelectedUrls::Urls(discovered_urls.to_vec())
    } else if args.interactive {
        info!("Starting interactive TUI selector...");
        match adapters::tui::run_selector(discovered_urls).await {
            Ok(selected) => {
                info!("{} User selected {} URLs", ok, selected.len());
                if selected.is_empty() {
                    info!("No URLs selected, exiting");
                    SelectedUrls::None
                } else {
                    SelectedUrls::Urls(selected)
                }
            },
            Err(adapters::tui::TuiError::Interrupted) => {
                info!("User interrupted TUI selector, exiting");
                SelectedUrls::None
            },
            Err(e) => {
                warn!("TUI error: {}", e);
                SelectedUrls::Error(CliExit::ProtocolError(e.to_string()))
            },
        }
    } else {
        info!(
            "Headless mode: will scrape all {} URLs",
            discovered_urls.len()
        );
        SelectedUrls::Urls(discovered_urls.to_vec())
    }
}

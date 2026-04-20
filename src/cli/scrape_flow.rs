//! Scraping flow logic extracted from orchestrator.

use std::path::PathBuf;
use url::Url;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{Args, ScraperConfig};
use crate::{HttpClient, HttpClientConfig};
use crate::application::scrape_single_url_for_tui;
use crate::domain::ScrapedContent;
use crate::adapters::tui::{ScrapeProgress, ScrapeStatus, ScrapeError};
use crate::export_factory;
use crate::infrastructure::export::state_store::StateStore;

/// Apply resume mode filtering.
pub async fn apply_resume_mode(
    urls_to_scrape: Vec<Url>,
    args: &Args,
    target_url: &str,
) -> (
    Vec<Url>,
    Option<StateStore>,
) {
    let state_store = if args.resume {
        info!("Resume mode enabled - tracking processed URLs");
        let state_dir = args.state_dir.clone().unwrap_or_else(|| {
            let cache_base = std::env::var("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".cache")
                });
            cache_base.join("rust-scraper").join("state")
        });

        let domain = export_factory::domain_from_url(target_url);
        info!("State store domain: {}", domain);
        match export_factory::create_state_store(state_dir, &domain) {
            Ok(store) => Some(store),
            Err(e) => {
                warn!("Failed to create state store: {}", e);
                None
            },
        }
    } else {
        None
    };

    let filtered = if args.resume {
        if let Some(store) = state_store.as_ref() {
            match store.load_or_default() {
                Ok(state) => {
                    let original_count = urls_to_scrape.len();
                    let filtered: Vec<_> = urls_to_scrape
                        .into_iter()
                        .filter(|url| {
                            let should_skip = store.is_processed(&state, url.as_str());
                            if should_skip {
                                info!("Skipping already processed: {}", url);
                            }
                            !should_skip
                        })
                        .collect();

                    let skipped_count = original_count - filtered.len();
                    info!(
                        "Resume mode: {} URLs already processed, {} new URLs to scrape",
                        skipped_count,
                        filtered.len()
                    );

                    filtered
                },
                Err(e) => {
                    warn!("Failed to load state: {}", e);
                    urls_to_scrape
                },
            }
        } else {
            urls_to_scrape
        }
    } else {
        urls_to_scrape
    };

    (filtered, state_store)
}

/// Scrape all URLs with progress events via mpsc channel.
///
/// When progress_tx is provided (non-TUI mode), emits ScrapeProgress events.
/// When progress_tx is None (TUI mode), no progress events are emitted.
/// The --quiet flag suppresses all output including progress events.
pub async fn scrape_urls(
    urls: &[Url],
    scraper_config: &ScraperConfig,
    args: &Args,
    progress_tx: Option<mpsc::Sender<ScrapeProgress>>,
) -> (
    Vec<ScrapedContent>,
    Vec<(String, String)>,
) {
    let http_config = HttpClientConfig {
        max_retries: args.max_retries,
        backoff_base_ms: args.backoff_base_ms,
        backoff_max_ms: args.backoff_max_ms,
        accept_language: args.accept_language.clone(),
        ..HttpClientConfig::default()
    };
    let http_client = match HttpClient::new(http_config) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to create HTTP client: {}", e);
            return (Vec::new(), vec![("http_client".into(), e.to_string())]);
        },
    };

    let total_urls = urls.len();

    let mut results = Vec::with_capacity(total_urls);
    let mut failures: Vec<(String, String)> = Vec::new();

    for url in urls {
        let url_str = url.as_str();
        let _url_host = url.host_str().unwrap_or("unknown").to_string();

        // Emit progress event: Started
        if !args.quiet {
            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(ScrapeProgress::Started {
                        url: url_str.to_string(),
                    })
                    .await;
            }
        }

        // Emit progress event: StatusChanged to Fetching
        if !args.quiet {
            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(ScrapeProgress::StatusChanged {
                        url: url_str.to_string(),
                        status: ScrapeStatus::Fetching,
                    })
                    .await;
            }
        }

        match scrape_single_url_for_tui(http_client.client(), url, scraper_config).await {
            Ok(content) => {
                let chars = content.content.chars().count();
                results.push(content);
                // Emit progress event: Completed (only if not quiet)
                if !args.quiet {
                    if let Some(ref tx) = progress_tx {
                        let _ = tx
                            .send(ScrapeProgress::Completed {
                                url: url_str.to_string(),
                                chars,
                            })
                            .await;
                    }
                }
            },
            Err(e) => {
                let url_str = url.as_str().to_string();
                let err_msg = e.to_string();
                warn!("Failed to scrape {}: {}", url_str, err_msg);
                failures.push((url_str.clone(), err_msg));
                // Emit progress event: Failed
                if !args.quiet {
                    if let Some(ref tx) = progress_tx {
                        let _ = tx
                            .send(ScrapeProgress::Failed {
                                url: url_str.clone(),
                                error: ScrapeError::Other(format!("{}", e)),
                            })
                            .await;
                    }
                }
            },
        }
    }

    // Count totals from results/failures
    let total_successful = results.len();
    let total_failed = failures.len();

    // Emit Finished event when all done (only if not quiet)
    if !args.quiet {
        if let Some(ref tx) = progress_tx {
            let _ = tx
                .send(ScrapeProgress::Finished {
                    total: total_urls,
                    successful: total_successful,
                    failed: total_failed,
                })
                .await;
        }
    }

    (results, failures)
}

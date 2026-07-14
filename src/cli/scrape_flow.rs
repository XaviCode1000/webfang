//! Scraping flow logic extracted from orchestrator.

use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{info, warn};
use url::Url;

use crate::application::crawl_options::CrawlOptions;
use crate::application::export_factory;
use crate::application::progress_types::{ScrapeError, ScrapeProgress, ScrapeStatus};
use crate::application::scrape_single_url_for_tui;
use crate::domain::ScrapedContent;
use crate::infrastructure::crawler::robots_utils::{is_allowed_by_robots, new_robots_cache};
use crate::infrastructure::export::state_store::StateStore;
use crate::ScraperConfig;
use crate::{HttpClient, HttpClientConfig};

/// Apply resume mode filtering.
pub async fn apply_resume_mode(
    urls_to_scrape: Vec<Url>,
    opts: &CrawlOptions,
    target_url: &str,
) -> (Vec<Url>, Option<StateStore>) {
    let state_store: Option<StateStore> = if opts.crawl.resume {
        info!("Resume mode enabled - tracking processed URLs");
        let state_dir = opts.crawl.state_dir.clone().unwrap_or_else(|| {
            let cache_base = std::env::var("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".cache")
                });
            cache_base.join("rust_scraper").join("state")
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

    let filtered = if opts.crawl.resume {
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
    opts: &CrawlOptions,
    progress_tx: Option<mpsc::Sender<ScrapeProgress>>,
    downloader: Option<&crate::adapters::downloader::Downloader>,
) -> (
    Vec<ScrapedContent>,
    Vec<(String, crate::error::ScraperError)>,
) {
    // Early return if --force-js-render is requested (Phase 2 feature)
    if opts.network.force_js_render {
        warn!("--force-js-render no está implementado (Fase 2)");
        return (
            Vec::new(),
            vec![(
                "force_js_render".into(),
                crate::error::ScraperError::FeatureGated(
                    "JavaScript rendering no está implementado. Fase 2 planificada.".into(),
                ),
            )],
        );
    }

    let http_config = build_http_client_config(opts);
    let http_client = match HttpClient::new(http_config) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to create HTTP client: {}", e);
            return (
                Vec::new(),
                vec![(
                    "http_client".into(),
                    crate::error::ScraperError::Config(e.to_string()),
                )],
            );
        },
    };

    let _total_urls = urls.len();

    // Robots.txt cache — shared across all URLs in this batch
    let robots_cache = new_robots_cache();

    // Apply max_pages limit if configured
    let urls_to_process = if let Some(max_pages) = scraper_config.max_pages {
        let limited: Vec<_> = urls.iter().take(max_pages).cloned().collect();
        if limited.len() < urls.len() {
            tracing::info!(
                "Limiting to {} pages (max_pages={}), skipping {} URLs",
                limited.len(),
                max_pages,
                urls.len() - limited.len()
            );
        }
        limited
    } else {
        urls.to_vec()
    };

    let processing_count = urls_to_process.len();
    let mut results = Vec::with_capacity(processing_count);
    let mut failures: Vec<(String, crate::error::ScraperError)> = Vec::new();

    for url in urls_to_process {
        let url_str = url.as_str();
        let _url_host = url.host_str().unwrap_or("unknown").to_string();

        // Emit progress event: Started
        if !opts.export.quiet {
            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(ScrapeProgress::Started {
                        url: url_str.to_string(),
                    })
                    .await;
            }
        }

        // Emit progress event: StatusChanged to Fetching
        if !opts.export.quiet {
            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(ScrapeProgress::StatusChanged {
                        url: url_str.to_string(),
                        status: ScrapeStatus::Fetching,
                    })
                    .await;
            }
        }

        // Robots.txt enforcement — skip disallowed URLs unless --ignore-robots
        if !opts.crawl.ignore_robots {
            let domain = url.host_str().unwrap_or("unknown");
            if !is_allowed_by_robots(url_str, domain, &robots_cache).await {
                info!("Blocked by robots.txt: {}", url_str);
                if !opts.export.quiet {
                    if let Some(ref tx) = progress_tx {
                        let _ = tx
                            .send(ScrapeProgress::Failed {
                                url: url_str.to_string(),
                                error: ScrapeError::Other("blocked by robots.txt".into()),
                            })
                            .await;
                    }
                }
                failures.push((
                    url_str.to_string(),
                    crate::error::ScraperError::Validation("blocked by robots.txt".into()),
                ));
                continue;
            }
        }

        match scrape_single_url_for_tui(http_client.client(), &url, scraper_config, downloader)
            .await
        {
            Ok(content) => {
                let chars = content.content.chars().count();
                results.push(content);
                // Emit progress event: Completed (only if not quiet)
                if !opts.export.quiet {
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
                warn!("Failed to scrape {}: {}", url_str, e);
                // Emit progress event: Failed (borrows `e` before it is moved)
                if !opts.export.quiet {
                    if let Some(ref tx) = progress_tx {
                        let _ = tx
                            .send(ScrapeProgress::Failed {
                                url: url_str.clone(),
                                error: ScrapeError::Other(format!("{e}")),
                            })
                            .await;
                    }
                }
                failures.push((url_str.clone(), e));
            },
        }
    }

    // Count totals from results/failures
    let total_successful = results.len();
    let total_failed = failures.len();

    // Emit Finished event when all done (only if not quiet)
    if !opts.export.quiet {
        if let Some(ref tx) = progress_tx {
            let _ = tx
                .send(ScrapeProgress::Finished {
                    total: processing_count,
                    successful: total_successful,
                    failed: total_failed,
                })
                .await;
        }
    }

    (results, failures)
}

fn build_http_client_config(opts: &CrawlOptions) -> HttpClientConfig {
    HttpClientConfig {
        max_retries: opts.network.max_retries,
        backoff_base_ms: opts.network.backoff_base_ms,
        backoff_max_ms: opts.network.backoff_max_ms,
        accept_language: opts.network.accept_language.clone(),
        user_agent: opts.network.user_agent.clone(),
        timeout_secs: opts.network.timeout_secs,
        h2_profile: opts.network.h2_profile.clone(),
        ..HttpClientConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{build_http_client_config, is_allowed_by_robots, new_robots_cache};
    use crate::application::crawl_options::CrawlOptions;

    #[test]
    fn build_http_client_config_uses_opts_timeout_secs() {
        let mut opts = CrawlOptions::default();
        opts.network.timeout_secs = 7;

        let config = build_http_client_config(&opts);

        assert_eq!(config.timeout_secs, 7);
        assert_eq!(config.max_retries, opts.network.max_retries);
        assert_eq!(config.backoff_base_ms, opts.network.backoff_base_ms);
        assert_eq!(config.backoff_max_ms, opts.network.backoff_max_ms);
        assert_eq!(config.accept_language, opts.network.accept_language);
    }

    #[test]
    fn build_http_client_config_preserves_default_timeout_when_unset() {
        let opts = CrawlOptions::default();

        let config = build_http_client_config(&opts);

        assert_eq!(config.timeout_secs, 30);
    }

    #[tokio::test]
    async fn robots_cache_allows_public_urls() {
        let cache = new_robots_cache();
        // No robots.txt for localhost → fail-open → allowed
        assert!(is_allowed_by_robots("http://localhost:18080/page", "localhost", &cache).await);
    }

    #[test]
    fn ignore_robots_flag_defaults_to_false() {
        let opts = CrawlOptions::default();
        assert!(!opts.crawl.ignore_robots);
    }
}

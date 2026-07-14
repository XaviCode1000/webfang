//! Export pipeline utilities
//!
//! Provides helper functions for creating exporters and state stores
//! based on CLI configuration.

use crate::infrastructure::export::StateStore;
use std::path::PathBuf;
use tracing::info;

/// Get domain from URL for StateStore
///
/// # Arguments
///
/// * `url` - URL to extract domain from
///
/// # Returns
///
/// Domain string
///
/// # Examples
///
/// ```
/// use rust_scraper::export_utils::domain_from_url;
///
/// assert_eq!(domain_from_url("https://example.com"), "example.com");
/// assert_eq!(domain_from_url("https://sub.example.com/page"), "example.com");
/// ```
pub fn domain_from_url(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            return host.to_string();
        }
    }
    "unknown".to_string()
}

/// Initialize StateStore with custom state directory
///
/// # Arguments
///
/// * `state_dir` - Custom state directory path
/// * `url` - URL to extract domain from
///
/// # Returns
///
/// Configured StateStore instance
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use rust_scraper::export_utils::create_state_store;
///
/// let store = create_state_store(
///     PathBuf::from("/custom/state/dir"),
///     "https://example.com"
/// ).unwrap();
/// ```
pub fn create_state_store(state_dir: PathBuf, url: &str) -> anyhow::Result<StateStore> {
    let domain = domain_from_url(url);
    let mut store = StateStore::new(&domain);

    if !state_dir.as_os_str().is_empty() {
        store.set_cache_dir(state_dir);
    }

    info!("Initialized StateStore for domain: {}", domain);
    Ok(store)
}

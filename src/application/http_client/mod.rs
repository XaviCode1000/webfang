//! HTTP client wrapper with retry middleware and status-specific handling
//!
//! Provides a configurable HTTP client with:
//! - Custom headers (Accept-Language, Accept, Referer, Cache-Control)
//! - Exponential backoff retry policy
//! - Specific handling for 403 (rotate UA), 429 (backoff), 5xx (retry)
//! - Cookie support
//!
//! # Examples
//!
//! ```no_run
//! use rust_scraper::application::http_client::{HttpClient, HttpClientConfig};
//!
//! let config = HttpClientConfig::default();
//! let client = HttpClient::new(config).unwrap();
//! // Use client for HTTP requests
//! ```

mod client;
mod config;
mod error;
mod waf;

pub use client::{create_http_client, get_random_user_agent_from_pool, HttpClient};
pub use config::HttpClientConfig;
pub use error::{HttpError, HttpResult};
pub use waf::detect_waf_challenge;

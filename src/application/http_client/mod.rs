//! HTTP client wrapper with retry middleware and status-specific handling
//!
//! Provides a configurable HTTP client with:
//! - Custom headers (Accept-Language, Accept, Referer, Cache-Control)
//! - Exponential backoff retry policy
//! - Specific handling for 403 (rotate UA), 429 (backoff), 5xx (retry)
//! - Cookie support
//! - Configurable request and connection timeouts
//! - Rate limiting (requests per minute)
//! - URL validation
//! - TLS fingerprint rotation (Chrome 131, 145, Firefox, etc.)
//!
//! # Examples
//!
//! ```no_run
//! use rust_scraper::application::http_client::{HttpClient, HttpClientConfig};
//! use wreq_util::Profile;
//!
//! let config = HttpClientConfig {
//!     timeout_secs: 60,
//!     rate_limit_rpm: Some(30),
//!     tls_emulation: Profile::Chrome131,
//!     ..Default::default()
//! };
//! let client = HttpClient::new(config).unwrap();
//! // Use client for HTTP requests
//! ```

mod client;
mod error;
mod http_config;
pub mod port;

pub use client::{create_http_client, get_random_user_agent_from_pool, HttpClient};
pub use error::{HttpError, HttpResult};
pub use http_config::HttpClientConfig;
pub use port::{HttpClientPort, HttpResponse};

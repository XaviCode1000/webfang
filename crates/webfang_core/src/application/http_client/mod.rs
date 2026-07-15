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
//! # Architecture
//!
//! The port contract (`HttpClientPort`, `HttpResponse`, `HttpError`,
//! `HttpResult`, `HttpClientConfig`) is **owned by the domain layer**
//! (`crate::domain::http_port` / `http_error` / `http_config`). This
//! module re-exports them for backward compatibility so existing importers
//! (`crate::application::http_client::*`) keep working. The concrete
//! `HttpClient` and the `impl HttpClientPort for wreq::Client` live here,
//! depending inward on the domain port (Clean Architecture direction).
//!
//! # Examples
//!
//! ```no_run
//! use webfang::application::http_client::{HttpClient, HttpClientConfig};
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
pub mod port;

pub use crate::domain::http_config::HttpClientConfig;
pub use crate::domain::http_error::{HttpError, HttpResult};
pub use crate::domain::http_port::{HttpClientPort, HttpResponse};
pub use client::{create_http_client, get_random_user_agent_from_pool, HttpClient};

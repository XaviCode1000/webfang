//! HTTP port trait and response DTO — owned by the **domain** layer.
//!
//! The domain defines the contract for HTTP fetching. The production client
//! (wreq-backed) and test doubles implement this trait in the
//! application/infrastructure layers. Keeping the port in the domain layer
//! enforces Clean Architecture: application code depends on `HttpClientPort`,
//! never on a concrete HTTP client.

use std::collections::HashMap;
use std::pin::Pin;

use crate::domain::http_error::HttpResult;

/// Simplified HTTP response for application-layer consumption.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code (e.g. 200, 404).
    pub status: u16,
    /// Response body as a UTF-8 string.
    pub body: String,
    /// Response headers (lowercased keys).
    pub headers: HashMap<String, String>,
}

/// Port trait for HTTP requests — application layer depends on this, not `wreq`.
///
/// Implementors provide the actual network I/O (production) or canned
/// responses (tests). This trait is intentionally thin — only `get` is
/// required — so that mock implementations stay simple and fast to compile.
///
/// # Thread safety
///
/// Implementations must be `Send + Sync` to work with Tokio's
/// multi-threaded runtime.
pub trait HttpClientPort: Send + Sync {
    /// Fetch a URL and return the response body.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError`] on network failure, timeout, or non-2xx status.
    fn get(
        &self,
        url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = HttpResult<HttpResponse>> + Send + '_>>;
}

//! Shared test fixtures for `webfang_core` unit tests.
//!
//! Compiled only under `cfg(test)` (declared with `#[cfg(test)]` in
//! `lib.rs`). Consolidates the mock HTTP client previously duplicated in
//! `application::http_client::port` and `domain::ports` into a single
//! source of truth (error-map-v2-cleanup, Item 2, Tier-1).

use std::collections::HashMap;
use std::pin::Pin;

use crate::domain::http_error::{HttpError, HttpResult};
use crate::domain::http_port::{HttpClientPort, HttpResponse};

/// Mock HTTP client returning canned responses keyed by URL.
///
/// URLs without a registered response produce `HttpError::ClientError(404)`.
pub(crate) struct MockHttpClient {
    responses: HashMap<String, HttpResult<HttpResponse>>,
}

impl MockHttpClient {
    /// Create an empty mock (every URL resolves to a 404 client error).
    pub(crate) fn new() -> Self {
        Self {
            responses: HashMap::new(),
        }
    }

    /// Register a canned result for a URL (builder-style).
    #[must_use]
    pub(crate) fn with_response(mut self, url: &str, result: HttpResult<HttpResponse>) -> Self {
        self.responses.insert(url.to_string(), result);
        self
    }
}

impl HttpClientPort for MockHttpClient {
    fn get(
        &self,
        url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = HttpResult<HttpResponse>> + Send + '_>> {
        let result = match self.responses.get(url) {
            Some(Ok(resp)) => Ok(HttpResponse {
                status: resp.status,
                body: resp.body.clone(),
                headers: resp.headers.clone(),
            }),
            Some(Err(HttpError::Forbidden)) => Err(HttpError::Forbidden),
            Some(Err(HttpError::RateLimited(r))) => Err(HttpError::RateLimited(*r)),
            Some(Err(HttpError::ClientError(c))) => Err(HttpError::ClientError(*c)),
            Some(Err(HttpError::ServerError(c))) => Err(HttpError::ServerError(*c)),
            Some(Err(HttpError::Timeout)) => Err(HttpError::Timeout),
            Some(Err(HttpError::Connection(m))) => Err(HttpError::Connection(m.clone())),
            Some(Err(HttpError::Request(m))) => Err(HttpError::Request(m.clone())),
            Some(Err(HttpError::WafChallenge(p))) => Err(HttpError::WafChallenge(p.clone())),
            Some(Err(HttpError::DomainBanned(d))) => Err(HttpError::DomainBanned(d.clone())),
            None => Err(HttpError::ClientError(404)),
        };
        Box::pin(async move { result })
    }
}

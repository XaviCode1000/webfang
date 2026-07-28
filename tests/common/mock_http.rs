//! Shared mock HTTP client for integration tests (error-map-v2-cleanup, Item 2, Tier-2).
//!
//! Integration-test counterpart of the unit-side `webfang_core::test_fixtures`
//! mock. Kept as a separate copy because `#[cfg(test)] pub(crate)` fixtures
//! are not visible outside the crate. Wired into each consuming test via the
//! flat `#[path = "common/mock_http.rs"] mod mock_http;` convention (see
//! `cli_binary_test.rs`) — deliberately NOT routed through `tests/common/mod.rs`
//! (orphaned, references the unresolvable `webfang::` bin-only crate).
#![allow(dead_code)]

use std::collections::HashMap;

use webfang_core::application::http_client::{HttpClientPort, HttpError, HttpResponse, HttpResult};

/// Mock HTTP client returning canned responses keyed by URL.
///
/// URLs without a registered response produce `HttpError::ClientError(404)`.
pub struct MockHttpClient {
    responses: HashMap<String, HttpResult<HttpResponse>>,
}

impl MockHttpClient {
    /// Create an empty mock (every URL resolves to a 404 client error).
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
        }
    }

    /// Register a canned result for a URL (builder-style).
    pub fn with_response(mut self, url: &str, result: HttpResult<HttpResponse>) -> Self {
        self.responses.insert(url.to_string(), result);
        self
    }

    /// Shorthand for a 200 OK response with the given HTML body.
    pub fn with_ok_response(self, url: &str, body: &str) -> Self {
        self.with_response(
            url,
            Ok(HttpResponse {
                status: 200,
                body: body.to_string(),
                headers: HashMap::new(),
            }),
        )
    }
}

impl HttpClientPort for MockHttpClient {
    fn get(
        &self,
        url: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = HttpResult<HttpResponse>> + Send + '_>>
    {
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

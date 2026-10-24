//! Shared arrange for the sitemap parse tests (#1757, S1).
//!
//! The arrange "serve a body on a wiremock mock, then parse it" is ONE
//! thing, and it used to live inside a single test binary
//! (`sitemap_integration.rs`) where no other binary could import it — so
//! `sitemap_behavioral.rs` and `orchestrator_unit_tests.rs` rewrote the same
//! block inline, once per test. This module is the one copy.
//!
//! Reach it from a test binary of this crate with the crate's convention:
//!
//! ```ignore
//! #[path = "common/mod.rs"] mod common;
//! use common::sitemap_arrange::{serve_and_parse, serve_many_and_parse};
//! ```
//!
//! The helper does NOT touch the environment: the entry-guard posture stays
//! at the call site (`webfang_test_utils::EnvGuard::entry_guard_off()`), which
//! is what keeps the one test that deliberately keeps the guard armed honest.

use webfang_core::domain::CorrelationId;
use webfang_core::infrastructure::crawler::{SitemapError, SitemapParser, SitemapUrl};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper: create parser with default config (no gzip, low depth for tests)
pub fn parser() -> SitemapParser {
    SitemapParser::new().unwrap()
}

/// Shared fetch-and-parse arrange (#1889, T7 slice-14): serve a single body
/// at `mount_path` on the caller's mock server, then run `parse_from_url`
/// against `entry_path` on that server. Mirrors the unit-level helper shape
/// (serve → parse → result), parameterized for the integration level instead
/// of forked: the server lifecycle and entry-guard posture stay at the call
/// site (index bodies embed `mock.uri()`), each test keeps its template
/// construction and result assertions.
pub async fn serve_and_parse(
    mock: &MockServer,
    mount_path: &str,
    response: ResponseTemplate,
    entry_path: &str,
) -> Result<Vec<SitemapUrl>, SitemapError> {
    serve_many_and_parse(mock, vec![(mount_path, response)], entry_path).await
}

/// Multi-mount variant for sitemap-index cases (index + children).
pub async fn serve_many_and_parse(
    mock: &MockServer,
    mounts: Vec<(&str, ResponseTemplate)>,
    entry_path: &str,
) -> Result<Vec<SitemapUrl>, SitemapError> {
    for (mount_path, response) in mounts {
        Mock::given(method("GET"))
            .and(path(mount_path))
            .respond_with(response)
            .mount(mock)
            .await;
    }
    parser()
        .parse_from_url(
            &format!("{}{}", mock.uri(), entry_path),
            &CorrelationId::new(),
        )
        .await
}

/// Shared arrange for the sitemap-INDEX cases: serve the index body at
/// `index_path` plus the child sitemaps at their own paths, then parse the
/// index entry.
///
/// The index body is the caller's (it embeds `mock.uri()`, and the
/// index-under-test differs per case: two children, a malformed child, a
/// self-referencing loop); the CHILD MOUNTS are the caller's too. What this
/// helper owns is the part that was copy-pasted: the index mount (200 +
/// `application/xml` + the index body) and the entry path that both the
/// mount and the parse must agree on.
pub async fn serve_index_and_parse(
    mock: &MockServer,
    index_path: &str,
    index_xml: &str,
    child_mounts: Vec<(&str, ResponseTemplate)>,
) -> Result<Vec<SitemapUrl>, SitemapError> {
    let mut mounts = vec![(
        index_path,
        ResponseTemplate::new(200)
            .set_body_string(index_xml)
            .insert_header("content-type", "application/xml"),
    )];
    mounts.extend(child_mounts);
    serve_many_and_parse(mock, mounts, index_path).await
}

/// Helper to create a sitemap index XML with mock server base URL
pub fn sitemap_index_with_base(base_url: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap>
        <loc>{base_url}/sitemap1.xml</loc>
        <lastmod>2024-01-15</lastmod>
    </sitemap>
    <sitemap>
        <loc>{base_url}/sitemap2.xml</loc>
        <lastmod>2024-01-10</lastmod>
    </sitemap>
</sitemapindex>"#
    )
}

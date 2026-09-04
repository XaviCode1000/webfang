//! Unit tests for MCP parameter validation (issue #512, Slice 1)
//!
//! These tests cover two contracts:
//!
//! 1. The `validation::*` helper functions — they must reject malformed
//!    inputs (file:// scheme, path traversal, oversize strings) and accept
//!    well-formed ones, with `McpError::INVALID_PARAMS` as the error code.
//! 2. The `deny_unknown_fields` serde attribute on every `*Params` struct —
//!    an extra JSON key must be rejected at deserialization time.
//!
//! Per the contract-based-test-audit 6-node diagnostic:
//! - Observable behavior only: assertions match on `ErrorCode::INVALID_PARAMS`,
//!   never on error message strings (those are not part of the contract).
//! - No infrastructure: pure unit tests, no I/O, no time, no randomness.
//! - Arrange ≤ 5 lines per test.

use webfang_mcp::mcp_server::params::*;
use webfang_mcp::mcp_server::validation;

/// Test helper: build an `McpUrl` from a KNOWN-VALID http(s) string.
fn vu(s: &str) -> McpUrl {
    s.parse().expect("test url must be valid http(s)")
}

// ===========================================================================
// validation::require_http_url
// ===========================================================================

#[test]
fn require_http_url_accepts_https() {
    let parsed =
        validation::require_http_url("url", "https://example.com/path?q=1").expect("https ok");
    assert_eq!(parsed.scheme(), "https");
}

#[test]
fn require_http_url_accepts_http() {
    let parsed = validation::require_http_url("url", "http://example.com").expect("http ok");
    assert_eq!(parsed.scheme(), "http");
}

#[test]
fn require_http_url_rejects_file_scheme() {
    let err = validation::require_http_url("url", "file:///etc/passwd").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_http_url_rejects_ftp_scheme() {
    let err = validation::require_http_url("url", "ftp://example.com").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_http_url_rejects_javascript_scheme() {
    let err = validation::require_http_url("url", "javascript:alert(1)").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_http_url_rejects_data_scheme() {
    let err = validation::require_http_url("url", "data:text/plain,hello").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_http_url_rejects_empty() {
    let err = validation::require_http_url("url", "").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_http_url_rejects_oversize() {
    let oversize = "https://example.com/".to_string() + &"a".repeat(validation::MAX_URL_LEN);
    let err = validation::require_http_url("url", &oversize).unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_http_url_rejects_unparseable() {
    let err = validation::require_http_url("url", "not a url at all").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

// ===========================================================================
// validation::require_safe_path
// ===========================================================================

#[test]
fn require_safe_path_accepts_relative() {
    let path = validation::require_safe_path("output_dir", "exports/2026").expect("relative ok");
    assert_eq!(path.to_str(), Some("exports/2026"));
}

#[test]
fn require_safe_path_rejects_parent_traversal() {
    let err = validation::require_safe_path("output_dir", "../etc/passwd").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_path_rejects_parent_traversal_in_middle() {
    let err = validation::require_safe_path("output_dir", "exports/../secrets").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_path_rejects_absolute_unix() {
    let err = validation::require_safe_path("output_dir", "/etc/passwd").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_path_rejects_absolute_windows() {
    let err = validation::require_safe_path("output_dir", "C:\\Windows\\System32").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_path_rejects_empty() {
    let err = validation::require_safe_path("output_dir", "").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_path_rejects_oversize() {
    let oversize = "a".repeat(validation::MAX_PATH_LEN + 1);
    let err = validation::require_safe_path("output_dir", &oversize).unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

// NOTE: absolute-path acceptance for the export/obsidian `output_dir` /
// `vault_path` params (issue #600) is covered by the internal `params::tests`
// module in `src/mcp_server/params.rs`, and `export_file` (the only `pub`
// struct of this shape) is covered end-to-end by
// `params_rejection_test.rs::export_file_rejects_absolute_output_dir_without_roots`
// (the #756 fail-closed root-of-trust gate complements #600). The
// `require_safe_path_allow_absolute_*` tests above cover the shared helper.

// ===========================================================================
// validation::require_max_len
// ===========================================================================

#[test]
fn require_max_len_accepts_within() {
    validation::require_max_len("html", "<html/>", 1024).expect("within limit");
}

#[test]
fn require_max_len_rejects_over() {
    let oversize = "x".repeat(11);
    let err = validation::require_max_len("html", &oversize, 10).unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

// ===========================================================================
// validation::require_safe_domain
// ===========================================================================

#[test]
fn require_safe_domain_accepts_bare_domain() {
    validation::require_safe_domain("seed_domain", "example.com").expect("bare domain ok");
    validation::require_safe_domain("seed_domain", "a.b.c.example.co.uk")
        .expect("deep subdomain ok");
}

#[test]
fn require_safe_domain_rejects_url() {
    let err = validation::require_safe_domain("seed_domain", "https://example.com").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_domain_rejects_traversal() {
    let err = validation::require_safe_domain("seed_domain", "example..com").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_domain_rejects_no_dot() {
    let err = validation::require_safe_domain("seed_domain", "localhost").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

// ===========================================================================
// validation::require_safe_seed
// ===========================================================================

#[test]
fn require_safe_seed_accepts_bare_domain() {
    validation::require_safe_seed("seed_domain", "example.com").expect("bare domain ok");
    validation::require_safe_seed("seed_domain", "a.b.c.example.co.uk").expect("deep subdomain ok");
}

#[test]
fn require_safe_seed_accepts_full_url() {
    validation::require_safe_seed("seed_domain", "https://example.com/path")
        .expect("full http(s) URL ok");
}

#[test]
fn require_safe_seed_rejects_file_scheme() {
    let err = validation::require_safe_seed("seed_domain", "file:///etc").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_seed_rejects_traversal_url() {
    let err = validation::require_safe_seed("seed_domain", "https://evil.com/..").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_seed_rejects_empty() {
    let err = validation::require_safe_seed("seed_domain", "").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_seed_rejects_no_dot() {
    let err = validation::require_safe_seed("seed_domain", "localhost").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn require_safe_seed_rejects_traversal_bare() {
    let err = validation::require_safe_seed("seed_domain", "..").unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

// ===========================================================================
// validation::require_one_of
// ===========================================================================

#[test]
fn require_one_of_accepts_match() {
    validation::require_one_of("format", "jsonl", &["jsonl", "vector", "auto"])
        .expect("exact match");
    validation::require_one_of("format", "JSONL", &["jsonl", "vector", "auto"])
        .expect("case-insensitive match");
}

#[test]
fn require_one_of_rejects_unknown() {
    let err =
        validation::require_one_of("format", "xml", &["jsonl", "vector", "auto"]).unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

// ===========================================================================
// Per-struct validate() — ScrapeUrlParams (the headline case)
// ===========================================================================

#[test]
fn scrape_url_params_accepts_https() {
    let p = ScrapeUrlParams {
        url: vu("https://example.com"),
    };
    p.validate().expect("https url should be valid");
}

/// #1116: `file://` and empty URLs are now UNREPRESENTABLE — `McpUrl`
/// rejects them during deserialization, so a `ScrapeUrlParams` carrying
/// them cannot be built. This is strictly stronger than the old
/// "construct then validate()" tests: the invalid state never exists.
#[test]
fn scrape_url_params_reject_file_scheme_and_empty_at_boundary() {
    for bad in ["file:///etc/passwd", ""] {
        let res = serde_json::from_value::<ScrapeUrlParams>(serde_json::json!({ "url": bad }));
        assert!(res.is_err(), "`{bad}` must fail to deserialize into McpUrl");
    }
}

// ===========================================================================
// Per-struct validate() — ScrapeBatchParams (array element validation)
// ===========================================================================

#[test]
fn scrape_batch_params_rejects_empty_list() {
    let p = ScrapeBatchParams {
        urls: vec![],
        concurrency: None,
        ignore_robots: None,
    };
    let err = p.validate().unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

/// #1116: a single bad element makes the whole batch fail to deserialize —
/// the invalid URL is unrepresentable, so `validate()` never even runs.
#[test]
fn scrape_batch_params_reject_one_bad_url_at_boundary() {
    let res = serde_json::from_value::<ScrapeBatchParams>(serde_json::json!({
        "urls": ["https://ok.example", "file:///etc/passwd"]
    }));
    assert!(
        res.is_err(),
        "a bad batch element must fail the whole deserialize"
    );
}

#[test]
fn scrape_batch_params_rejects_oversize_concurrency() {
    let p = ScrapeBatchParams {
        urls: vec![vu("https://example.com")],
        concurrency: Some(65),
        ignore_robots: None,
    };
    let err = p.validate().unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn scrape_batch_params_accepts_valid() {
    let p = ScrapeBatchParams {
        urls: vec![vu("https://example.com"), vu("https://other.example/path")],
        concurrency: Some(8),
        ignore_robots: None,
    };
    p.validate().expect("valid batch");
}

// ===========================================================================
// Per-struct validate() — ExportFileParams (path + format + content)
// ===========================================================================

#[test]
fn export_file_params_accepts_valid() {
    let p = ExportFileParams {
        output_dir: "exports/2026".into(),
        filename: "out".into(),
        content_format: "jsonl".into(),
        content: "row".into(),
    };
    p.validate().expect("valid export");
}

#[test]
fn export_file_params_rejects_output_dir_traversal() {
    let p = ExportFileParams {
        output_dir: "../etc".into(),
        filename: "out".into(),
        content_format: "jsonl".into(),
        content: "row".into(),
    };
    let err = p.validate().unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn export_file_params_rejects_unknown_format() {
    let p = ExportFileParams {
        output_dir: "exports".into(),
        filename: "out".into(),
        content_format: "xml".into(),
        content: "row".into(),
    };
    let err = p.validate().unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn export_file_params_rejects_empty_filename() {
    let p = ExportFileParams {
        output_dir: "exports".into(),
        filename: String::new(),
        content_format: "jsonl".into(),
        content: "row".into(),
    };
    let err = p.validate().unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

// ===========================================================================
// Per-struct validate() — CleanHtmlParams / CrawlSiteParams
// ===========================================================================

#[test]
fn clean_html_params_rejects_empty() {
    let p = CleanHtmlParams {
        html: String::new(),
    };
    let err = p.validate().unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn crawl_site_params_rejects_oversize_max_depth() {
    let p = CrawlSiteParams {
        url: vu("https://example.com"),
        max_depth: Some(11),
        max_pages: None,
    };
    let err = p.validate().unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

#[test]
fn crawl_site_params_rejects_oversize_max_pages() {
    let p = CrawlSiteParams {
        url: vu("https://example.com"),
        max_depth: None,
        max_pages: Some(100_001),
    };
    let err = p.validate().unwrap_err();
    assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
}

// ===========================================================================
// deny_unknown_fields — serde-level contract
// ===========================================================================

#[test]
fn deny_unknown_fields_rejects_extra_field() {
    let json = r#"{"url": "https://example.com", "extra": "boom"}"#;
    let result: Result<ScrapeUrlParams, _> = serde_json::from_str(json);
    assert!(result.is_err(), "deny_unknown_fields must reject extras");
}

#[test]
fn deny_unknown_fields_accepts_known_field() {
    let json = r#"{"url": "https://example.com"}"#;
    let result: Result<ScrapeUrlParams, _> = serde_json::from_str(json);
    assert!(result.is_ok(), "known-only payload must deserialize");
}

#[test]
fn deny_unknown_fields_rejects_extra_on_export_file() {
    let json = r#"{
        "output_dir": "exports",
        "filename": "out",
        "format": "jsonl",
        "content": "row",
        "unexpected": "boom"
    }"#;
    let result: Result<ExportFileParams, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn deny_unknown_fields_rejects_extra_on_clean_html() {
    let json = r#"{"html": "<p>hi</p>", "magic": true}"#;
    let result: Result<CleanHtmlParams, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

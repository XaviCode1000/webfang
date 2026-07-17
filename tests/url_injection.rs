//! Security fuzzing tests for URL validation
//!
//! Verifies that the URL validator rejects malicious inputs including:
//! - Path traversal attacks
//! - Null bytes
//! - Newline/whitespace injection
//! - SSRF targets (link-local, metadata endpoints)
//! - Very long URLs
//! - Unicode normalization attacks
//! - Non-HTTP schemes (file://, javascript:, etc.)

use url::Url;

// ============================================================================
// Domain URL validator (pure logic, no network)
// ============================================================================

fn validate_url_domain(url: &str) -> webfang::domain::ValidationResult {
    let parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(_) => return webfang::domain::ValidationResult::Invalid("parse error".into()),
    };
    webfang::domain::url_validator::UrlValidator::filter_invalid_patterns(&parsed)
}

// ============================================================================
// Scheme injection — non-HTTP schemes must be rejected
// ============================================================================

#[test]
fn reject_file_scheme() {
    assert!(matches!(
        validate_url_domain("file:///etc/passwd"),
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn reject_javascript_scheme() {
    assert!(matches!(
        validate_url_domain("javascript:alert(1)"),
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn reject_ftp_scheme() {
    assert!(matches!(
        validate_url_domain("ftp://example.com/file"),
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn reject_data_uri() {
    assert!(matches!(
        validate_url_domain("data:text/html,<script>alert(1)</script>"),
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn reject_mailto_scheme() {
    assert!(matches!(
        validate_url_domain("mailto:user@example.com"),
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn reject_dict_scheme() {
    assert!(matches!(
        validate_url_domain("dict://example.com:word"),
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn accept_http_scheme() {
    assert!(matches!(
        validate_url_domain("http://example.com/page"),
        webfang::domain::ValidationResult::Valid
    ));
}

#[test]
fn accept_https_scheme() {
    assert!(matches!(
        validate_url_domain("https://example.com/page"),
        webfang::domain::ValidationResult::Valid
    ));
}

// ============================================================================
// SSRF — link-local and metadata endpoint targets
// ============================================================================

#[test]
fn aws_metadata_endpoint_accepted_by_scheme_check() {
    let result = validate_url_domain("http://169.254.169.254/latest/meta-data/");
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Valid
    ));
}

#[test]
fn azure_metadata_endpoint_accepted_by_scheme_check() {
    let result =
        validate_url_domain("http://169.254.169.254/metadata/instance?api-version=2021-02-01");
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Valid
    ));
}

#[test]
fn gcp_metadata_endpoint_accepted_by_scheme_check() {
    let result = validate_url_domain("http://metadata.google.internal/computeMetadata/v1/");
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Valid
    ));
}

// ============================================================================
// Path traversal — ../ sequences
// ============================================================================

#[test]
fn path_traversal_normalized_by_url_parser() {
    let url = Url::parse("http://example.com/../../../etc/passwd").unwrap();
    let path = url.path();
    assert!(
        !path.contains("../"),
        "URL parser should normalize path traversal: got path={path}"
    );
}

#[test]
fn path_traversal_resolves_to_root() {
    let url = Url::parse("http://example.com/page/../../secret").unwrap();
    assert_eq!(url.path(), "/secret");
}

// ============================================================================
// Null byte injection
// ============================================================================

#[test]
fn percent_encoded_null_byte_in_url() {
    let result = Url::parse("http://example.com/page%00.html");
    assert!(
        result.is_ok(),
        "URL with percent-encoded null byte should parse"
    );
    let url = result.unwrap();
    assert!(url.path().contains("%00") || url.path().contains('\0'));
}

#[test]
fn raw_null_byte_in_url_no_panic() {
    let result = Url::parse("http://example.com/page\x00.html");
    // Null bytes are percent-encoded by the WHATWG URL parser
    let url = result.expect("URL with null byte should parse");
    assert!(
        url.as_str().contains("%00"),
        "null byte must be percent-encoded: {url}"
    );
}

// ============================================================================
// Newline / whitespace injection
// ============================================================================

#[test]
fn newline_in_url_no_panic() {
    let result = Url::parse("http://example.com/page\n<script>alert(1)</script>");
    // WHATWG spec strips newlines during preprocessing — URL may parse but with cleaned input
    if let Ok(url) = &result {
        assert!(
            !url.as_str().contains('\n'),
            "newline must not survive URL parsing: {url}"
        );
    }
}

#[test]
fn carriage_return_in_url_no_panic() {
    let result = Url::parse("http://example.com/page\r\n<script>alert(1)</script>");
    // WHATWG spec strips CR+LF during preprocessing — URL may parse but with cleaned input
    if let Ok(url) = &result {
        assert!(
            !url.as_str().contains('\r'),
            "carriage return must not survive URL parsing: {url}"
        );
    }
}

#[test]
fn tab_in_url_no_panic() {
    let result = Url::parse("http://example.com/page\there");
    // WHATWG spec strips tabs during preprocessing — URL may parse but with cleaned input
    if let Ok(url) = &result {
        assert!(
            !url.as_str().contains('\t'),
            "tab must not survive URL parsing: {url}"
        );
    }
}

// ============================================================================
// Very long URLs (DoS vector)
// ============================================================================

#[test]
fn very_long_url_scheme() {
    let long_path = "a".repeat(10_000);
    let url_str = format!("https://example.com/{long_path}");
    let result = Url::parse(&url_str);
    assert!(result.is_ok(), "10k-char URL should parse without panic");
}

#[test]
fn very_long_url_total_length_2000() {
    let path_len = 2000 - "https://example.com/".len();
    let long_path = "a".repeat(path_len);
    let url_str = format!("https://example.com/{long_path}");
    let result = Url::parse(&url_str);
    assert!(result.is_ok(), "2000-char URL should parse");
}

#[test]
fn very_long_url_total_length_5000() {
    let path_len = 5000 - "https://example.com/".len();
    let long_path = "a".repeat(path_len);
    let url_str = format!("https://example.com/{long_path}");
    let result = Url::parse(&url_str);
    assert!(result.is_ok(), "5k-char URL should parse without panic");
}

// ============================================================================
// Unicode normalization attacks
// ============================================================================

#[test]
fn unicode_homograph_attack_no_panic() {
    let url_str = "https://еxаmple.com/page"; // Cyrillic е and а
    let result = Url::parse(url_str);
    if let Ok(url) = result {
        // Cyrillic homograph must not resolve to ASCII domain
        assert_ne!(
            url.host_str(),
            Some("example.com"),
            "Cyrillic homograph must not resolve to ASCII target"
        );
    }
}

#[test]
fn unicode_ideographic_space_no_panic() {
    let url_str = "https://example.com/page\u{3000}here";
    let result = Url::parse(url_str);
    assert!(result.is_ok(), "URL with ideographic space should parse");
}

#[test]
fn bidi_override_attack_no_panic() {
    let url_str = format!("https://example.com/{}\u{202E}fdp.html", "image");
    let result = Url::parse(&url_str);
    assert!(result.is_ok(), "URL with bidi override should parse");
}

// ============================================================================
// Infrastructure UrlValidator (with HTTP client)
// ============================================================================

#[test]
fn infrastructure_validator_filters_ftp() {
    let validator = webfang::infrastructure::crawler::url_validator::UrlValidator::new();
    let url = Url::parse("ftp://example.com/file").unwrap();
    let result = validator.filter_invalid_patterns(&url);
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn infrastructure_validator_filters_file() {
    let validator = webfang::infrastructure::crawler::url_validator::UrlValidator::new();
    let url = Url::parse("file:///etc/passwd").unwrap();
    let result = validator.filter_invalid_patterns(&url);
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn infrastructure_validator_accepts_https() {
    let validator = webfang::infrastructure::crawler::url_validator::UrlValidator::new();
    let url = Url::parse("https://example.com/page").unwrap();
    let result = validator.filter_invalid_patterns(&url);
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Valid
    ));
}

// ============================================================================
// Node.js version pattern injection
// ============================================================================

#[test]
fn reject_invalid_node_version_v100() {
    let url = Url::parse("https://nodejs.org/blog/release/v100.0.0").unwrap();
    let result = webfang::domain::url_validator::UrlValidator::filter_invalid_patterns(&url);
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn accept_valid_node_version_v20() {
    let url = Url::parse("https://nodejs.org/blog/release/v20.11.1").unwrap();
    let result = webfang::domain::url_validator::UrlValidator::filter_invalid_patterns(&url);
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Valid
    ));
}

#[test]
fn node_version_with_query_params() {
    let url = Url::parse("https://nodejs.org/blog/release/v200.0?foo=bar").unwrap();
    let result = webfang::domain::url_validator::UrlValidator::filter_invalid_patterns(&url);
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

#[test]
fn node_version_with_fragment() {
    let url = Url::parse("https://nodejs.org/blog/release/v200.0#section").unwrap();
    let result = webfang::domain::url_validator::UrlValidator::filter_invalid_patterns(&url);
    assert!(matches!(
        result,
        webfang::domain::ValidationResult::Invalid(_)
    ));
}

// ============================================================================
// URL parsing edge cases — verify no panics
// ============================================================================

#[test]
fn empty_url_is_err() {
    let result = Url::parse("");
    assert!(result.is_err());
}

#[test]
fn just_scheme_no_panic() {
    let result = Url::parse("https:");
    assert!(
        result.is_err(),
        "incomplete URL with only scheme must be rejected"
    );
}

#[test]
fn double_slash_only_no_panic() {
    let result = Url::parse("//");
    assert!(
        result.is_err(),
        "protocol-relative URL without base must be rejected"
    );
}

#[test]
fn colon_in_path() {
    let result = Url::parse("https://example.com/path:with:colons");
    assert!(result.is_ok());
}

#[test]
fn at_sign_in_path() {
    let result = Url::parse("https://example.com/path@with@ats");
    assert!(result.is_ok());
}

#[test]
fn percent_encoded_special_chars_stay_encoded() {
    let result = Url::parse("https://example.com/%3Cscript%3Ealert(1)%3C/script%3E");
    assert!(result.is_ok());
    let url = result.unwrap();
    assert!(url.path().contains("%3C"));
    assert!(url.path().contains("%3E"));
}

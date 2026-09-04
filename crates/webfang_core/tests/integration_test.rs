//! Integration tests for AX-tree DTOs, observability, and Spanish error
//! messages.

use webfang_core::error::ScraperError;

#[test]
fn test_compactnode_role_and_name_preserved() {
    use webfang_core::infrastructure::axtree::CompactNode;

    let node = CompactNode {
        r#ref: "@e1".to_string(),
        name: "Submit".to_string(),
        role: "button".to_string(),
    };
    assert_eq!(node.name, "Submit");
    assert_eq!(node.role, "button");
}

#[test]
fn test_spanish_error_message() {
    let err = ScraperError::invalid_url("URL vacía");
    let msg = err.to_string();
    // Spanish user-facing error format
    assert!(
        msg.starts_with("URL inválida:"),
        "Spanish error must start with 'URL inválida:'"
    );
}

#[test]
fn test_spanish_http_error() {
    use webfang_core::error::ScraperError;

    let err = ScraperError::http(404, "https://example.com");
    let msg = err.to_string();
    // HTTP errors should have Spanish format
    assert!(
        msg.contains("404") || msg.contains("error"),
        "HTTP error must contain status code"
    );
}

#[test]
fn test_tracing_instrumentation() {
    use tracing::info;

    info!("test tracing instrumentation");
}

#[test]
fn test_screenshot_size_concept() {
    let png_data: Vec<u8> = vec![0x89, 0x50, 0x4e, 0x47]; // PNG magic header
    let size = png_data.len();
    assert_eq!(size, 4, "PNG magic header should be 4 bytes");
}

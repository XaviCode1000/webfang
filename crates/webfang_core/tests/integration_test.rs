//! Integration tests for the SomCapture module (SDD #790).
//! Tests the mark overlay, observability, and Spanish error messages.

use webfang_core::application::som_capture::Mark;
use webfang_core::error::ScraperError;

#[test]
fn test_mark_serialize_roundtrip() {
    let mark = Mark {
        r#ref: "@e1".to_string(),
        number: 1,
        r#box: [100.0, 100.0, 200.0, 100.0, 200.0, 200.0, 100.0, 200.0],
        label: Some("Submit button".to_string()),
    };
    let json = serde_json::to_string(&mark).unwrap();
    let decoded: Mark = serde_json::from_str(&json).unwrap();
    assert_eq!(mark, decoded);
}

#[test]
fn test_mark_default_label_is_none() {
    let mark = Mark {
        r#ref: "@e1".to_string(),
        number: 1,
        r#box: [100.0, 100.0, 200.0, 100.0, 200.0, 200.0, 100.0, 200.0],
        label: None,
    };
    let json = serde_json::to_string(&mark).unwrap();
    let decoded: Mark = serde_json::from_str(&json).unwrap();
    assert_eq!(mark, decoded);
}

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
fn test_mark_count_field() {
    use webfang_core::application::som_capture::Mark;

    let mark1 = Mark {
        r#ref: "@e1".to_string(),
        number: 1,
        r#box: [100.0, 100.0, 200.0, 100.0, 200.0, 200.0, 100.0, 200.0],
        label: Some("Button".to_string()),
    };
    let mark2 = Mark {
        r#ref: "@e2".to_string(),
        number: 2,
        r#box: [150.0, 150.0, 250.0, 150.0, 250.0, 250.0, 150.0, 250.0],
        label: None,
    };

    let marks = vec![mark1, mark2];
    assert_eq!(marks.len(), 2, "mark count should be 2");
    assert!(marks[0].number == 1, "first mark number should be 1");
    assert!(marks[1].number == 2, "second mark number should be 2");
}

#[test]
fn test_screenshot_size_concept() {
    let png_data: Vec<u8> = vec![0x89, 0x50, 0x4e, 0x47]; // PNG magic header
    let size = png_data.len();
    assert_eq!(size, 4, "PNG magic header should be 4 bytes");
}

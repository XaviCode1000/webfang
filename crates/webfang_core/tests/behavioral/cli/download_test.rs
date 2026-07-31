//! Download behavior: images and documents.

use crate::cmd;
use tempfile::TempDir;
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Small 1x1 PNG (valid PNG header)
const PNG_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, 0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

const PAGE_WITH_IMG: &str = r#"
<html><body><article>
<h1>Image Download Test</h1>
<p>Content with embedded image for download testing.</p>
<img src="/photo.png" alt="test photo">
</article></body></html>
"#;

const PAGE_WITH_DOC: &str = r#"
<html><body><article>
<h1>Document Download Test</h1>
<p>Content with linked document for download testing.</p>
<a href="/report.pdf">Download Report</a>
</article></body></html>
"#;

// ---------------------------------------------------------------------------
// --download-images
// ---------------------------------------------------------------------------

#[tokio::test]
async fn download_images_saves_png_file() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_WITH_IMG))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/photo.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG_BYTES.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/page", server.uri()))
        .arg("--single-page")
        .arg("--download-images")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let images_dir = output.path().join("images");
    assert!(
        images_dir.exists(),
        "images/ directory should be created when --download-images is used"
    );

    let image_files: Vec<_> = std::fs::read_dir(&images_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
        .collect();
    assert!(
        !image_files.is_empty(),
        "at least one image file should be downloaded"
    );
}

#[tokio::test]
async fn download_images_png_content_is_valid() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_WITH_IMG))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/photo.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG_BYTES.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/page", server.uri()))
        .arg("--single-page")
        .arg("--download-images")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let images_dir = output.path().join("images");
    let image_files: Vec<_> = std::fs::read_dir(&images_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
        .collect();

    assert!(!image_files.is_empty());
    let saved = std::fs::read(image_files[0].path()).unwrap();
    assert_eq!(saved, PNG_BYTES, "downloaded image should match original");
}

// ---------------------------------------------------------------------------
// --download-documents
// ---------------------------------------------------------------------------

#[tokio::test]
async fn download_documents_saves_pdf_file() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let pdf_content = b"%PDF-1.4 fake pdf content for testing document download";

    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_WITH_DOC))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(pdf_content.to_vec())
                .insert_header("content-type", "application/pdf"),
        )
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/page", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let docs_dir = output.path().join("documents");
    assert!(
        docs_dir.exists(),
        "documents/ directory should be created when --download-documents is used"
    );

    let doc_files: Vec<_> = std::fs::read_dir(&docs_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
        .collect();
    assert!(
        !doc_files.is_empty(),
        "at least one document file should be downloaded"
    );
}

#[tokio::test]
async fn download_documents_pdf_content_is_valid() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let pdf_content = b"%PDF-1.4 fake pdf content for testing document download";

    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_WITH_DOC))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(pdf_content.to_vec())
                .insert_header("content-type", "application/pdf"),
        )
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/page", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let docs_dir = output.path().join("documents");
    let doc_files: Vec<_> = std::fs::read_dir(&docs_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
        .collect();

    assert!(!doc_files.is_empty());
    let saved = std::fs::read(doc_files[0].path()).unwrap();
    assert_eq!(
        saved, pdf_content,
        "downloaded document should match original"
    );
}

// ---------------------------------------------------------------------------
// Direct PDF URL download (--download-documents with binary content-type)
// ---------------------------------------------------------------------------

/// When a page returns binary content-type (application/pdf), the raw bytes
/// are saved to a file in the output directory.
#[tokio::test]
async fn download_pdf_saves_binary_file() {
    let server = MockServer::start().await;
    let output = tempfile::TempDir::new().unwrap();

    let pdf_content = b"%PDF-1.4 fake pdf content for testing binary download feature";

    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(pdf_content.to_vec())
                .insert_header("content-type", "application/pdf"),
        )
        .expect(1)
        .mount(&server)
        .await;

    crate::cmd()
        .arg("--url")
        .arg(format!("{}/report.pdf", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let pdf_files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "pdf"))
        .collect();

    assert!(
        !pdf_files.is_empty(),
        "a .pdf file should exist in the output directory when downloading a PDF URL"
    );

    let saved = std::fs::read(pdf_files[0].path()).unwrap();
    assert_eq!(
        saved, pdf_content,
        "saved PDF content should match the original"
    );
}

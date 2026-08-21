//! Sitemap exit-code contract suite (RED — stabilization-sitemap-regression, PR A).
//!
//! Pins the exit-code semantics for every sitemap discovery failure/success mode:
//! - exit 2  → "no URLs discovered" (empty urlset, missing sitemap, empty children)
//! - exit 69 → fetch/parse/config failures (404 explicit sitemap, malformed XML,
//!             invalid `--max-depth 0`, all children failing)
//! - exit 0  → success paths (double-encoded gzip, image namespace, HEAD 405 fallback)
//!
//! RED expectations at commit time:
//! - `max_depth_zero_use_sitemap_exits_69` currently exits 2 (want 69)
//! - `head_405_get_200_exits_0` currently exits 2 / false-negative (want 0)
//! - `index_children_all_fail_exits_69` proves string-coupling (want 69)
//! - scenarios 1–7 are regression guards (may already be green).

use crate::{assert_snapshot_redacted, cmd};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Article HTML comfortably over the 50-char minimum content guard.
const ARTICLE_HTML: &str = "<html><body><article>\
     <h1>Sitemap Page</h1>\
     <p>Substantive content from a sitemap-listed page, long enough to clear \
     the fifty character minimum content guard comfortably.</p>\
     </article></body></html>";

const EMPTY_URLSET: &str =
    r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"></urlset>"#;

fn urlset_with(locs: &[String]) -> String {
    let urls: String = locs
        .iter()
        .map(|loc| format!("<url><loc>{loc}</loc></url>"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</urlset>"#
    )
}

fn sitemap_index_with(locs: &[String]) -> String {
    let urls: String = locs
        .iter()
        .map(|loc| format!("<sitemap><loc>{loc}</loc></sitemap>"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</sitemapindex>"#
    )
}

async fn mount_seed_and_robots(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
        .mount(server)
        .await;
}

/// Compress `data` with gzip using the existing workspace dependency
/// (`async-compression` only exposes `tokio::bufread` adapters).
async fn gzip_compress(data: &[u8]) -> Vec<u8> {
    use async_compression::tokio::bufread::GzipEncoder;
    use tokio::io::{AsyncReadExt, BufReader};

    let mut encoder = GzipEncoder::new(BufReader::new(std::io::Cursor::new(data)));
    let mut out = Vec::new();
    encoder.read_to_end(&mut out).await.unwrap();
    out
}

// ---------------------------------------------------------------------------
// 1. Empty urlset → exit 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_urlset_exits_2() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(EMPTY_URLSET))
        .mount(&server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(2),
        "an empty urlset must exit 2 (no URLs discovered)"
    );
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    assert_snapshot_redacted("empty_urlset_stderr", output.path(), stderr);
}

// ---------------------------------------------------------------------------
// 2. Missing sitemap, auto-discovery finds nothing → exit 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_sitemap_auto_discovery_exits_2() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Only seed page + robots.txt; NO sitemap stubs — unmatched requests 404
    // across every discovery tier.
    mount_seed_and_robots(&server).await;

    let result = cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(2),
        "auto-discovery with no sitemap anywhere must exit 2"
    );
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    assert_snapshot_redacted("missing_sitemap_auto_discovery_stderr", output.path(), stderr);
}

// ---------------------------------------------------------------------------
// 3. Sitemap index whose children are all empty → exit 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn index_all_children_empty_exits_2() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();

    let children = [format!("{base}/child-a.xml"), format!("{base}/child-b.xml")];
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sitemap_index_with(&children)))
        .mount(&server)
        .await;
    for child in ["/child-a.xml", "/child-b.xml"] {
        Mock::given(method("GET"))
            .and(path(child))
            .respond_with(ResponseTemplate::new(200).set_body_string(EMPTY_URLSET))
            .mount(&server)
            .await;
    }

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(2),
        "an index whose children are all empty must exit 2"
    );
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    assert_snapshot_redacted("index_all_children_empty_stderr", output.path(), stderr);
}

// ---------------------------------------------------------------------------
// 4. Explicit --sitemap-url 404 → exit 69
// ---------------------------------------------------------------------------

#[tokio::test]
async fn explicit_sitemap_404_exits_69() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{base}/sitemap.xml"))
        .arg("--output")
        .arg(output.path())
        .arg("--max-retries")
        .arg("0")
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(69),
        "an explicit sitemap 404 must exit 69 (fetch failure)"
    );
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    assert_snapshot_redacted("explicit_sitemap_404_stderr", output.path(), stderr);
}

// ---------------------------------------------------------------------------
// 5. Malformed XML sitemap → exit 69
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_xml_exits_69() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string("this is not xml <<<"))
        .mount(&server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(69),
        "malformed sitemap XML must exit 69 (parse failure)"
    );
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    assert_snapshot_redacted("malformed_xml_stderr", output.path(), stderr);
}

// ---------------------------------------------------------------------------
// 6. Double-encoded gzip sitemap → exit 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gzip_double_encoded_valid_exits_0() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();

    mount_seed_and_robots(&server).await;

    // Body is gzip-of-gzip of a valid urlset: transport decoding strips one
    // layer, the extension handler must survive the second.
    let page_url = format!("{base}/article");
    let xml = urlset_with(&[page_url]);
    let once = gzip_compress(xml.as_bytes()).await;
    let twice = gzip_compress(&once).await;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(twice))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{base}/sitemap.xml.gz"))
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(0),
        "a double-gzipped valid sitemap must succeed, stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

// ---------------------------------------------------------------------------
// 7. Image-namespace sitemap → exit 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn image_namespace_sitemap_exits_0() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();

    mount_seed_and_robots(&server).await;

    let page_url = format!("{base}/gallery");
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
    <url>
        <loc>{page_url}</loc>
        <image:image><image:loc>{base}/img.jpg</image:loc></image:image>
    </url>
</urlset>"#
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gallery"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(0),
        "an image-namespace sitemap must succeed, stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

// ---------------------------------------------------------------------------
// 8. --max-depth 0 with --use-sitemap → exit 69 (RED: currently exits 2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_depth_zero_use_sitemap_exits_69() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(urlset_with(&[
            format!("{base}/article"),
        ])))
        .mount(&server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--max-depth")
        .arg("0")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(69),
        "--max-depth 0 with --use-sitemap is a config failure (69), not 'no URLs' (2)"
    );
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    assert_snapshot_redacted("max_depth_zero_stderr", output.path(), stderr);
}

// ---------------------------------------------------------------------------
// 9. HEAD 405 then GET 200 → exit 0 (RED: currently 2 / false-negative)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn head_405_get_200_exits_0() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();

    mount_seed_and_robots(&server).await;

    Mock::given(method("HEAD"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(405))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(urlset_with(&[
            format!("{base}/article"),
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(0),
        "HEAD 405 must fall back to GET; the run must succeed, stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

// ---------------------------------------------------------------------------
// 10. Index whose children all fail → exit 69 (proves string-coupling)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn index_children_all_fail_exits_69() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();

    let children = [format!("{base}/child-a.xml"), format!("{base}/child-b.xml")];
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sitemap_index_with(&children)))
        .mount(&server)
        .await;
    for child in ["/child-a.xml", "/child-b.xml"] {
        Mock::given(method("GET"))
            .and(path(child))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&server)
            .await;
    }

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--output")
        .arg(output.path())
        .arg("--max-retries")
        .arg("0")
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        result.status.code(),
        Some(69),
        "an index whose children ALL fail must exit 69 (fetch failure), not 2"
    );
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    assert_snapshot_redacted("index_children_all_fail_stderr", output.path(), stderr);
}

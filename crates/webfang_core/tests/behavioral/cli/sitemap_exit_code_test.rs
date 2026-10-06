//! Sitemap exit-code contract suite (stabilization-sitemap-regression, PR A).
//!
//! Pins the exit-code semantics for every sitemap discovery failure/success mode:
//! - exit 2  → "no URLs discovered" (empty urlset, missing sitemap, empty children)
//! - exit 69 → fetch/parse/config failures (404 explicit sitemap, malformed XML,
//!   invalid `--max-depth 0`, all children failing)
//! - exit 0  → success paths (double-encoded gzip, image namespace, HEAD 405 fallback)
//!
//! Regression guards: `max_depth_zero_use_sitemap_exits_69`,
//! `head_405_get_200_exits_0`, and `index_children_all_fail_exits_69` pin the
//! typed-error fixes (string-match removal, HEAD→GET fallback, max-depth guard).
//! - scenarios 1–7 are regression guards (may already be green).

use crate::{assert_snapshot_redacted, cmd, redact_nondeterministic};
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

/// Start a fresh mock server with a temp output dir for one exit-code scenario.
async fn start_scenario() -> (MockServer, TempDir) {
    (MockServer::start().await, TempDir::new().unwrap())
}

/// Mount a sitemap body at `/sitemap.xml` with `200 OK`.
async fn mount_sitemap_xml(server: &MockServer, body: &str) {
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

/// Mount a two-child sitemap index at `/sitemap.xml` plus both children,
/// each served with `status` and `body`.
async fn mount_index_with_children(server: &MockServer, base: &str, status: u16, body: &str) {
    let children = [format!("{base}/child-a.xml"), format!("{base}/child-b.xml")];
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sitemap_index_with(&children)))
        .mount(server)
        .await;
    for child in ["/child-a.xml", "/child-b.xml"] {
        Mock::given(method("GET"))
            .and(path(child))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(server)
            .await;
    }
}

/// Run the sitemap command against `url`, inserting `extra` args between
/// `--use-sitemap` and `--output <dir> --quiet`.
fn run_sitemap_cmd(output: &TempDir, url: &str, extra: &[&str]) -> std::process::Output {
    let mut command = cmd();
    command.arg("--url").arg(url).arg("--use-sitemap");
    command.args(extra);
    command
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run webfang")
}

/// Snapshot a finished run's stderr with the standard redaction chain.
fn snapshot_stderr(name: &str, output: &TempDir, result: &std::process::Output) {
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    assert_snapshot_redacted(name, output.path(), stderr);
}

// ---------------------------------------------------------------------------
// 1. Empty urlset → exit 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_urlset_exits_2() {
    let (server, output) = start_scenario().await;

    mount_sitemap_xml(&server, EMPTY_URLSET).await;

    let result = run_sitemap_cmd(&output, &server.uri(), &[]);

    assert_eq!(
        result.status.code(),
        Some(2),
        "an empty urlset must exit 2 (no URLs discovered)"
    );
    snapshot_stderr("empty_urlset_stderr", &output, &result);
}

// ---------------------------------------------------------------------------
// 2. Missing sitemap, auto-discovery finds nothing → exit 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_sitemap_auto_discovery_exits_2() {
    let (server, output) = start_scenario().await;

    // Only seed page + robots.txt; NO sitemap stubs — unmatched requests 404
    // across every discovery tier.
    mount_seed_and_robots(&server).await;

    let result = run_sitemap_cmd(&output, &server.uri(), &[]);

    assert_eq!(
        result.status.code(),
        Some(2),
        "auto-discovery with no sitemap anywhere must exit 2"
    );
    snapshot_stderr("missing_sitemap_auto_discovery_stderr", &output, &result);
}

// ---------------------------------------------------------------------------
// 3. Sitemap index whose children are all empty → exit 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn index_all_children_empty_exits_2() {
    let (server, output) = start_scenario().await;
    let base = server.uri();

    mount_index_with_children(&server, &base, 200, EMPTY_URLSET).await;

    let result = run_sitemap_cmd(&output, &base, &[]);

    assert_eq!(
        result.status.code(),
        Some(2),
        "an index whose children are all empty must exit 2"
    );
    snapshot_stderr("index_all_children_empty_stderr", &output, &result);
}

// ---------------------------------------------------------------------------
// 4. Explicit --sitemap-url 404 → exit 69
// ---------------------------------------------------------------------------

#[tokio::test]
async fn explicit_sitemap_404_exits_69() {
    let (server, output) = start_scenario().await;
    let base = server.uri();

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&server)
        .await;

    let result = run_sitemap_cmd(
        &output,
        &base,
        &[
            "--sitemap-url",
            &format!("{base}/sitemap.xml"),
            "--max-retries",
            "0",
        ],
    );

    assert_eq!(
        result.status.code(),
        Some(69),
        "an explicit sitemap 404 must exit 69 (fetch failure)"
    );
    snapshot_stderr("explicit_sitemap_404_stderr", &output, &result);
}

// ---------------------------------------------------------------------------
// 5. Malformed XML sitemap → exit 69
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_xml_exits_69() {
    let (server, output) = start_scenario().await;
    let base = server.uri();

    mount_sitemap_xml(&server, "this is not xml <<<").await;

    let result = run_sitemap_cmd(&output, &base, &[]);

    assert_eq!(
        result.status.code(),
        Some(69),
        "malformed sitemap XML must exit 69 (parse failure)"
    );
    snapshot_stderr("malformed_xml_stderr", &output, &result);
}

// ---------------------------------------------------------------------------
// 6. Double-encoded gzip sitemap → exit 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gzip_double_encoded_valid_exits_0() {
    let (server, output) = start_scenario().await;
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

    let result = run_sitemap_cmd(
        &output,
        &base,
        &["--sitemap-url", &format!("{base}/sitemap.xml.gz")],
    );

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
    let (server, output) = start_scenario().await;
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

    let result = run_sitemap_cmd(&output, &base, &[]);

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
    let (server, output) = start_scenario().await;
    let base = server.uri();

    mount_sitemap_xml(&server, &urlset_with(&[format!("{base}/article")])).await;

    let result = run_sitemap_cmd(&output, &base, &["--max-depth", "0"]);

    assert_eq!(
        result.status.code(),
        Some(69),
        "--max-depth 0 with --use-sitemap is a config failure (69), not 'no URLs' (2)"
    );
    snapshot_stderr("max_depth_zero_stderr", &output, &result);
}

// ---------------------------------------------------------------------------
// 9. HEAD 405 then GET 200 → exit 0 (RED: currently 2 / false-negative)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn head_405_get_200_exits_0() {
    let (server, output) = start_scenario().await;
    let base = server.uri();

    mount_seed_and_robots(&server).await;

    Mock::given(method("HEAD"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(405))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(urlset_with(&[format!("{base}/article")])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&server)
        .await;

    let result = run_sitemap_cmd(&output, &base, &[]);

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
    let (server, output) = start_scenario().await;
    let base = server.uri();

    mount_index_with_children(&server, &base, 500, "Internal Server Error").await;

    let result = run_sitemap_cmd(&output, &base, &["--max-retries", "0"]);

    assert_eq!(
        result.status.code(),
        Some(69),
        "an index whose children ALL fail must exit 69 (fetch failure), not 2"
    );
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();

    // The per-child fetch rejections are emitted from concurrent child futures,
    // so their relative order on stderr is NOT guaranteed: pin presence only
    // (#1317). Since #1318 the rejection is a `log_scrape_error` event, so the
    // pin is on the context message and its `stage` field rather than on the
    // old inline `status: 500 from <url>` sentence — checked on the redacted
    // stderr because the fmt layer paints field names with ANSI, which would
    // break a raw `contains` across the `stage:` boundary.
    let logged = redact_nondeterministic(output.path(), &stderr);
    assert!(
        logged.contains("Sitemap URL returned non-2xx status")
            && logged.contains("stage: sitemap.fetch")
            && logged.contains("http request failed: 500"),
        "each failing child must log its non-2xx rejection with stage=sitemap.fetch, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("/child-a.xml") && stderr.contains("/child-b.xml"),
        "both failing children must be reported with their URLs, stderr:\n{stderr}"
    );

    // The aggregated error is built from results collected in input order
    // (`buffered` preserves result order even with concurrent fetches), so the
    // aggregation always lists child-a before child-b. That ordering IS
    // deterministic: keep it pinned, anchored on the aggregation message.
    let agg = &stderr[stderr
        .find("all 2 child sitemaps failed")
        .expect("aggregated failure message must be present in stderr")..];
    let a_pos = agg
        .find("child-a.xml")
        .expect("child-a must appear in the aggregation");
    let b_pos = agg
        .find("child-b.xml")
        .expect("child-b must appear in the aggregation");
    assert!(
        a_pos < b_pos,
        "aggregation must list children in input order (child-a before child-b), stderr:\n{stderr}"
    );
}

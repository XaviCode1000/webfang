//! Literal-IP SSRF rejection in the asset download chain (PI-1 / SEC F1).
//!
//! Asset URLs are extracted from caller-supplied HTML, so `ValidUrl`'s
//! syntactic validation (#1117) cannot protect them: an attacker page can
//! point `<img src>` at the cloud-metadata or loopback address and the
//! download stage would dial it. These tests pin that
//! [`download_assets_if_enabled`] skips forbidden literal targets BEFORE any
//! socket opens, while a legitimate target still downloads.
//!
//! wiremock binds `127.0.0.1` — itself a forbidden literal — so the positive
//! case arms the documented entry-guard hatch via `EnvGuard::entry_guard_off`
//! (exact `"1"`; production never sets it, #1217/#1578). With the hatch armed
//! the forbidden literals would be dialed for real, so the negative case runs
//! in its own test under a hatch-free environment instead of a mixed batch.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use webfang_core::application::asset_download::download_assets_if_enabled;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;

/// Guard armed (hatch removed): every asset URL in the page is a forbidden
/// literal — cloud metadata, loopback port 9, or the loopback wiremock
/// itself — so the whole batch must be skipped: `Ok(vec![])`, zero mock hits.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
async fn forbidden_literal_asset_targets_are_skipped_without_any_socket() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_ENTRY_GUARD_ENV]);

    let server = MockServer::start().await;
    // Tripwire: the filter must short-circuit before any downloader is built.
    // If a rejected URL ever reached the network layer it would either hit
    // this mock or stall dialing the unroutable metadata address.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("never"))
        .expect(0)
        .mount(&server)
        .await;

    let html = format!(
        "<html><body>\
         <img src=\"http://169.254.169.254/latest/meta-data/logo.png\">\
         <img src=\"http://127.0.0.1:9/x.png\">\
         <img src=\"{}/logo.png\">\
         </body></html>",
        server.uri()
    );
    let base_url = url::Url::parse("https://attacker.example/page").expect("valid url");
    let config = ScraperConfig::default().with_images();

    let result = download_assets_if_enabled(&html, &base_url, &config, None)
        .await
        .expect("rejections must be skips, not errors");

    assert!(
        result.is_empty(),
        "no forbidden literal may yield a downloaded asset: {result:?}"
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("request recording is enabled")
            .len(),
        0,
        "the literal-IP filter must short-circuit BEFORE any socket"
    );
}

/// Positive control: with the entry-guard hatch armed, a legitimate asset on
/// the wiremock host downloads normally — the filter only removes forbidden
/// literals and leaves the rest of the pipeline untouched.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
async fn legitimate_hostname_asset_downloads_with_entry_guard_disarmed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/logo.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"\x89PNG-fake-bytes".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    // wiremock is 127.0.0.1: the hatch must be armed BEFORE the download.
    let _guard = webfang_test_utils::EnvGuard::entry_guard_off();

    let html = format!(
        "<html><body><img src=\"{}/logo.png\"></body></html>",
        server.uri()
    );
    let base_url = url::Url::parse("https://example.com/page").expect("valid url");
    let output = tempfile::TempDir::new().expect("temp output dir");
    let config = ScraperConfig::default()
        .with_images()
        .with_output_dir(output.path().to_path_buf());

    let result = download_assets_if_enabled(&html, &base_url, &config, None)
        .await
        .expect("the legitimate asset must download");

    assert_eq!(result.len(), 1, "exactly the legitimate asset downloads");
    assert!(
        result[0].url.ends_with("/logo.png"),
        "unexpected asset url: {}",
        result[0].url
    );
    assert_eq!(result[0].asset_type, "image");
    assert!(result[0].size > 0, "the served bytes must reach the asset");
}

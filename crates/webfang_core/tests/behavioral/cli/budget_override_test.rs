//! Budget-override flag-to-enforcement paths (#897).
//!
//! Every concurrency knob must survive its WHOLE pipeline:
//! source → normalize / `From<Args>` → the binary's final merge →
//! `BudgetModel::build` → enforcement site.
//!
//! The enforcement sites log their effective bound at INFO (`-v`), which
//! gives deterministic observables without timing assertions:
//!
//! - scrape path: `scraping with bounded concurrency` with a structured
//!   `concurrency` field (`scrape_flow.rs`, right before `buffer_unordered`)
//! - batch path: `Starting batch processing: N URLs, concurrency=X`
//!   (`orchestrator.rs prepare_batch_manager`)
//! - asset path: `Asset downloads wired` with structured
//!   `asset_concurrency` (`orchestrator.rs prepare_phase`)
//!
//! Auto-detection NEVER derives a crawl budget of exactly 2
//! (1–2 cores → 1, 3–4 cores → 3, 5–7 → 5, 8+ → min(cores−1, 8)), so an
//! effective crawl bound of 2 can only come from an explicit override that
//! reached the model.

use crate::cmd;
use regex::Regex;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Write `$XDG_CONFIG_HOME/webfang/config.toml` with the given TOML body and
/// return the temp dir backing it. The CLI resolves its config through
/// `dirs::config_dir()`, which honors `XDG_CONFIG_HOME` on Linux.
fn write_toml_config(body: &str) -> TempDir {
    let xdg = TempDir::new().expect("create XDG temp dir");
    let conf_dir = xdg.path().join("webfang");
    std::fs::create_dir_all(&conf_dir).expect("create webfang config dir");
    std::fs::write(conf_dir.join("config.toml"), body).expect("write config.toml");
    xdg
}

/// Standard two-page discovery mock: robots.txt allow-all plus a sitemap
/// listing `/page-a` and `/page-b`, so the multi-URL scrape enforcement site
/// runs.
async fn mount_two_page_site(server: &MockServer) {
    let base = server.uri();
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/page-a</loc></url>
    <url><loc>{base}/page-b</loc></url>
</urlset>"#
        )))
        .mount(server)
        .await;
    for page in ["/", "/page-a", "/page-b"] {
        Mock::given(method("GET"))
            .and(path(page))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<html><body><article>\
                     <h1>Page</h1>\
                     <p>Substantive content long enough to clear the fifty \
                     character minimum content guard comfortably.</p>\
                     </article></body></html>",
            ))
            .mount(server)
            .await;
    }
}

/// Strip ANSI escapes, then extract the effective crawl-concurrency value
/// logged by the scrape enforcement site (`scraping with bounded
/// concurrency` + structured `concurrency` field; the pretty tracing layer
/// may render field and message on different lines).
fn logged_scrape_concurrency(stderr: &str) -> usize {
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").expect("valid regex");
    let clean = ansi.replace_all(stderr, "");
    let re = Regex::new(r"bounded concurrency[\s\S]{0,400}?concurrency[=:]\s*(\d+)")
        .expect("valid regex");
    let caps = re.captures(&clean).unwrap_or_else(|| {
        panic!("enforcement-site concurrency log not found; stderr was:\n{clean}")
    });
    caps[1].parse().expect("concurrency is numeric")
}

/// Assert a structured tracing FIELD reached stderr. The pretty layer may
/// render fields with `=` (compact) or `: ` (pretty), wrapped in ANSI
/// escapes — normalize and match both.
fn assert_structured_field(stderr: &str, field: &str, value: usize) {
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").expect("valid regex");
    let clean = ansi.replace_all(stderr, "");
    let re = Regex::new(&format!(r"{field}[=:]\s*{value}\b")).expect("valid regex");
    assert!(
        re.is_match(&clean),
        "structured field `{field}={value}` not found in stderr:\n{clean}"
    );
}

/// #897 item 1: a TOML-sourced `concurrency = "2"` must reach the scrape
/// enforcement site through normalize → into_crawl_options → the binary's
/// final merge → BudgetModel::build. Before the fix the merge dropped the
/// projected overrides entirely, so the model silently fell back to the
/// hardware-derived auto tier.
#[tokio::test]
async fn toml_concurrency_reaches_scrape_enforcement() {
    let server = MockServer::start().await;
    mount_two_page_site(&server).await;
    let output = TempDir::new().expect("temp output dir");
    let _xdg = write_toml_config("concurrency = \"2\"\n");

    let assert = cmd()
        .env("XDG_CONFIG_HOME", _xdg.path())
        .args([
            "--url",
            &server.uri(),
            "--use-sitemap",
            "--sitemap-url",
            &format!("{}/sitemap.xml", server.uri()),
            "--output",
            output.path().to_string_lossy().as_ref(),
            "-v",
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    let effective = logged_scrape_concurrency(&stderr);
    assert_eq!(
        effective, 2,
        "TOML-sourced concurrency=2 must drive the enforcement bound; stderr:\n{stderr}"
    );
}

/// #897 triangulation: an explicitly supplied CLI `--concurrency` outranks
/// the TOML default (CLI rank > ConfigFile rank), and it must KEEP winning
/// after the field-wise merge lands.
#[tokio::test]
async fn cli_concurrency_flag_outranks_toml_config() {
    let server = MockServer::start().await;
    mount_two_page_site(&server).await;
    let output = TempDir::new().expect("temp output dir");
    let _xdg = write_toml_config("concurrency = \"2\"\n");

    let assert = cmd()
        .env("XDG_CONFIG_HOME", _xdg.path())
        .args([
            "--url",
            &server.uri(),
            "--use-sitemap",
            "--sitemap-url",
            &format!("{}/sitemap.xml", server.uri()),
            "--output",
            output.path().to_string_lossy().as_ref(),
            "--concurrency",
            "5",
            "-v",
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    let effective = logged_scrape_concurrency(&stderr);
    assert_eq!(
        effective, 5,
        "explicit --concurrency must outrank the TOML default; stderr:\n{stderr}"
    );
}

/// #897 item 5: `--batch-concurrency` must reach its enforcement path —
/// `prepare_batch_manager` logs the model's Operation.batch tier it actually
/// applies.
#[tokio::test]
async fn batch_concurrency_flag_reaches_model_tier() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Batch</h1><p>Substantive batch content long enough to clear the fifty character minimum content guard comfortably.</p></article></body></html>",
        ))
        .mount(&server)
        .await;
    let output = TempDir::new().expect("temp output dir");
    let urls = output.path().join("urls.txt");
    std::fs::write(&urls, format!("{}\n", server.uri())).expect("write batch file");

    let assert = cmd()
        .args([
            "--batch-file",
            urls.to_string_lossy().as_ref(),
            "--output",
            output.path().to_string_lossy().as_ref(),
            "--batch-concurrency",
            "4",
            "-v",
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("concurrency=4"),
        "--batch-concurrency 4 must reach the model's batch tier log; stderr:\n{stderr}"
    );
}

/// #897 item 5: `--download-concurrency` must reach its enforcement site —
/// `prepare_phase` wires the model's Asset tier into the scraper config and
/// logs the effective bound as a STRUCTURED field (m1: never interpolate
/// values into the message). Default is 3, so 7 proves the explicit flag
/// arrived through the merge.
#[tokio::test]
async fn download_concurrency_flag_reaches_asset_tier() {
    let server = MockServer::start().await;
    mount_two_page_site(&server).await;
    let output = TempDir::new().expect("temp output dir");

    let assert = cmd()
        .args([
            "--url",
            &server.uri(),
            "--output",
            output.path().to_string_lossy().as_ref(),
            "--download-concurrency",
            "7",
            "-v",
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert_structured_field(&stderr, "asset_concurrency", 7);
}

/// #897 triangulation (sharpest slot-copy guard): TOML crawl concurrency AND
/// an explicit CLI download flag must survive the SAME merge simultaneously.
/// A wholesale `opts.budget_overrides = projected.budget_overrides` would
/// keep crawl=2 but wipe the CLI asset override back to the default 3; the
/// field-wise merge must deliver both.
#[tokio::test]
async fn toml_crawl_and_cli_download_survive_same_merge() {
    let server = MockServer::start().await;
    mount_two_page_site(&server).await;
    let output = TempDir::new().expect("temp output dir");
    let _xdg = write_toml_config("concurrency = \"2\"\n");

    let assert = cmd()
        .env("XDG_CONFIG_HOME", _xdg.path())
        .args([
            "--url",
            &server.uri(),
            "--output",
            output.path().to_string_lossy().as_ref(),
            "--download-concurrency",
            "6",
            "-v",
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert_eq!(
        logged_scrape_concurrency(&stderr),
        2,
        "TOML crawl concurrency must survive the merge; stderr:\n{stderr}"
    );
    assert_structured_field(&stderr, "asset_concurrency", 6);
}

/// #897 item 2 — Zero Silent Loss: an explicit `--rate-limit-burst 0` on
/// the CLI flag path must hard-error with the Spanish boundary message and
/// exit 78 (ConfigError), never silently degrade to the derived default.
/// Fails before any network I/O, so no mock server is needed.
#[test]
fn cli_rate_limit_burst_zero_hard_errors() {
    let output = TempDir::new().expect("temp output dir");

    let assert = cmd()
        .args([
            "--url",
            "https://example.com",
            "--output",
            output.path().to_string_lossy().as_ref(),
            "--rate-limit-burst",
            "0",
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .failure();

    assert_eq!(
        assert.get_output().status.code(),
        Some(78),
        "rejected burst 0 must exit 78 (ConfigError)"
    );
}

/// #897 item 2 — Zero Silent Loss, TOML path: a config-file-sourced
/// `rate_limit_burst = 0` must also hard-error with exit 78 (ConfigError),
/// never silently degrade to the derived default. Fails before any network
/// I/O, so no mock server is needed. (Preserved from the #925 landing.)
#[test]
fn toml_rate_limit_burst_zero_hard_errors() {
    let output = TempDir::new().expect("temp output dir");
    let _xdg = write_toml_config("rate_limit_burst = 0\n");

    let assert = cmd()
        .env("XDG_CONFIG_HOME", _xdg.path())
        .args([
            "--url",
            "https://example.com",
            "--output",
            output.path().to_string_lossy().as_ref(),
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .failure();

    assert_eq!(
        assert.get_output().status.code(),
        Some(78),
        "TOML-sourced burst 0 must exit 78 (ConfigError)"
    );
}

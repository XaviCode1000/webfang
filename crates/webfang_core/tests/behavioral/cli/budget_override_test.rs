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
use assert_cmd::assert::Assert;
use regex::Regex;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Write a `config.toml` with the given TOML body, returning the temp dir
/// backing it plus the exact config file path.
///
/// The path is returned so the caller can point `WEBFANG_CONFIG` at it, which
/// is what makes this helper platform-independent (#1631). It used to return
/// only the dir and rely on the caller setting `XDG_CONFIG_HOME` to the parent
/// of a `webfang/` subdirectory. That is Linux-shaped in two ways, and the
/// first attempt at this helper only fixed one: `XDG_CONFIG_HOME` was read
/// through a private `main.rs` copy of the resolver that called
/// `dirs::config_dir()` raw, so macOS ignored it silently and Windows could
/// not be redirected by env at all. The resolver is now the shared one and
/// `domain::paths` honors an ABSOLUTE `XDG_CONFIG_HOME` on every platform, so
/// the env var alone would work again — but it names a DIRECTORY and the
/// shared resolver appends `webfang/` to it, while this helper writes the file
/// at the root. `WEBFANG_CONFIG` names the file itself and drops that
/// assumption. The temp dir must be kept alive by the caller for the same
/// reason it always was — `Command` cannot own a `TempDir`'s lifetime.
fn write_toml_config(body: &str) -> (TempDir, PathBuf) {
    let conf_dir = TempDir::new().expect("create config temp dir");
    let config_path = conf_dir.path().join("config.toml");
    std::fs::write(&config_path, body).expect("write config.toml");
    (conf_dir, config_path)
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

/// Spawn `webfang` scraping `server`'s base URL into `output`, with `envs`
/// applied to the child process and `extra_args` appended after the fixed
/// `--url … --output …` prefix, returning the finished command assert.
///
/// No verbosity flag is added: tests that need `-v` pass it in `extra_args`,
/// because the fail-closed family below asserts at DEFAULT verbosity.
fn scrape(
    server: &MockServer,
    envs: &[(&str, &str)],
    output: &TempDir,
    extra_args: &[&str],
) -> Assert {
    let mut command = cmd();
    for (key, value) in envs {
        command.env(key, value);
    }
    command
        .args([
            "--url",
            server.uri().as_str(),
            "--output",
            output.path().to_string_lossy().as_ref(),
        ])
        .args(extra_args)
        .timeout(Duration::from_secs(60))
        .assert()
}

/// Mount a single catch-all page so a run that PASSES validation has a
/// hermetic, instant success target instead of the real network.
///
/// Required for the fail-closed cases below: pre-fix they do NOT fail, so
/// without a mock they would dial `example.com` and the RED observation would
/// depend on the machine's connectivity (slow, and possibly a flaky 69/74
/// instead of a clean 0).
async fn mount_single_page_site(server: &MockServer) {
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Page</h1>\
             <p>Substantive batch content long enough to clear the fifty \
             character minimum content guard comfortably.</p></article></body></html>",
        ))
        .mount(server)
        .await;
}

/// Arrange the fail-closed family's fixture (fresh single-page mock site +
/// temp output dir) and run a scrape against it with `envs` applied to the
/// child and `extra_args` appended, returning the output dir (the caller
/// keeps it alive for snapshot redaction) plus the finished assert.
async fn run_single_page_scrape(envs: &[(&str, &str)], extra_args: &[&str]) -> (TempDir, Assert) {
    let server = MockServer::start().await;
    mount_single_page_site(&server).await;
    let output = TempDir::new().expect("temp output dir");
    let assert = scrape(&server, envs, &output, extra_args);
    (output, assert)
}

/// Assert a run FAILED and did so with the fail-closed ConfigError exit
/// code (78) — the `.failure()` outcome assertion plus the exact code —
/// returning the assert so callers can still inspect stderr.
fn assert_exit_78(assert: Assert, context: &str) -> Assert {
    let assert = assert.failure();
    assert_eq!(assert.get_output().status.code(), Some(78), "{context}");
    assert
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
    let (_conf, conf_path) = write_toml_config("concurrency = \"2\"\n");
    let sitemap_url = format!("{}/sitemap.xml", server.uri());

    let assert = scrape(
        &server,
        &[("WEBFANG_CONFIG", conf_path.to_string_lossy().as_ref())],
        &output,
        &["--use-sitemap", "--sitemap-url", &sitemap_url, "-v"],
    )
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
    let (_conf, conf_path) = write_toml_config("concurrency = \"2\"\n");
    let sitemap_url = format!("{}/sitemap.xml", server.uri());

    let assert = scrape(
        &server,
        &[("WEBFANG_CONFIG", conf_path.to_string_lossy().as_ref())],
        &output,
        &[
            "--use-sitemap",
            "--sitemap-url",
            &sitemap_url,
            "--concurrency",
            "5",
            "-v",
        ],
    )
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

    let assert = scrape(
        &server,
        &[],
        &output,
        &["--download-concurrency", "7", "-v"],
    )
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
    let (_conf, conf_path) = write_toml_config("concurrency = \"2\"\n");

    let assert = scrape(
        &server,
        &[("WEBFANG_CONFIG", conf_path.to_string_lossy().as_ref())],
        &output,
        &["--download-concurrency", "6", "-v"],
    )
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

/// #1813 (fail-closed family) — Zero Silent Loss, non-numeric arm:
/// `--rate-limit-burst banana` must hard-error with a Spanish config error and
/// exit 78, never degrade silently to the hardware-derived default.
///
/// This is the arm the #897 tests never covered: the parser returned
/// `Ok(None)` plus a pre-logging note, so a typo'd burst silently changed the
/// request cadence. Fails before any network I/O, so the mock is only there to
/// give the pre-fix run a fast hermetic success.
#[tokio::test]
async fn cli_rate_limit_burst_non_numeric_fails_closed() {
    let (output, assert) = run_single_page_scrape(&[], &["--rate-limit-burst", "banana"]).await;

    let assert = assert_exit_78(
        assert,
        "a non-numeric burst must exit 78 (ConfigError), not degrade to the derived default",
    );
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    crate::assert_snapshot_redacted(
        "burst_non_numeric_fails_closed_stderr",
        output.path(),
        stderr,
    );
}

/// #1813 triangulation — the SAME fail-closed contract for an
/// out-of-`u32`-range burst delivered through the CLI flag. `4294967296` is
/// `u32::MAX + 1`, i.e. an explicit request that cannot be honoured.
#[tokio::test]
async fn cli_rate_limit_burst_out_of_range_fails_closed() {
    let (output, assert) = run_single_page_scrape(&[], &["--rate-limit-burst", "4294967296"]).await;

    let assert = assert_exit_78(
        assert,
        "an out-of-u32-range burst must exit 78 (ConfigError)",
    );
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    crate::assert_snapshot_redacted(
        "burst_out_of_range_fails_closed_stderr",
        output.path(),
        stderr,
    );
}

/// #1813 triangulation — a VALID burst must still be accepted. Guards the
/// fail-closed change against a false positive: rejecting everything would
/// pass both tests above.
#[tokio::test]
async fn cli_rate_limit_burst_valid_value_is_accepted() {
    let (_output, assert) = run_single_page_scrape(&[], &["--rate-limit-burst", "7"]).await;
    assert.success();
}

/// #1813 triangulation — `WEBFANG_RATE_LIMIT_BURST` reaches the same
/// validator as argv, so a bad env var must fail closed identically rather
/// than than being a second, softer front door.
#[tokio::test]
async fn env_rate_limit_burst_non_numeric_fails_closed() {
    let (_output, assert) =
        run_single_page_scrape(&[("WEBFANG_RATE_LIMIT_BURST", "banana")], &[]).await;

    assert_exit_78(
        assert,
        "a non-numeric WEBFANG_RATE_LIMIT_BURST must exit 78 (ConfigError)",
    );
}

/// #1813 regression fix — the `auto` keyword must NOT be caught by the
/// fail-closed arm. `--rate-limit-burst auto` was a WORKING spelling on the
/// base, so rejecting it turns a pinned configuration into a hard exit-78 break
/// on upgrade. This is a fail-CLOSED regression, and it is the one #1813's own
/// tests had enshrined (the unit test listed `auto` among the rejected typos).
///
/// Both front doors are covered because both are real deployments: argv in a
/// shell script, env in a unit file or a container definition. Each must reach
/// the derived default, never exit 78.
#[tokio::test]
async fn cli_rate_limit_burst_auto_keyword_is_accepted_as_unset() {
    let (_output, assert) = run_single_page_scrape(&[], &["--rate-limit-burst", "auto"]).await;
    assert.success();
}

/// Same contract through the env front door: `WEBFANG_RATE_LIMIT_BURST=auto`
/// is a container-definition spelling, and the env arm attaches the SAME
/// validator as argv, so it must reach the derived default too.
#[tokio::test]
async fn env_rate_limit_burst_auto_keyword_is_accepted_as_unset() {
    let (_output, assert) =
        run_single_page_scrape(&[("WEBFANG_RATE_LIMIT_BURST", "auto")], &[]).await;
    assert.success();
}

/// Triangulation for the keyword arm: case-insensitivity is part of the
/// contract because the sibling `--concurrency` parser lowercases before
/// comparing (`domain/config.rs:391-393`). If the burst parser stopped
/// tolerating `AUTO`, a deployment that upper-cased its config would start
/// failing closed — the same regression in a narrower spelling.
#[tokio::test]
async fn cli_rate_limit_burst_auto_keyword_is_case_insensitive() {
    let (_output, assert) = run_single_page_scrape(&[], &["--rate-limit-burst", "AUTO"]).await;
    assert.success();
}

/// #897 item 2 — Zero Silent Loss, TOML path: a config-file-sourced
/// `rate_limit_burst = 0` must also hard-error with exit 78 (ConfigError),
/// never silently degrade to the derived default. Fails before any network
/// I/O, so no mock server is needed. (Preserved from the #925 landing.)
#[test]
fn toml_rate_limit_burst_zero_hard_errors() {
    let output = TempDir::new().expect("temp output dir");
    let (_conf, conf_path) = write_toml_config("rate_limit_burst = 0\n");

    let assert = cmd()
        .env("WEBFANG_CONFIG", &conf_path)
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

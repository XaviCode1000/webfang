//! Exit code integration tests
//!
//! Verifies that the CLI returns correct exit codes for:
//! - Empty sitemap discovery → exit 2 (EXIT_EMPTY_DISCOVERY)
//! - Network timeout → exit 69 (EXIT_UNAVAILABLE)
//! - Successful crawl → exit 0 (EXIT_SUCCESS)
//!
//! Run with: cargo nextest run --test-threads 2 exit_code_integration

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Resolve the path to the `webfang` binary.
///
/// `webfang` is built by `webfang_cli` (a workspace sibling), so
/// `assert_cmd::cargo_bin` cannot resolve it — `CARGO_BIN_EXE_webfang`
/// is only set for the owning crate.  This fallback searches
/// `target/{debug,release}` and builds the binary on demand.
fn webfang_path() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return PathBuf::from(p);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("resolve workspace root");
    let target_root = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("target"));
    for profile in ["debug", "release"] {
        let mut candidate = target_root.join(profile).join("webfang");
        if cfg!(windows) {
            candidate.set_extension("exe");
        }
        if candidate.exists() {
            return candidate;
        }
    }
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "webfang_cli", "--bin", "webfang", "--quiet"])
        .status()
        .expect("spawn cargo to build webfang");
    assert!(status.success(), "cargo build --bin webfang failed");
    let mut built = target_root.join("debug").join("webfang");
    if cfg!(windows) {
        built.set_extension("exe");
    }
    built
}

fn cmd() -> Command {
    let mut c = Command::new(webfang_path());
    // Hermeticity: remove all WEBFANG_* / WEBFANG_AI_MODEL_ID / AI_MODEL_ID
    // env vars so poisoned CI environments (bug-discovery workflow) don't
    // affect arg parsing. Legacy `AI_MODEL_ID` is still honored by the CLI
    // itself, via `webfang_core::cli::args::ai::LEGACY_AI_MODEL_ENV` — the
    // `webfang_ai` reader is gone since ADR-0004 paso 6 (#980).
    let poisoned: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("WEBFANG_") || k == "AI_MODEL_ID")
        .map(|(k, _)| k)
        .collect();
    for key in poisoned {
        c.env_remove(&key);
    }
    // SSRF entry-guard allowance (F-06 + F-32, #1217): mocks bind 127.0.0.1 —
    // a forbidden literal the production request path now rejects — so this
    // funnel disarms ONLY the entry layer for spawned binaries. Production
    // never sets this; `test_ssrf_literal_seeds_rejected_before_fetch` opts
    // back out with `.env_remove(...)`.
    c.env(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
    // Config gets the same treatment, for a stronger reason than the cache:
    // the CLI resolves its config file through `WEBFANG_CONFIG`
    // (`cli::config::load_config_defaults`), so pointing that at an EMPTY
    // file means a spawned binary can never read the developer's real
    // `~/.config/webfang/config.toml`. It MATTERS for the `--clean-ai` case
    // below: a config that declares an embedding provider would make the run
    // resolve — and PROBE — that endpoint instead of failing with the
    // "no provider" configuration error the test is about. An explicit
    // override must be absolute and present (#1659), so the file is created
    // for real; `ConfigDefaults` is `#[serde(default)]`, so empty TOML parses
    // to the same all-`None` defaults an absent file used to produce.
    c.env("WEBFANG_CONFIG", hermetic_empty_config_file());
    c
}

/// An EMPTY config file for one spawned-binary invocation.
///
/// Created eagerly (the explicit-override contract rejects a missing file,
/// #1659) and left for the OS temp cleanup to reclaim: the `Command` cannot
/// own the file's lifetime.
///
/// Thin wrapper over [`hermetic_config_file`] with an empty body: the
/// "no provider resolvable" run needs a file that DECLARES nothing, while
/// the `--offline` run needs one that DECLARES a provider — same contract
/// (absolute, present, parseable), different body.
fn hermetic_empty_config_file() -> std::path::PathBuf {
    hermetic_config_file("")
}

/// A config file with the given contents for one spawned-binary invocation.
///
/// The explicit-override contract rejects a missing file (#1659), so the file
/// is created for real. `ConfigDefaults` is `#[serde(default)]`, so empty
/// TOML parses to the same all-`None` defaults an absent file used to
/// produce — which is why the "no provider" run can point at an EMPTY file
/// and the `--offline` run at one that declares a provider.
fn hermetic_config_file(contents: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "webfang-exit-code-config-{}-{n}.toml",
        std::process::id()
    ));
    std::fs::write(&path, contents).expect("create hermetic config.toml");
    path
}

// ============================================================================
// Tests: Empty sitemap → exit 2
// ============================================================================

/// Empty sitemap (no <loc> entries) returns exit code 2.
#[tokio::test]
async fn test_empty_sitemap_returns_exit_2() {
    let mock_server = MockServer::start().await;

    // Serve an empty sitemap
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#,
        ))
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let sitemap_url = format!("{}/sitemap.xml", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("No URLs discovered"));
}

/// Site without any sitemap (auto-discovery finds nothing) returns exit 2,
/// not exit 69: "no sitemap" is a terminal discovery state, not an
/// infrastructure failure (#695, OBS-SITEMAP-001).
#[tokio::test]
async fn test_missing_sitemap_returns_exit_2() {
    let mock_server = MockServer::start().await;

    // No mocks mounted: robots.txt and every sitemap candidate 404.
    let base_url = format!("{}/", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--use-sitemap")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("No URLs discovered"));
}

// ============================================================================
// Tests: DOM discovery with only external links → exit 0 (seed injected)
// ============================================================================

/// DOM discovery that finds no internal URLs must still scrape the seed URL:
/// empty discovery is not fatal outside sitemap mode, so the run exits 0
/// instead of EmptyDiscovery (exit 2) (#488).
#[tokio::test]
async fn test_dom_external_only_links_scrapes_seed() {
    let mock_server = MockServer::start().await;

    // The seed page links only to an external domain, so DOM discovery yields
    // zero internal URLs.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><h1>Seed content</h1>\
             <p>The seed page carries plenty of substantive server-rendered text so it \
             comfortably clears the fifty character minimum content guard.</p>\
             <a href=\"https://iana.org\">external link</a></body></html>",
        ))
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let out_dir = tempfile::TempDir::new().expect("create temp output dir");

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--output")
        .arg(out_dir.path())
        .timeout(Duration::from_secs(30))
        .assert()
        .code(0);

    // The seed page itself must have been fetched: DOM discovery reads it once
    // for link extraction and the scraper reads it again for content.
    let requests = mock_server.received_requests().await.unwrap();
    let seed_requests = requests.iter().filter(|r| r.url.path() == "/").count();
    assert!(
        seed_requests >= 1,
        "expected the seed page / to be fetched, got {seed_requests} requests"
    );

    // The seed page was saved as markdown in the output dir and contains the
    // identifiable seed content.
    let md_files: Vec<PathBuf> = walkdir::WalkDir::new(out_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| e.path().to_path_buf())
        .collect();
    assert_eq!(
        md_files.len(),
        1,
        "expected 1 .md file in output, got {}",
        md_files.len()
    );
    let content = std::fs::read_to_string(&md_files[0]).expect("read .md file");
    assert!(
        content.contains("Seed content"),
        "expected seed content in .md file"
    );
}

// ============================================================================
// Tests: Network timeout → exit 69
// ============================================================================

/// Timeout during sitemap fetch returns exit code 69.
#[tokio::test]
async fn test_timeout_returns_exit_69() {
    let mock_server = MockServer::start().await;

    // Serve a response with a very long delay to trigger timeout
    Mock::given(method("GET"))
        .and(path("/slow-sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://PLACEHOLDER/page1</loc></url>
</urlset>"#,
                )
                .set_delay(Duration::from_secs(120)),
        )
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let sitemap_url = format!("{}/slow-sitemap.xml", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .arg("--timeout-secs")
        .arg("1")
        .timeout(Duration::from_secs(60))
        .assert()
        .code(69)
        .stderr(predicate::str::contains("URL discovery failed"));
}

// ============================================================================
// Tests: Successful discovery → exit 0
// ============================================================================

/// Valid sitemap with URLs returns exit code 0 (no regression).
#[tokio::test]
async fn test_valid_sitemap_returns_exit_0() {
    let mock_server = MockServer::start().await;
    let server_uri = mock_server.uri();

    // Serve a valid sitemap with one URL
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>{server_uri}/page1</loc></url>
</urlset>"#
        )))
        .mount(&mock_server)
        .await;

    // Serve the page content
    Mock::given(method("GET"))
        .and(path("/page1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><h1>Hello World</h1><p>Test content long enough to clear the \
                 fifty character minimum content guard comfortably.</p></body></html>",
        ))
        .mount(&mock_server)
        .await;

    let base_url = format!("{server_uri}/");
    let sitemap_url = format!("{server_uri}/sitemap.xml");

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(0);
}

// ============================================================================
// Tests: --clean-ai without a resolvable embedding provider (#761, ADR-0004)
// ============================================================================

/// `--clean-ai` WITHOUT a resolvable remote embedding provider must fail
/// with exit 78 BEFORE any network request — previously the CLI scraped the
/// whole page and only failed at export time (#761).
///
/// ADR-0004 paso 6 changed the CAUSE, not the shape. The `ai` Cargo feature
/// is gone, so "this build has no cleaner" is no longer a question about the
/// binary: `--clean-ai` is un-gated, and what it needs is an embedding
/// provider (`--embedding-provider` / `WEBFANG_EMBEDDING_PROVIDER`). With
/// none resolvable, `build_ai_cleaner` answers `CliExit::ConfigError` —
/// exit 78 — and names the knob that fixes it.
///
/// The run is hermetic: `cmd()` points `WEBFANG_CONFIG` at an EMPTY config
/// file, so no provider can be resolved from the developer's machine and no
/// endpoint is ever probed.
#[tokio::test]
async fn test_clean_ai_without_embedding_provider_fails_before_fetch() {
    let mock_server = MockServer::start().await;

    // If the CLI fetches anything, this mock would record it — the assertion
    // below proves the preflight fired before any request left the process.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                "<html><body><p>content that must never be fetched</p></body></html>",
            ),
        )
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--clean-ai")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(78)
        .stderr(predicate::str::contains("--clean-ai"))
        .stderr(predicate::str::contains("--embedding-provider"));

    let requests = mock_server.received_requests().await.unwrap();
    assert!(
        requests.is_empty(),
        "preflight must fail before any network request, got {} requests",
        requests.len()
    );
}

// ============================================================================
// Tests: --clean-ai --offline with a CONFIGURED provider (ADR-0004 S10)
// ============================================================================

/// Stable head of the offline guard's user-facing message.
///
/// Source: the `offline` arm of `build_embedding_provider`
/// (`crates/webfang_core/src/cli/llm_wire.rs:141-147`), rendered by
/// `CliExit::ConfigError` as `Error: {msg}` (`cli/error.rs:426-429`).
/// Only the stable head of the sentence is asserted: the tail names the
/// remedy and is free to change with the wording.
const OFFLINE_REMOTE_FRAGMENT: &str = "modo offline con provider remoto de embeddings";

/// RFC 2606 reserved hostname: guaranteed never to resolve, so the
/// configured endpoint is genuinely un-dialable and the test never touches a
/// real network.
const REMOTE_PROVIDER_HOST: &str = "embeddings.invalid";

/// A config file declaring ONE remote (public) embedding provider.
///
/// The provider is declared, not selected: `--embedding-provider` and
/// `WEBFANG_EMBEDDING_PROVIDER` are both absent (the harness strips every
/// `WEBFANG_*` variable), so the run resolves the CONFIG FILE's default
/// provider — the first one declaring the `embedding` capability
/// (`ProviderRegistry::resolve_default_embedding`,
/// `crates/webfang_core/src/domain/providers.rs:322`).
fn remote_provider_config() -> String {
    format!(
        r#"
[[providers]]
id = "remote-embeddings"
display_name = "Remote embeddings (public)"
kind = "open_ai_compatible"
base_url = "https://{REMOTE_PROVIDER_HOST}/v1"
auth = {{ source = "none" }}
capabilities = ["embedding"]
model = "text-embedding-test"
"#
    )
}

/// `--clean-ai --offline` with a CONFIGURED REMOTE embedding provider must
/// exit 78 BEFORE any request leaves the process (ADR-0004 S10).
///
/// S10 — "`--offline` + remoto: exit 78 (guard de regresión, no criterio
/// nuevo; `cli/error.rs:36`, `llm_wire.rs:141-147`)" (ADR:116).
///
/// The guard lives in the provider RESOLUTION path, so it only fires once a
/// provider IS resolvable — which is exactly why this test needs a
/// CONFIGURED provider. The other arm (no provider at all → the "no
/// provider" message) is covered by
/// `test_clean_ai_without_embedding_provider_fails_before_fetch`.
///
/// The endpoint is `embeddings.invalid` (RFC 2606 reserved): it can never
/// resolve, so the run is deterministic and the endpoint is un-dialable. If
/// the code ever dialed BEFORE the offline check, the run would fail with a
/// transport/DNS error naming the endpoint instead of the offline message —
/// the negative assertion below is written to catch exactly that.
#[tokio::test]
async fn test_clean_ai_offline_with_remote_embedding_provider_exits_78() {
    let mock_server = MockServer::start().await;

    // Method-agnostic catch-all: if the CLI reaches the network AT ALL, this
    // mock records it — a GET on the seed URL, a POST on the provider's
    // `/v1/embeddings`, anything. The assertion below proves the preflight
    // fired before any request left the process.
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                "<html><body><p>content that must never be fetched</p></body></html>",
            ),
        )
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());

    let outcome = cmd()
        .env(
            "WEBFANG_CONFIG",
            hermetic_config_file(&remote_provider_config()),
        )
        .arg("--url")
        .arg(&base_url)
        .arg("--clean-ai")
        .arg("--offline")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(78)
        .stderr(predicate::str::contains(OFFLINE_REMOTE_FRAGMENT));

    // The guard runs BEFORE the adapter is built, so the configured endpoint
    // is never contacted: a dial names the endpoint in the error, the guard
    // does not.
    let stderr = String::from_utf8_lossy(&outcome.get_output().stderr).into_owned();
    assert!(
        !stderr.contains(REMOTE_PROVIDER_HOST),
        "the offline guard must fire before the endpoint is contacted, got: {stderr}"
    );

    let requests = mock_server.received_requests().await.unwrap();
    assert!(
        requests.is_empty(),
        "preflight must fail before any network request, got {} requests",
        requests.len()
    );
}

/// `--clean-ai --offline` with a CONFIGURED LOOPBACK embedding provider is
/// ALSO rejected today — a DISCREPANCY with the documented semantics, pinned
/// here on purpose.
///
/// The notice the CLI prints on every run states the rule as "loopback
/// permitido, remoto público -> exit 78"
/// (`crates/webfang_core/src/cli/args/ai.rs:263`), and the ADR repeats it.
/// The implementation does not implement that rule: the `offline` arm of
/// `build_embedding_provider`
/// (`crates/webfang_core/src/cli/llm_wire.rs:141-147`) rejects EVERY
/// resolved `open_ai_compatible` provider, loopback included, because the
/// guard never looks at the endpoint's host. The guard was introduced
/// unconditional in #1518, so this is a prose/implementation mismatch, not a
/// paso-6 regression.
///
/// This test therefore pins the CURRENT behavior and names the criterion it
/// does not yet satisfy. When the guard learns to exempt loopback, THIS test
/// is the one that must change — that is the reason it exists.
///
/// The endpoint is the mock server itself, so the zero-request assertion
/// below is a REAL proof that the guard fired before the dial: without it,
/// the adapter would build and POST to `/v1/embeddings`.
#[tokio::test]
async fn test_clean_ai_offline_with_loopback_embedding_provider_is_rejected_today() {
    let mock_server = MockServer::start().await;

    // Method-agnostic catch-all: the provider endpoint IS this server, so a
    // dial would land here as a POST on `/v1/embeddings`.
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                "<html><body><p>content that must never be fetched</p></body></html>",
            ),
        )
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let provider_config = format!(
        r#"
[[providers]]
id = "loopback-embeddings"
display_name = "Loopback embeddings"
kind = "open_ai_compatible"
base_url = "{}/v1"
auth = {{ source = "none" }}
capabilities = ["embedding"]
model = "text-embedding-test"
allow_loopback = true
"#,
        mock_server.uri()
    );

    cmd()
        .env("WEBFANG_CONFIG", hermetic_config_file(&provider_config))
        .arg("--url")
        .arg(&base_url)
        .arg("--clean-ai")
        .arg("--offline")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(78)
        .stderr(predicate::str::contains(OFFLINE_REMOTE_FRAGMENT));

    let requests = mock_server.received_requests().await.unwrap();
    assert!(
        requests.is_empty(),
        "the offline guard must fire before the endpoint is dialed, got {} requests",
        requests.len()
    );
}

// ============================================================================
// Tests: SSRF literal-IP entry guard — F-06 + F-32 (#1217)
// ============================================================================

/// IP-literal seeds in ANY encoding must be rejected BEFORE any socket opens.
///
/// `127.0.0.1`, `169.254.169.254`, hex `0x7f000001`, decimal `2130706433`
/// and abbreviated `127.1` all address loopback / link-local (cloud metadata).
/// The CLI used to dial every one of them: entry validation lived in the MCP
/// crate only, the connect-time resolver never sees IP literals (wreq
/// short-circuits them), and the redirect guard fires on redirects only.
/// Mirrors `test_clean_ai_without_feature_fails_before_fetch`: the mock
/// proves zero outbound requests.
#[tokio::test]
async fn test_ssrf_literal_seeds_rejected_before_fetch() {
    let mock_server = MockServer::start().await;

    // If the CLI fetches anything, this mock would record it — the assertion
    // below proves the entry guard fired before any request left the process.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><h1>Seed content</h1>\
             <p>The seed page carries plenty of substantive server-rendered text so it \
             comfortably clears the fifty character minimum content guard.</p>\
             <a href=\"https://iana.org\">external link</a></body></html>",
        ))
        .mount(&mock_server)
        .await;

    // wiremock binds 127.0.0.1 — address the same listener in every encoding
    // the URL parser accepts (dotted, hex, decimal, abbreviated).
    let port = mock_server
        .uri()
        .rsplit(':')
        .next()
        .expect("mock uri carries a port")
        .trim_end_matches('/')
        .to_owned();
    let literal_urls = [
        format!("http://127.0.0.1:{port}/"),
        format!("http://169.254.169.254:{port}/"),
        format!("http://0x7f000001:{port}/"),
        format!("http://2130706433:{port}/"),
        format!("http://127.1:{port}/"),
    ];

    for url in literal_urls {
        let out_dir = tempfile::TempDir::new().expect("create temp output dir");
        // Opt out of the harness entry-guard allowance: production never sets
        // it, so the spawned binary must enforce like production.
        cmd()
            .env_remove(webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV)
            .arg("--url")
            .arg(&url)
            .arg("--single-page")
            .arg("--output")
            .arg(out_dir.path())
            .arg("--timeout-secs")
            .arg("3")
            .arg("--max-retries")
            .arg("0")
            .timeout(Duration::from_secs(60))
            .assert()
            .failure()
            .stderr(predicate::str::contains("SSRF"));
    }

    let requests = mock_server.received_requests().await.unwrap();
    assert!(
        requests.is_empty(),
        "entry guard must reject literal seeds before any socket opens, got {} requests",
        requests.len()
    );
}

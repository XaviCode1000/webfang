//! Behavioral test: `--js-strategy static` exercises the FetchRouter path.
//!
//! Verifies that the CLI scrape flow wires the JsStrategy into the
//! FetchRouter and produces correct output against a wiremock server (#303).

use crate::cmd;
use crate::BehavioralTest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const SEED_HTML: &str = r#"
<html><head><title>JS Strategy Test</title></head>
<body><main><article>
<h1>Static Strategy Page</h1>
<p>This content is served via the static wreq downloader path, confirming
that the FetchRouter wiring dispatches correctly for --js-strategy static.</p>
<p>A second paragraph gives readability enough signal to extract the
document body as the primary content region of this test page.</p>
</article></main></body></html>
"#;

#[tokio::test]
async fn js_strategy_static_scrapes_successfully() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SEED_HTML)
                .insert_header("Content-Type", "text/html"),
        )
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--quiet")
        .arg("--js-strategy")
        .arg("static")
        .arg("--timeout-secs")
        .arg("5")
        .assert()
        .success();

    let content = t.read_md_content();
    crate::assert_snapshot_redacted(
        "js_strategy_static_scrapes_successfully",
        t.out.path(),
        content,
    );
}

/// Invalid `--js-strategy` values must be rejected at arg-parse time (value
/// enum), not silently defaulted — a typo like `bogus` should fail fast with a
/// non-zero exit and a message naming the valid values (#542 coverage
/// extension). Pure CLI parse test, no network.
#[test]
fn js_strategy_invalid_value_is_rejected() {
    let output = cmd()
        .arg("--url")
        .arg("https://example.com")
        .arg("--js-strategy")
        .arg("bogus")
        .output()
        .expect("run binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit for invalid --js-strategy value"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("js-strategy"),
        "stderr should name the offending flag (--js-strategy): {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Chromium (`--js-strategy full`) honest-status regression (#1311)
// ---------------------------------------------------------------------------

/// Best-effort Chrome presence probe so the E2E skips (rather than fails) on
/// machines without a browser. Mirrors the downloader-layer probe in
/// `chromiumoxide_downloader.rs`; the production gate owns real resolution
/// (preflight + #1278).
#[cfg(feature = "chromium")]
fn chrome_present_for_e2e() -> bool {
    [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ]
    .iter()
    .any(|binary| {
        std::process::Command::new(binary)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// Build a `chromium`-enabled `webfang` binary and return a PRIVATE copy of it.
///
/// The shared harness (`webfang_path()`) derives the binary's feature set from
/// the test crate without `chromium`, so `--js-strategy full` in a harness-built
/// binary would hit the feature stub instead of the real CDP downloader. This
/// helper builds the binary with the feature active (same on-demand `cargo
/// build` pattern the harness itself uses) and copies it out of the shared
/// `target/` location: sibling tests rebuild the shared binary concurrently,
/// and a pinned path into `target/debug/` could race a mid-run replacement.
#[cfg(feature = "chromium")]
fn chromium_webfang_bin() -> std::path::PathBuf {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let status = std::process::Command::new(cargo)
        .args([
            "build",
            "-p",
            "webfang_cli",
            "--bin",
            "webfang",
            "--features",
            "chromium",
            "--quiet",
        ])
        .status()
        .expect("spawn cargo to build the chromium-enabled webfang binary");
    assert!(
        status.success(),
        "cargo build of the chromium-enabled webfang binary failed"
    );
    let built = match std::env::var("CARGO_TARGET_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir).join("debug").join("webfang"),
        Err(_) => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("resolve workspace root")
            .join("target")
            .join("debug")
            .join("webfang"),
    };
    assert!(
        built.exists(),
        "chromium-enabled binary missing at {}",
        built.display()
    );
    let private_dir = std::env::temp_dir().join(format!("webfang-test-bin-{}", std::process::id()));
    std::fs::create_dir_all(&private_dir).expect("create private bin dir");
    let private = private_dir.join("webfang");
    std::fs::copy(&built, &private).expect("copy chromium-enabled binary to a private path");
    private
}

/// `assert_cmd::Command` over the chromium-enabled binary with the same env
/// sanitization the shared harness applies in `sanitize_env` (which is private
/// to `cli_harness.rs`): strip ambient `WEBFANG_*`/`AI_MODEL_ID`, disarm the
/// SSRF entry guard for loopback wiremock (F-06 + F-32, #1217), and give the
/// spawned binary its own hermetic cache base.
#[cfg(feature = "chromium")]
fn chromium_cmd(
    bin: &std::path::Path,
    t: &BehavioralTest,
    cache: &std::path::Path,
) -> assert_cmd::Command {
    let mut cmd = assert_cmd::Command::new(bin);
    let poisoned: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("WEBFANG_") || k == "AI_MODEL_ID")
        .map(|(k, _)| k)
        .collect();
    for key in poisoned {
        cmd.env_remove(&key);
    }
    cmd.env(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
    cmd.env("XDG_CACHE_HOME", cache);
    // NOTE: no --url here — the caller supplies the per-case URL (a duplicate
    // --url is rejected with exit 64 and would mask the real assertion).
    cmd.arg("--output").arg(t.out.path());
    cmd
}

/// #1311 regression: the Chromium production path must carry the REAL
/// navigation HTTP status into `FetchedPage.status` so the existing
/// guard-chain classification (`scrape_single_url_inner`) rejects non-2xx
/// exactly like the wreq path. It previously hardcoded `status: 200`, so a
/// browser-fetched 404/500 was silently accepted and exported as a success
/// record — a lie about the page.
///
/// E2E over loopback wiremock: a 404 and a 500 must each surface as an honest
/// error (`EXIT_UNAVAILABLE`, 69) with zero success records persisted.
/// Skips gracefully where no Chrome binary exists (bare CI), mirroring the
/// downloader-layer settle E2E.
#[cfg(feature = "chromium")]
#[tokio::test]
async fn non2xx_browser_page_is_honest_error_exit_69() {
    if !chrome_present_for_e2e() {
        eprintln!("skipping non2xx browser E2E: no Chrome binary on PATH");
        return;
    }
    let bin = chromium_webfang_bin();
    let t = BehavioralTest::new().await;
    let cache = tempfile::TempDir::new().expect("hermetic cache dir");

    // Rich bodies so extraction would succeed — the point is that the fetch
    // status (404/500), not the content quality, must fail the run.
    let body_for = |title: &str| {
        format!(
            r#"<html><head><title>{title}</title></head><body><main><article>\
<h1>{title}</h1><p>This page answers with an error status, but its body is \
real content so the only honest outcome is to reject the fetch on status.</p>\
<p>A second paragraph gives readability enough signal to extract a document \
body if — wrongly — the non-2xx status were reported as a 200 success.</p>\
</article></main></body></html>"#
        )
    };
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/missing"))
        .respond_with(
            wiremock::ResponseTemplate::new(404)
                .set_body_string(body_for("Missing"))
                .insert_header("Content-Type", "text/html"),
        )
        .mount(&t.server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/boom"))
        .respond_with(
            wiremock::ResponseTemplate::new(500)
                .set_body_string(body_for("Boom"))
                .insert_header("Content-Type", "text/html"),
        )
        .mount(&t.server)
        .await;

    for (path, status) in [("/missing", 404), ("/boom", 500)] {
        let output = chromium_cmd(&bin, &t, cache.path())
            .arg("--url")
            .arg(format!("{}{path}", t.server.uri()))
            .arg("--single-page")
            .arg("--js-strategy")
            .arg("full")
            .arg("--max-retries")
            .arg("0")
            .arg("--timeout-secs")
            .arg("60")
            .arg("--quiet")
            .output()
            .expect("run chromium-enabled webfang");

        assert_eq!(
            output.status.code(),
            Some(69),
            "HTTP {status} fetched through the browser must exit 69 \
             (EXIT_UNAVAILABLE), got {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            t.find_files("md").is_empty(),
            "HTTP {status} through the browser must not produce a success record"
        );
    }
}

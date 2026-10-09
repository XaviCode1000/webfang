//! Batch export failure must surface a non-zero exit code (#1820).
//!
//! `export_phase` returns a `CliExit`, but `run_batch` called it as a bare
//! statement and dropped the value: the final exit came only from
//! `batch_exit_code`, which aggregates crawl + extraction outcomes. A batch
//! whose export failed (full disk, unwritable RAG export dir, broken export
//! sink) therefore exited 0 while its export wrote nothing.
//!
//! This test pins the single-run precedence (orchestrator.rs): the
//! crawl/extraction aggregate wins over the export exit, a failed export wins
//! over `Success`, and cancellation still beats both.
//!
//! The second pin (#1949) holds the elastic-ingestion exit contract: a
//! TRANSIENT ingestion failure must leave through the canonical classify →
//! exit machinery with 69 (EX_UNAVAILABLE, matrix row 26), never the hardcoded
//! 74 the batch call site used to emit for every ingestion error class.
//!
//! Fixture: `--quick-save --vault <dir>` keeps the capture spool and the
//! `_inbox` markdown writes healthy (both root at the vault via
//! `resolve_persistence_root`), while `-o` points at a plain FILE. Under
//! `--quick-save` the RAG export dir is exactly `-o` (`resolve_export_dir`),
//! so `export_factory::process_results`'s `create_dir_all` fails
//! deterministically — the path exists as a regular file, and `create_dir_all`
//! refuses to turn it into a directory on every OS (EEXIST here) — a
//! no-chmod, no-permission export failure using the same "plain file where a
//! directory is required" pattern the vector exporter's blocked-dir test uses.

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use common::cli_harness::{cmd, redact_nondeterministic, BehavioralTest};
use insta::assert_snapshot;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// `CliExit::IoError` maps to exit 74 (EX_IOERR) — `cli/error.rs`
/// `EXIT_IO_ERROR`, applied by the `std::process::Termination` impl that
/// `webfang_cli`'s `async fn main() -> CliExit` resolves through.
const EXPECTED_EXIT_CODE: i32 = 74;

#[tokio::test]
async fn batch_export_failure_exits_io_error() {
    let t = BehavioralTest::new().await;

    // One healthy page: crawl and extraction must succeed so the ONLY failing
    // phase is the export.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch Export Exit</h1>\
                 <p>This body carries enough substantive text to pass the minimum-content guard.</p>\
                 </article></body></html>",
        ))
        .mount(&t.server)
        .await;

    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", t.server.uri())).expect("write batch file");

    // Writable vault: with --quick-save it receives the spool file and the
    // _inbox markdown, so every pre-export phase stays healthy.
    let vault = t.out.path().join("vault");
    std::fs::create_dir(&vault).expect("create vault dir");

    // The RAG export dir is `-o` under --quick-save (`resolve_export_dir`):
    // a plain FILE makes the export's create_dir_all fail with ENOTDIR.
    let blocked = t.out.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("create blocked file");

    let output = cmd()
        .arg("--batch-file")
        .arg(&batch_file)
        .arg("--quick-save")
        .arg("--vault")
        .arg(&vault)
        .arg("--output")
        .arg(&blocked)
        .timeout(Duration::from_secs(60))
        .output()
        .expect("spawn webfang binary");

    let code = output
        .status
        .code()
        .expect("webfang must not die by signal");

    assert_eq!(
        code,
        EXPECTED_EXIT_CODE,
        "a batch whose export failed must exit {EXPECTED_EXIT_CODE} (CliExit::IoError), \
         not {code}; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let redacted = redact_nondeterministic(t.out.path(), &stderr);
    let mut settings = insta::Settings::clone_current();
    // Collapse the OS-specific errno tail: "File exists (os error 17)" on
    // Linux reads differently on Windows, and the load-bearing part of the
    // message is the prefix naming the failed operation. The filter stops at
    // a quote or end of line so surrounding syntax — the `IoError("...")`
    // debug wrapper in the tracing ERROR line — survives redaction intact.
    settings.add_filter(
        r#"failed to create output directory: [^"\n]*"#,
        "failed to create output directory: <IO_ERROR>",
    );
    // Hermetic harness paths (per-invocation, PID-keyed) are outside the
    // <OUT_DIR> redaction scope; no log should print them, but the filter
    // keeps the snapshot honest if one ever does.
    settings.add_filter(r"webfang-test-cache-\d+-\d+", "<CACHE_DIR>");
    settings.add_filter(r"webfang-test-empty-config-\d+-\d+\.toml", "<CONFIG_FILE>");
    settings.bind(|| {
        assert_snapshot!("batch_export_failure_stderr", redacted);
    });
}

/// `CliExit::NetworkError` maps to exit 69 (EX_UNAVAILABLE) — `cli/error.rs`
/// `EXIT_UNAVAILABLE`, the class default for `TransientRetriable` /
/// `TransientBackoff` (matrix row 26).
#[cfg(feature = "persistence")]
const EXPECTED_TRANSIENT_EXIT_CODE: i32 = 69;

/// #1949 — a TRANSIENT elastic-ingestion failure must exit 69
/// (EX_UNAVAILABLE, matrix row 26), not the hardcoded 74 the batch call site
/// used to emit for every ingestion error class.
///
/// The `--elastic` sink is `persistence`-gated (`preflight::check_elastic_sink`
/// exits 78 without the feature), so the pin is gated the same way: a
/// no-features run compiles it to nothing, exactly like `sqlite_integration.rs`.
///
/// Fixture — the ingestion pipeline RE-DOWNLOADS every scraped URL, so ONE
/// mock server can serve the scrape and then fail the re-download
/// deterministically:
///
/// 1. a one-shot 200 mock (`.up_to_n_times(1)` — caps matching, so the
///    ingestion's re-download cannot hit it) answers the batch scrape — the
///    only `GET /` request the crawl makes (#1229 single-fetch, no assets, no
///    sitemap in batch mode);
/// 2. the ingestion's re-download finds the one-shot exhausted — wiremock's
///    `MountedMock::matches` returns `false` at `max_n_matches` — and falls
///    through to the second mock, a 301 redirecting to `dead.invalid`;
/// 3. `.invalid` is reserved by RFC 6761, so the name never resolves on any
///    OS/CI runner: the dial fails at DNS and the transport failure comes
///    back as `ScraperError::Network`, which classifies `TransientRetriable`
///    — the canonical class default is exactly the matrix row 26 contract:
///    "backend unavailable, retry" → 69.
///
/// Determinism: request #1 goes to the first-registered mock (stable priority
/// sort), request #2 to the second; no timers, no ambient state beyond the
/// hermetic env the harness already installs.
#[cfg(feature = "persistence")]
#[tokio::test]
async fn batch_transient_ingestion_failure_exits_unavailable() {
    let t = BehavioralTest::new().await;

    // Request #1 — the scrape. `up_to_n_times(1)` CAPS matching (wiremock's
    // `.expect` only verifies at server drop, it does not stop matching),
    // which is what routes the ingestion's re-download to the 301; the
    // `.expect(1)` keeps a drop-time proof that exactly one request hit it.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch Ingestion Exit</h1>\
                 <p>This body carries enough substantive text to pass the minimum-content guard.</p>\
                 </article></body></html>",
        ))
        .up_to_n_times(1)
        .expect(1)
        .mount(&t.server)
        .await;

    // Request #2 — the ingestion re-download, redirected to a host that
    // cannot resolve. Not a literal IP: the SSRF redirect policy would
    // `stop()` on a forbidden literal and hand the ingestion the 3xx as a
    // successful (empty) download instead of failing.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", "http://dead.invalid/"))
        .mount(&t.server)
        .await;

    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", t.server.uri())).expect("write batch file");

    let db_path = t.out.path().join("elastic.db");

    let output = cmd()
        .arg("--batch-file")
        .arg(&batch_file)
        .arg("--elastic")
        .arg("--db-path")
        .arg(&db_path)
        .arg("--output")
        .arg(t.out.path())
        .timeout(Duration::from_secs(60))
        .output()
        .expect("spawn webfang binary");

    let code = output
        .status
        .code()
        .expect("webfang must not die by signal");

    assert_eq!(
        code,
        EXPECTED_TRANSIENT_EXIT_CODE,
        "a transient ingestion failure must exit \
         {EXPECTED_TRANSIENT_EXIT_CODE} (EX_UNAVAILABLE, matrix row 26), not {code}; \
         stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // The user-facing message must stay in Spanish and name the phase that
    // failed — the operator reads this line to decide whether to retry.
    assert!(
        stderr.contains("Falló la ingesta de vectores"),
        "the user-facing message must name the failed ingestion in Spanish: {stderr}"
    );

    let redacted = redact_nondeterministic(t.out.path(), &stderr);
    let mut settings = insta::Settings::clone_current();
    // The transport-failure tail is resolver/OS-dependent ("Name or service
    // not known" on glibc, "No such host is known." on Windows) — collapse it
    // after the class-identifying `error de red:` prefix so the snapshot stays
    // portable across runners.
    settings.add_filter(r"error de red: .*", "error de red: <TRANSPORT_FAILURE>");
    // Same tail inside the pipeline's fail-fast WARN line (`error=` field),
    // plus wreq's own error-kind rendering ("client error (Connect)" vs a
    // resolver-kind name on another stack): the KIND is incidental to the
    // pin, which is the Spanish message + the network class.
    settings.add_filter(r"client error \([A-Za-z]+\)", "client error (<KIND>)");
    settings.add_filter(
        r"error=(?:[^\n]*dead\.invalid[^\n]*)",
        "error=<TRANSPORT_FAILURE>",
    );
    settings.add_filter(r"webfang-test-cache-\d+-\d+", "<CACHE_DIR>");
    settings.add_filter(r"webfang-test-empty-config-\d+-\d+\.toml", "<CONFIG_FILE>");
    settings.bind(|| {
        assert_snapshot!("batch_transient_ingestion_failure_stderr", redacted);
    });
}

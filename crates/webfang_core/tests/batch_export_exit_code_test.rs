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

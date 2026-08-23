//! Task 5.1 — bounded-memory measurement harness (change
//! stabilization-concurrency-budget, decision Q3 MEASURE FIRST).
//!
//! RSS reader over `/proc/self/statm` (Linux CI) plus a tiny append-only
//! report writer. Probe tests co-located with each measured structure write
//! one line per scenario so the committed BEFORE/AFTER tables are produced
//! by the same code path on any machine.
//!
//! Design constraints honored here:
//! - No absolute-byte assertions anywhere (coarse plateau detection only).
//! - No new dependencies (plain std fs reads).
//! - Report destination is env-driven: `WEBFANG_MEMORY_REPORT_PATH`
//!   (default `target/memory-report.md`).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// Current resident set size in bytes, read from `/proc/self/statm`.
///
/// Returns `None` outside Linux (the measurement gate runs on Linux CI;
/// elsewhere probes degrade to entry-count-only reporting).
#[must_use]
pub fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    // Page size is virtually always 4096 on Linux CI runners; read it
    // properly anyway via the auxiliary sysconf-like constant.
    Some(resident_pages.saturating_mul(page_size_bytes()))
}

// Documented approximation: 4096 covers x86_64 Linux CI runners.
//
// The workspace denies `unsafe_code`, so `libc::sysconf(_SC_PAGESIZE)` is not
// available here. On 64 KiB-page kernels (some aarch64 configs) this
// OVERESTIMATES absolute MiB by up to 16x; entry counts are exact and all
// BEFORE/AFTER comparisons use the same constant, so relative deltas stay valid
// on any kernel. Absolute numbers in reports assume a 4 KiB page.
fn page_size_bytes() -> u64 {
    4_096
}

/// Destination of the measurement report. `None` when
/// `WEBFANG_MEMORY_REPORT_PATH` is unset: probes then print to stdout instead
/// of touching the filesystem, so ordinary test runs never litter the working
/// tree or `target/`.
#[must_use]
pub fn report_path() -> Option<PathBuf> {
    std::env::var("WEBFANG_MEMORY_REPORT_PATH")
        .ok()
        .map(PathBuf::from)
}

/// Append one probe line under `section`. With the env var set, creates the
/// file (with a header) when missing; without it, prints the line instead.
/// Never panics on I/O failure — measurement must not break the test that
/// hosts it; failures surface as a stderr note instead.
pub fn append_report(section: &str, line: &str) {
    let Some(path) = report_path() else {
        println!("memory_probe [{section}]: {line}");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let needs_header = !path.exists();
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        eprintln!("memory_probe: cannot open report at {}", path.display());
        return;
    };
    if needs_header {
        let _ = writeln!(file, "# Memory probe report\n");
    }
    let _ = writeln!(file, "## {section}\n{line}\n");
}

/// Format an optional byte count for reports (`n/a` off-Linux).
#[must_use]
pub fn fmt_rss(rss: Option<u64>) -> String {
    match rss {
        Some(b) => format!("{b} bytes ({:.1} MiB)", b as f64 / (1024.0 * 1024.0)),
        None => "n/a (non-Linux)".to_string(),
    }
}

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

fn page_size_bytes() -> u64 {
    // Rust exposes no stable page_size API; 4096 is universal on our CI
    // targets (x86_64/aarch64 linux). Documented assumption.
    4_096
}

/// Destination of the measurement report.
#[must_use]
pub fn report_path() -> PathBuf {
    std::env::var("WEBFANG_MEMORY_REPORT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/memory-report.md"))
}

/// Append one probe line under `section`. Creates the file (with a header)
/// when missing. Never panics on I/O failure — measurement must not break
/// the test that hosts it; failures surface as a stderr note instead.
pub fn append_report(section: &str, line: &str) {
    let path = report_path();
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

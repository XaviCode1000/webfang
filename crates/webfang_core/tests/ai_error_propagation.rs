//! AI error propagation behavioral tests (#339).
//!
//! Verifies that when `--clean-ai` cannot be honored, the CLI surfaces the
//! REAL cause with the correct exit code, instead of a generic
//! "recompile with the feature" message.
//!
//! # What changed in ADR-0004 paso 6
//!
//! This file used to pin TWO ONNX-era error types, both of which are gone
//! with the local model stack:
//!
//! - `SemanticError::OfflineMode` — the hf_hub cache miss behind
//!   `--clean-ai --offline` against an empty `HF_HOME`;
//! - the model-DOWNLOAD failure, surfaced as `NetworkError` (exit 69) when
//!   the proxy pointed at a dead address.
//!
//! Neither has a surviving behavior: there is no hf_hub cache, no model
//! download, and no `ai` Cargo feature to compile. So the download test was
//! DELETED — it exercised a code path that no longer exists, and keeping it
//! would have pinned an exit code (69) that the current contract cannot
//! produce.
//!
//! What SURVIVES — and is what this file now pins — is the shape of the
//! failure: `--clean-ai` without a resolvable remote embedding provider
//! answers `CliExit::ConfigError` (exit 78) with a Spanish message naming
//! `--embedding-provider`, and the ERROR never tells the operator to rebuild
//! with a Cargo feature that no longer exists. (The paso-5 ANNOUNCEMENT still
//! mentions the removed feature on purpose — that is the whole point of an
//! announcement — so the check targets the error line, not the whole stderr.)
//!
//! The vector is deterministic and network-free: the harness installs an EMPTY
//! `WEBFANG_CONFIG` (so no provider can be resolved) and the run stops at
//! the configuration gate, before any request leaves the process.
//!
//! Run with: `cargo nextest run -p webfang_core --test ai_error_propagation`

#[path = "common/cli_harness.rs"]
mod common;
use common::{redact_nondeterministic, BehavioralTest};

use std::time::Duration;

/// Wall-clock budget for the spawned binary. The run stops at the
/// configuration gate, so this is a hang guard, not a latency budget.
const RUN_TIMEOUT_SECS: u64 = 60;

/// `--clean-ai` with no resolvable embedding provider must fail fast with the
/// real cause (exit 78, `ConfigError`), never a generic "recompile with a
/// feature" message.
///
/// The message must NAME the knob that fixes it (`--embedding-provider`): an
/// operator who cannot act on the error is an operator who cannot use the
/// flag, and the whole point of the de-gating is that `--clean-ai` still has
/// a working path.
#[tokio::test]
async fn clean_ai_without_embedding_provider_surfaces_config_error() {
    let t = BehavioralTest::new().await;
    common::mount_single_page_site(&t).await;

    let output = t
        .scraper_cmd()
        .arg("--clean-ai")
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("run webfang binary");

    assert_eq!(
        output.status.code(),
        Some(78),
        "expected ConfigError exit code 78, got {:?}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("--embedding-provider"),
        "stderr must name the knob that resolves the cleaner; stderr:\n{stderr}"
    );
    // The CONFIGURATION ERROR must not point the operator at a Cargo feature:
    // the `ai` feature is GONE, so "recompile with --features ai" is an
    // instruction nobody can follow. The ANNOUNCEMENT (the `WARN` line) is a
    // different thing — it is the paso-5 notice that says the feature is
    // disappearing — so the check targets the error line, not the whole
    // stderr.
    let error_line = stderr
        .lines()
        .find(|line| line.starts_with("Error:"))
        .unwrap_or_default();
    assert!(
        !error_line.contains("--features ai"),
        "the configuration error must not tell the operator to rebuild with a \
         Cargo feature that no longer exists; stderr:\n{stderr}"
    );

    insta::assert_snapshot!(
        "clean_ai_without_embedding_provider_surfaces_config_error",
        redact_nondeterministic(t.out.path(), &stderr)
    );
}

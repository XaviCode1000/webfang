//! AI error propagation behavioral tests (#339).
//!
//! Verifies that when the AI semantic cleaner fails to initialize in an
//! ai-enabled build, `--clean-ai` surfaces the REAL initialization error with
//! the correct exit code, instead of the misleading
//! "Recompile with --features ai" message that the bug produced.
//!
//! The vector is deterministic and network-free: `--clean-ai --offline` with
//! `HF_HOME` pointed at an empty temp dir (the cleaner resolves its hf_hub
//! cache via `Cache::from_env()`), so offline resolution fails fast with
//! `SemanticError::OfflineMode` before any scrape happens.
//!
//! Run with: `cargo nextest run --features ai --test ai_error_propagation`

#![cfg(feature = "ai")]

#[path = "common/cli_harness.rs"]
mod common;
use common::{redact_nondeterministic, webfang_path};

use assert_cmd::Command;

/// `--clean-ai --offline` with an empty hf_hub cache must fail fast with the
/// real `OfflineMode` cause (exit 78, ConfigError), never the false
/// "Recompile with --features ai" message.
#[test]
fn clean_ai_offline_empty_cache_surfaces_real_error() {
    let cache_dir = tempfile::TempDir::new().expect("create empty hf_hub cache");
    let output_dir = tempfile::TempDir::new().expect("create output dir");

    let output = Command::new(webfang_path())
        .arg("--url")
        .arg("https://example.com")
        .arg("--clean-ai")
        .arg("--offline")
        .arg("--ai-model")
        .arg("granite-97m")
        .arg("--output")
        .arg(output_dir.path())
        .env("HF_HOME", cache_dir.path())
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
        !stderr.contains("Recompile with --features ai"),
        "stderr must NOT contain the misleading recompile message:\n{stderr}"
    );
    assert!(
        stderr.contains("Modo offline"),
        "stderr must surface the real OfflineMode cause:\n{stderr}"
    );

    insta::assert_snapshot!(
        "clean_ai_offline_empty_cache_surfaces_real_error",
        redact_nondeterministic(output_dir.path(), &stderr)
    );
}

/// `--clean-ai` online (no `--offline`) with an empty `HF_HOME` cache and the
/// HTTP(S) proxy pointed at a dead address must fail the model download fast
/// and surface the real `Download` cause as `NetworkError` (exit 69), never the
/// false "Recompile with --features ai" message.
///
/// Deterministic and network-free: the proxy `http://127.0.0.1:1` is refused
/// instantly, so hf_hub never reaches the real endpoint nor downloads anything.
#[test]
fn clean_ai_download_failure_surfaces_network_error() {
    let cache_dir = tempfile::TempDir::new().expect("create empty hf_hub cache");
    let output_dir = tempfile::TempDir::new().expect("create output dir");

    let output = Command::new(webfang_path())
        .arg("--url")
        .arg("https://example.com")
        .arg("--clean-ai")
        .arg("--ai-model")
        .arg("granite-97m")
        .arg("--output")
        .arg(output_dir.path())
        .env("HF_HOME", cache_dir.path())
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .output()
        .expect("run webfang binary");

    assert_eq!(
        output.status.code(),
        Some(69),
        "expected NetworkError exit code 69, got {:?}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("Recompile with --features ai"),
        "stderr must NOT contain the misleading recompile message:\n{stderr}"
    );
    assert!(
        stderr.contains("Error descargando modelo"),
        "stderr must surface the real Download cause:\n{stderr}"
    );

    // The online path races model + tokenizer via try_join!, so the asset named
    // in the failing URL is not stable across runs — redact it for a
    // deterministic snapshot.
    insta::with_settings!({
        filters => vec![(r"resolve/main/[A-Za-z0-9._/-]+", "resolve/main/[ASSET]")],
    }, {
        insta::assert_snapshot!(
            "clean_ai_download_failure_surfaces_network_error",
            redact_nondeterministic(output_dir.path(), &stderr)
        );
    });
}

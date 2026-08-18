//! Explicit `--vault` redirect + preflight conflict E2E tests (#762 slice 1).
//!
//! Reproduces the issue-762 audit scenario hermetically with wiremock — no
//! real network:
//!
//! - `webfang --url <seed> --vault <obsidian-vault>` (no `-o`, no
//!   `--quick-save`) must treat the vault as the OUTPUT BASE: the scraped
//!   Markdown lands INSIDE the vault and the default `./output` directory is
//!   never created relative to the working directory.
//! - The contradictory invocation `--vault <vault> -o <custom>` must fail the
//!   step-6e preflight with exit 64 (sysexits EX_USAGE) and a Spanish usage
//!   message BEFORE logging, vault validation, or any network I/O happens.
//!
//! Run with: `cargo nextest run --test vault_redirect_test`

#[path = "common/cli_harness.rs"]
mod common;
use common::{cmd, redact_nondeterministic};

use std::path::Path;
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Minimal page: one article node with enough body text for readability to
/// score it as the main content, and no assets — the redirect contract is
/// proven by WHERE the Markdown file lands, not by its content (content
/// invariants are covered by `obsidian_byline_artifact_test.rs`, slice 2).
const MINIMAL_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><title>Vault Redirect</title></head>
<body>
<article>
<h1>Vault Redirect Probe</h1>
<p>Filler paragraph long enough so readability keeps this node and scores it
as the main article body, proving the scrape pipeline ran end to end.</p>
</article>
</body></html>"#;

/// Create a TempDir that `is_valid_vault` accepts: `validate_explicit_vault`
/// requires a directory containing a `.obsidian/` marker dir.
fn make_vault() -> tempfile::TempDir {
    let vault = tempfile::tempdir().expect("create vault tempdir");
    std::fs::create_dir_all(vault.path().join(".obsidian")).expect("create .obsidian marker dir");
    vault
}

/// True when at least one regular file with `ext` exists under `root`.
fn has_file_with_ext(root: &Path, ext: &str) -> bool {
    WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .any(|e| e.path().extension().is_some_and(|x| x == ext))
}

/// The issue-762 reproduction: an explicit `--vault` flag alone (no `-o`,
/// no `--quick-save`) makes the vault the output base — the scraped Markdown
/// lands inside the vault and no `./output` sibling appears in cwd.
#[tokio::test]
async fn explicit_vault_redirects_output_into_vault() {
    let server = MockServer::start().await;
    let vault = make_vault();
    let cwd = tempfile::tempdir().expect("create cwd tempdir");
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MINIMAL_PAGE))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--max-pages")
        .arg("1")
        .arg("--format")
        .arg("markdown")
        .arg("--vault")
        .arg(vault.path())
        .arg("--quiet")
        .current_dir(cwd.path())
        .assert()
        .success();

    assert!(
        has_file_with_ext(vault.path(), "md"),
        "expected a scraped .md INSIDE the vault at {:?}",
        vault.path()
    );
    assert!(
        !cwd.path().join("output").exists(),
        "default ./output must never be created when --vault is explicit"
    );
}

/// The contradictory invocation (`--vault` + custom non-default `-o`) must
/// fail the step-6e preflight with exit 64 — `CliExit::UsageError` maps to
/// sysexits EX_USAGE (see `EXIT_USAGE_ERROR` in `cli/error.rs`) — and name
/// the conflict in Spanish.
///
/// No mock server is needed on purpose: the process exits BEFORE any network
/// phase, so a URL that parses but is never fetched also proves the early
/// exit (a missed preflight would try to scrape it).
#[tokio::test]
async fn vault_with_custom_output_exits_with_usage_error() {
    let vault = make_vault();
    let out = tempfile::tempdir().expect("create output tempdir");
    let cwd = tempfile::tempdir().expect("create cwd tempdir");

    let output = cmd()
        .arg("--url")
        .arg("https://example.com")
        .arg("--vault")
        .arg(vault.path())
        .arg("-o")
        .arg(out.path())
        .current_dir(cwd.path())
        .output()
        .expect("run webfang binary");

    // Asserted exit code: 64. CliExit::UsageError reports EXIT_USAGE_ERROR
    // (sysexits EX_USAGE) via the Termination impl in cli/error.rs.
    assert_eq!(
        output.status.code(),
        Some(64),
        "expected usage-error exit 64, got {:?}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // The preflight exits before any filesystem side effect.
    assert!(
        !cwd.path().join("output").exists(),
        "the conflict must abort before creating ./output"
    );
    // Semantic invariant beyond the snapshot: the contradiction itself.
    assert!(
        stderr.contains("no pueden combinarse"),
        "stderr must state the --vault/-o contradiction: {stderr}"
    );
    insta::assert_snapshot!(
        "vault_with_custom_output_exits_with_usage_error",
        redact_nondeterministic(out.path(), &stderr)
    );
}

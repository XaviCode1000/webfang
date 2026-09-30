//! #1615 (DF-L2 / F10 / VG-07) — no MCP handler span may record its params.
//!
//! ## Why this file exists, and why it reads source
//!
//! The three audits that reported DF-L2, F10 and VG-07 named the same three
//! handlers and pointed at `fields(params = ?params)`. #1614 replaced that
//! literal on most handlers, leaving three. The remaining three were the
//! *visible* half of the problem; the actual mechanism was `#[instrument]`'s
//! default argument recording, which renders every function argument with
//! `Debug` unless it is named in `skip(...)`.
//!
//! Because every handler is declared as
//!
//! ```ignore
//! async fn tool(&self, Parameters(params): Parameters<ToolParams>) -> ...
//! ```
//!
//! the destructured `params` binding is an in-scope argument, so
//! `#[instrument(skip(self), fields(html_len = ...))]` recorded BOTH the
//! derived field AND `params=<the whole struct>`. Removing the explicit
//! `fields(params = ?params)` changed nothing observable: the span still
//! carried the full body, the full header map, and every free-text field.
//!
//! Fixing that at each call site is what produced this file instead: 33
//! attributes, and nothing to stop the 34th handler from leaking. So the
//! invariant is asserted over the SOURCE, which is where the mistake is made.
//!
//! ## What is and is not checked
//!
//! - **Checked**: every `#[instrument(...)]` in `handlers/` that instruments a
//!   tool must skip the `params` binding. A `fields(params = ?params)` — the
//!   literal the audits named — is also rejected, on the reasoning that it
//!   was never the leak and is never the fix.
//! - **NOT checked**: that the derived fields a handler DOES declare are free
//!   of payload. That is per-handler judgement, and the observation tests in
//!   `handlers/content.rs` and `handlers/security.rs` (`*_span_records_*`)
//!   cover the three handlers whose params carry raw content.

use std::path::PathBuf;

/// Repository-relative location of the handler modules.
const HANDLERS_DIR: &str = "src/mcp_server/handlers";

/// Read one handler module from this crate's source tree.
fn handler_source(file_name: &str) -> String {
    let path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(HANDLERS_DIR)
        .join(file_name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The `#[instrument(...)]` attribute bodies in `source`, in order.
fn instrument_attributes(source: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (idx, _) in source.match_indices("#[instrument") {
        // The attribute may span lines; take up to the closing bracket.
        let rest = &source[idx..];
        let end = rest
            .find(']')
            .unwrap_or_else(|| panic!("unterminated #[instrument at byte {idx}"));
        found.push((idx, rest[..=end].to_string()));
    }
    found
}

/// The signature that follows an `#[instrument]` attribute, up to its body.
///
/// Needed because not every handler is the same shape: one binds
/// `Parameters(params)` and one binds `Parameters(_params)`, and the two
/// differ in whether `#[instrument]` records the argument at all.
fn signature_after(source: &str, attr_end: usize) -> &str {
    let after = &source[attr_end..];
    // Stop at the body's opening brace, so attributes between the signature
    // and the body (there are none today, but a future one must not be read
    // as part of the signature) are not mistaken for it.
    let upto = after.find('{').unwrap_or(after.len());
    &after[..upto]
}

#[test]
fn issue_1615_no_handler_span_records_its_params() {
    let mut checked = 0_usize;
    let mut recorded = 0_usize;
    let mut violations: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(HANDLERS_DIR))
        .expect("handlers dir is readable")
    {
        let path = entry.expect("dir entry is readable").path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("utf-8 file name")
            .to_string();
        let source = handler_source(&file_name);

        for (idx, attr) in instrument_attributes(&source) {
            let line = source[..idx].lines().count();
            let signature = signature_after(&source, idx + attr.len());

            if attr.contains("params = ?params") || attr.contains("params = ?") {
                violations.push(format!(
                    "{file_name}:{line}: `fields(params = ...)` records the whole \
                     params struct — the audits' literal, and never the fix"
                ));
            }

            // A tool handler destructures its argument, so a `params` binding is
            // an in-scope argument and `#[instrument]` renders it with `Debug`
            // unless it is skipped. A `_params` binding is NOT recorded at all,
            // which is why the two shapes are distinguished instead of
            // demanding one spelling.
            let destructures_named_params = signature.contains("Parameters(params)");
            let destructures_any_params =
                signature.contains("Parameters(") || signature.contains("Parameters<");
            if !destructures_named_params {
                if destructures_any_params {
                    // A differently-bound params argument. Accept it only if it
                    // is underscore-prefixed, which `#[instrument]` ignores.
                    assert!(
                        signature.contains("Parameters(_params"),
                        "{file_name}:{line}: unrecognised params binding; update this \
                         guard rather than letting it pass unexamined: {signature}"
                    );
                }
                continue;
            }
            recorded += 1;
            if !attr.contains("skip(self, params)") {
                violations.push(format!(
                    "{file_name}:{line}: #[instrument does not skip `params`, so the \
                     span records the full params struct — use \
                     `#[instrument(skip(self, params), fields(<derived facts>))]`"
                ));
            }
        }
        checked += 1;
    }

    assert!(
        checked > 0,
        "no handler module was scanned in {HANDLERS_DIR}; this guard is vacuous \
         and must be fixed, not satisfied"
    );
    assert!(
        recorded >= 30,
        "expected the tool handlers' #[instrument] attributes here, matched {recorded}; \
         this guard is not seeing what it claims to"
    );
    assert!(
        violations.is_empty(),
        "MCP handler spans must not record tool params (#1615 DF-L2):\n  - {}",
        violations.join("\n  - ")
    );
}

#[test]
fn issue_1615_the_guard_actually_sees_the_attributes_it_claims_to() {
    // A guard that silently scans nothing is worse than no guard: it reports
    // green forever. Pin the count so a refactor that moves the handlers
    // elsewhere fails HERE, with a message naming the move, instead of
    // failing vacuously in the test above.
    let source = handler_source("content.rs");
    let attrs = instrument_attributes(&source);
    assert!(
        attrs.len() >= 7,
        "expected the content handlers' #[instrument] attributes here, found {}",
        attrs.len()
    );
    assert!(
        attrs.iter().all(|(_, a)| a.contains("skip(self, params)")),
        "every content.rs handler span must skip params: {attrs:?}"
    );
}

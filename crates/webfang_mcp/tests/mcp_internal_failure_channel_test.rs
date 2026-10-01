//! #1613 slice 4: internal failures must not masquerade as tool SUCCESS.
//!
//! Two error classes were reported to the agent as a successful tool result,
//! which is the one shape an agent consumer cannot detect:
//!
//! - **EC-06** — a `serde_json` serialization failure produced a SUCCESS whose
//!   entire body was the literal `"failed to serialize"` (four sites:
//!   `assets.rs` `download_assets`, `content.rs` `extract_links`, `scraping.rs`
//!   `discover_urls` / `discover_sitemap`). The report was lost, the call
//!   reported success.
//! - **EC-07** — `open_in_obsidian` answered `DispatchStatus::HandlerFailed`
//!   ("the OS handler exited non-zero — Obsidian is probably not installed")
//!   with a SUCCESS, faking the severity with a `⚠️` inside the body.
//!
//! The failure branches themselves cannot be reached from outside (a
//! `Serialize` impl that is total, and a real OS protocol handler), so the
//! mapping is pinned as unit tests next to the code, in the `#[cfg(test)] mod
//! tests` of each handler: `assets.rs`, `content.rs`, `scraping.rs` and
//! `obsidian.rs`. This file covers what IS reachable deterministically
//! end-to-end — that the success channel of the same tools still works after
//! the `match` change, so nothing was traded away for the error path.
//!
//! Run with: cargo nextest run --test mcp_internal_failure_channel_test --features mcp

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use wreq::Client;

mod common;
use common::{call_tool, init_session, is_tool_error, start_test_server, tool_text};

/// `RemoteDerived` bodies are indented one space per line as part of the
/// prompt-injection defense (`provenance::indent_lines`), so a JSON payload
/// must be de-indented before it parses. `tool_text` strips the envelope but
/// deliberately keeps the indentation.
fn dedent(payload: &str) -> String {
    payload
        .lines()
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `extract_links` on the happy path must still be a NORMAL success, and its
/// body must be the real JSON array of links.
///
/// This is the guard for the EC-06 fix: the handler's serialization step is
/// now a `match`, so a mistake there (inverted arms, an error-wrapped success)
/// would be invisible to a test that only checked the failure branch. The body
/// assertion is deliberately semantic — it parses the array and compares the
/// URLs — because "the body is some text" would pass for a placeholder too.
#[tokio::test]
async fn extract_links_happy_path_is_a_normal_success_with_the_real_json() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "extract_links",
        json!({
            "html": "<html><body><a href=\"/page\">link</a></body></html>",
            "base_url": "https://example.com"
        }),
    )
    .await;

    let result: &Value = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected a tool result, got: {resp}"));
    assert!(
        !is_tool_error(result),
        "a serializable link list must NOT be a tool error: {}",
        tool_text(result)
    );

    let links: Vec<String> = serde_json::from_str(&dedent(&tool_text(result)))
        .unwrap_or_else(|e| panic!("body must be a JSON array of links ({e}), got: {resp}"));
    assert_eq!(
        links,
        vec!["https://example.com/page".to_string()],
        "the body must be the real link list, not a placeholder"
    );
}

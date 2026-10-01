//! #1613 slice 3 (EC-02): settle the rmcp error-channel mapping, on a transport.
//!
//! EC-02 was classified **"Needs verification"** in `docs/src/mcp-error-contract.md`
//! (rows A1 and A2, "Unsettled rows"): this crate documents the mapping twice
//! (`mcp_server/params.rs:43-47` and `tests/params_rejection_test.rs:85-100`) and
//! contradicts itself about a neighbouring case, so nothing in the crate actually
//! *proved* which channel an MCP error travels on. This file is that proof, and
//! it is deliberately **not a code fix** — it observes and pins.
//!
//! # The mechanism being pinned (read out of rmcp 1.8.0 source)
//!
//! `rmcp-1.8.0/src/handler/server/tool.rs:181-196` —
//! `impl FromContextPart<ToolCallContext<'_>> for Parameters<P>` deserializes the
//! tool arguments with `serde_json::from_value` and wraps **any** serde error as
//! `ErrorData::invalid_params(format!("failed to deserialize parameters: {error}"), None)`.
//! Note the prefix: rmcp supplies it, so a custom `TryFrom<String>` message never
//! has to reproduce it to be classified correctly.
//!
//! `rmcp-1.8.0/src/handler/server/router/tool.rs:144-156` —
//! `into_tool_argument_error` downgrades to `CallToolResult::error` (i.e.
//! `isError: true`) **only** when the error's code is `INVALID_PARAMS` **and**
//! its message starts with the literal `"failed to deserialize parameters:"`
//! (`TOOL_ARGUMENT_DESERIALIZATION_ERROR_PREFIX`). Anything else is returned as
//! `Err(..)` from `ToolRouter::call` (`router/tool.rs:573-575`) and becomes a
//! JSON-RPC protocol error.
//!
//! So there are **two distinct routes that share one Rust error type**
//! (`McpError::invalid_params` *is* `rmcp::ErrorData`, aliased at
//! `mcp_server/params.rs:15`) — which is exactly what made the mapping look
//! ambiguous from the source alone:
//!
//! | Route | Where the error is raised | Lands on |
//! | :--- | :--- | :--- |
//! | Argument deserialization (`McpUrl` `try_from`, `deny_unknown_fields`) | inside rmcp, before the handler body | `result.isError = true`, **no** `error` member |
//! | Handler `params.validate()?` | inside the handler body | `error.code = -32602`, **no** `result` member |
//!
//! The three tests below pin one row each, asserting **both** observed channels
//! with no hedging: an "A or B" assertion here would re-create the ambiguity this
//! file exists to remove. Every assertion message embeds the whole response, so a
//! mismatch is diagnosable from the test output alone.
//!
//! Run with: `cargo nextest run --test mcp_error_channel_mapping_test --features mcp`

#![cfg(feature = "mcp")]

use serde_json::json;
use wreq::Client;

mod common;
use common::{
    call_tool, error_code, init_session, is_tool_error, tool_text, JSONRPC_INVALID_PARAMS,
};

// ============================================================================
// Row A1 — argument deserialization (`McpUrl` `#[serde(try_from = "String")]`,
// `mcp_server/params.rs:52-53`).
// ============================================================================

/// EC-02 / contract row A1: a URL the `McpUrl` boundary refuses (`file://`, a
/// scheme `ValidUrl::parse` rejects — `mcp_server/params.rs:81-93`) must arrive
/// as a **tool error** (`result.isError == true`) with **no** top-level `error`
/// member.
///
/// Why tool error and not `-32602`: the failure happens inside rmcp's
/// `Parameters<P>` deserialization, so rmcp itself prepends
/// `"failed to deserialize parameters:"` to whatever `McpUrl::try_from`
/// returned. That prefix is the whole discriminator `into_tool_argument_error`
/// looks at — the custom message does not need to fake it
/// (`rmcp-1.8.0/src/handler/server/tool.rs:181-196` →
/// `router/tool.rs:144-156`).
///
/// The reason text is asserted to name the offending scheme because the
/// deserialization message is the only channel a client has here: unlike the
/// protocol-error channel, `isError: true` carries no machine-readable `code`,
/// so the content text is the diagnosis.
#[tokio::test]
async fn mcp_url_deserialization_failure_is_a_tool_error_not_a_protocol_error() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "file:///etc/passwd" }),
    )
    .await;

    assert!(
        resp.get("error").is_none(),
        "an McpUrl deserialization failure must NOT be a JSON-RPC protocol error \
         (rmcp downgrades it to isError:true); got a top-level `error` member in: {resp}"
    );

    let result = resp.get("result").unwrap_or_else(|| {
        panic!("an McpUrl deserialization failure must return a `result` member, got: {resp}")
    });
    assert!(
        is_tool_error(result),
        "an McpUrl deserialization failure must set result.isError = true, got: {resp}"
    );

    let text = tool_text(result);
    assert!(
        text.to_lowercase().contains("file"),
        "the tool text must name the offending `file` scheme, got: {resp}"
    );
}

// ============================================================================
// Row A2 — argument deserialization via `#[serde(deny_unknown_fields)]`
// (`mcp_server/params.rs:7`, per-struct attribute).
// ============================================================================

/// EC-02 / contract row A2: an unknown JSON key must arrive as a **tool error**
/// (`result.isError == true`) with **no** top-level `error` member.
///
/// Same route as A1 and for the same reason: `deny_unknown_fields` fires during
/// the same `serde_json::from_value` call in rmcp's `Parameters<P>` extractor,
/// so it is wrapped with the same
/// `"failed to deserialize parameters: {error}"` prefix and downgraded to
/// `isError: true` by `into_tool_argument_error`
/// (`rmcp-1.8.0/src/handler/server/tool.rs:188-193` →
/// `router/tool.rs:146-155`). This test is what upgrades A2 from *unverified*
/// to pinned — `tests/params_rejection_test.rs:300-315` asserts `is_tool_error`
/// but never asserts the **absence** of a protocol error, so on its own it would
/// still pass if both channels appeared (`tests/params_rejection_test.rs:292-315`).
#[tokio::test]
async fn mcp_unknown_field_is_a_tool_error_not_a_protocol_error() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_with_options",
        json!({ "url": "https://example.com", "typo_field": 1 }),
    )
    .await;

    assert!(
        resp.get("error").is_none(),
        "a deny_unknown_fields rejection must NOT be a JSON-RPC protocol error \
         (rmcp downgrades it to isError:true); got a top-level `error` member in: {resp}"
    );

    let result = resp.get("result").unwrap_or_else(|| {
        panic!("a deny_unknown_fields rejection must return a `result` member, got: {resp}")
    });
    assert!(
        is_tool_error(result),
        "a deny_unknown_fields rejection must set result.isError = true, got: {resp}"
    );
}

// ============================================================================
// The other route — handler-level `params.validate()?` (contract row A3).
// ============================================================================

/// EC-02, the mirror image: a **handler-level** validation failure must arrive as
/// a **JSON-RPC protocol error** (`error.code == -32602`) with **no** `result`
/// member.
///
/// `crawl_site` with `max_depth: 11` is the fixture because the arguments
/// deserialize cleanly — the URL is a valid `https` and every field is
/// well-typed — and the rejection happens in the handler's own
/// `params.validate()?` (`max_depth` is checked at
/// `mcp_server/params.rs:502-506`, before any fetch). So the error is raised by
/// *our* code, not by rmcp's extractor, and it therefore never carries
/// `"failed to deserialize parameters:"`. `into_tool_argument_error`'s
/// prefix test fails, the error propagates as `Err(..)`, and the router answers
/// with a JSON-RPC error (`rmcp-1.8.0/src/handler/server/router/tool.rs:155`
/// and `:573-575`).
///
/// This is the row that proves the two routes are distinguishable at the wire
/// despite sharing one Rust type (`McpError::invalid_params` ==
/// `rmcp::ErrorData`, `mcp_server/params.rs:15`): A1/A2 → `isError: true`,
/// A3 → `-32602`.
#[tokio::test]
async fn mcp_handler_validate_failure_is_a_protocol_error_not_a_tool_error() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "crawl_site",
        json!({ "url": "https://example.com", "max_depth": 11 }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "a handler-level params.validate()? failure must be a JSON-RPC \
         -32602, got: {resp}"
    );

    assert!(
        resp.get("result").is_none(),
        "a handler-level params.validate()? failure must NOT return a `result` \
         member (it is a protocol error, not isError:true), got: {resp}"
    );
}

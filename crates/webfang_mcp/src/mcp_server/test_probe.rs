//! Env-gated panic probe — the only tool in the registry that panics on
//! request, and only when an operator asks for it (#1626, PC-3).
//!
//! Why it exists: F2 containment (#1611) is already proven in-process over the
//! real HTTP stack (`tests/mcp_panic_containment_test.rs`), but the stdio
//! transport is the one MCP clients actually spawn, and on stdio a contained
//! panic must ALSO leave the same structured, location-bearing record the HTTP
//! transport owes the operator. Proving that needs a `tools/call` that really
//! panics, and no production tool panics on demand. The existing in-crate seam
//! (`build_mcp_router_with_service`, `#[cfg(test)]`) is invisible to an
//! integration test that spawns the real binary, so the trigger has to be
//! reachable from the wire.
//!
//! The switch is the PRESENCE of [`PANIC_PROBE_ENV`] with a non-empty value
//! (same `WEBFANG_*` convention as `WEBFANG_AI_MODEL_ID`): an operator has to
//! set it, deliberately, per process. Absent or empty, [`build_router`] is
//! never called by the registry and the advertised tool surface is byte-for-byte
//! what it was before this module existed.
//!
//! The panic is raised inside the dispatch future and contained by the EXISTING
//! `catch_unwind` in `McpHandler::call_tool` — this module adds no second
//! containment layer and does not touch that code. Containment is the
//! observability: `setup_panic_hook` logs the location and the message, and
//! `call_tool` logs the payload, so the probe needs no instrumentation of its
//! own.

use std::future::Future;
use std::pin::Pin;

use rmcp::handler::server::tool::{ToolCallContext, ToolRoute, ToolRouter};
use rmcp::model::{CallToolResult, JsonObject, Tool};
use rmcp::ErrorData as McpError;

use super::McpHandler;

/// Environment variable that enables [`PANIC_PROBE_TOOL_NAME`].
///
/// Any non-empty value enables it; the value itself is never read, because
/// there is nothing to configure about a probe.
pub const PANIC_PROBE_ENV: &str = "WEBFANG_MCP_TEST_PANIC_TOOL";

/// Wire name of the probe tool.
///
/// Reads unmistakably as test-only to a human scanning `tools/list` in a bug
/// report, and it is a valid MCP tool name (letters plus underscores, so rmcp's
/// own name validator stays quiet).
pub const PANIC_PROBE_TOOL_NAME: &str = "test_panic_probe";

/// The panic message the probe raises.
///
/// Also the marker an end-to-end test asserts on, so it stays stable and
/// unique — a substring that a real panic could plausibly produce would let a
/// regression pass by accident.
pub const PANIC_PROBE_MESSAGE: &str = "webfang-pc3-panic-probe: deliberate tool panic";

/// Is the panic probe enabled in this process?
///
/// Presence-with-a-value of [`PANIC_PROBE_ENV`], matching how
/// `WEBFANG_AI_MODEL_ID` is treated elsewhere: a read-only env query, so it
/// needs none of the `webfang_test_utils` mutation serialization (#1126).
pub fn panic_probe_enabled() -> bool {
    std::env::var_os(PANIC_PROBE_ENV).is_some_and(|value| !value.is_empty())
}

/// Build the router carrying the single probe tool.
///
/// Callers gate on [`panic_probe_enabled`]; this function does not re-check, so
/// the decision stays visible at the registration site in
/// `handlers::build_tool_router`.
pub fn build_router() -> ToolRouter<McpHandler> {
    ToolRouter::<McpHandler>::new().with_route(ToolRoute::new_dyn(
        probe_tool_attribute(),
        panic_probe_route,
    ))
}

/// The advertised tool definition. Takes no arguments, and — like every other
/// tool an agent can see — carries the standard provenance notice so a client
/// that discovers the probe while the switch is on is told the same thing it
/// is told about the real ones.
fn probe_tool_attribute() -> Tool {
    let mut schema = JsonObject::new();
    schema.insert(
        "type".to_string(),
        serde_json::Value::String("object".to_string()),
    );
    schema.insert("properties".to_string(), serde_json::json!({}));
    Tool::new(
        PANIC_PROBE_TOOL_NAME,
        format!(
            "TEST-ONLY PROBE ({PANIC_PROBE_ENV} is set): panics on purpose so panic \
             containment can be exercised end to end. Never enable this in a \
             deployment. {}",
            crate::mcp_server::provenance::INJECTION_NOTICE
        ),
        schema,
    )
}

/// The panicking body.
///
/// An `async` block on purpose: the panic is raised while the dispatch future is
/// being POLLED, which is the only shape the F2 containment in
/// `McpHandler::call_tool` is proven against (a tool that panics after its own
/// `.await`).
fn panic_probe_route<'a>(
    _context: ToolCallContext<'a, McpHandler>,
) -> Pin<Box<dyn Future<Output = Result<CallToolResult, McpError>> + Send + 'a>> {
    Box::pin(async { panic!("{PANIC_PROBE_MESSAGE}") })
}

//! MCP Handler modules — tool implementations organized by category
//!
//! Each module provides a `#[tool_router]` impl block with tools and a
//! `build_router()` function that returns a partial `ToolRouter<McpHandler>`.
//! All routers are combined with the `+` operator in `build_tool_router()`.

pub use super::McpHandler;

use rmcp::handler::server::tool::ToolRouter;

use crate::mcp_server::test_probe;

pub mod ai;
pub mod assets;
pub mod axtree;
pub mod content;
pub mod export;
pub mod obsidian;
pub mod scraping;
pub mod security;
pub mod url_utils;

/// Test-only fixtures shared by the per-handler unit-test modules. Compiled
/// only under `cfg(test)`; never part of the release surface.
#[cfg(test)]
pub(crate) mod test_support;

/// Build the combined ToolRouter from all 9 category modules.
///
/// After combining the category routers, the schema bridge overrides the
/// advertised input schemas of tools whose parameters overlap an
/// OptionsSpec entry (ADR-002 slice 4, #940).
///
/// # Test-only surface shipped in release builds — read before changing this
///
/// When `WEBFANG_MCP_TEST_PANIC_TOOL` is set in the process environment, this
/// registry ALSO advertises `test_panic_probe`, a tool whose body panics on
/// purpose. That is deliberate (#1626, PC-3), and the tradeoff is:
///
/// - **It ships in release builds.** Gating it behind `cfg(test)` would make
///   it invisible to the one thing it exists for: an integration test that
///   spawns the real `webfang-mcp-stdio` binary and drives JSON-RPC over
///   stdio, where the in-crate `#[cfg(test)]` seam cannot be seen. There is no
///   production tool that panics on demand, so without a wire-reachable
///   trigger the E2E half of the contract ("a stdio round trip asserting a
///   record exists after a contained panic") is not testable.
/// - **It is contained, not a foot-gun.** The panic is caught by the existing
///   `catch_unwind` in `McpHandler::call_tool`: the session survives, the
///   caller gets the normal `isError: true` result, and the process keeps
///   serving. It cannot crash the server, and it grants no access to anything
///   the 36 real tools do not already expose.
/// - **It exists only when an operator sets the variable.** Unset or empty, the
///   advertised surface is byte-for-byte what it was before this was added,
///   which the `tools.len() == 36` stdio assertion pins.
/// - **What it costs:** a deployment that sets the variable in production gets
///   one extra advertised tool that always fails. That is a configuration
///   mistake with a visible symptom, not a latent vulnerability — and the
///   trade is worth it, because the alternative is an untested panic path on
///   the transport that matters most.
///
/// Do not "harden" this by adding a second containment layer, by moving the
/// registration behind a feature flag, or by removing the env gate.
pub fn build_tool_router() -> ToolRouter<McpHandler> {
    let mut router = scraping::build_router()
        + content::build_router()
        + export::build_router()
        + url_utils::build_router()
        + security::build_router()
        + obsidian::build_router()
        + assets::build_router()
        + ai::build_router()
        + axtree::build_router();
    crate::mcp_server::schema_bridge::apply_overrides(&mut router);
    // #1626 PC-3: the env-gated probe is added AFTER the schema bridge on
    // purpose — the bridge is the OptionsSpec SSOT for the real tools, and a
    // zero-argument test probe has no spec to derive a schema from.
    if test_probe::panic_probe_enabled() {
        router += test_probe::build_router();
    }
    router
}

//! MCP Server — Model Context Protocol bridge for AI agents
//!
//! Exposes 36 scraper tools across 9 categories via Streamable HTTP.
//! Architecture:
//! - `state.rs` — McpState with embedded Container + per-category semaphores
//! - `server.rs` — Axum router + StreamableHttpService setup
//! - `handlers/` — 9 handler modules (one per tool category)
//!
//! Backpressure: Each category has its own tokio::sync::Semaphore
//! to prevent resource exhaustion on constrained hardware.

#[macro_use]
pub mod macros;
pub mod auth;
pub mod handlers;
pub mod metrics;
pub mod panic_hook;
pub mod params;
pub mod provenance;
// Crate-internal: the gate's items are `pub(crate)`, so documenting the
// module as `pub` would make its docs public documentation linking to
// private items (rustdoc `private_intra_doc_links`, denied by CI).
pub(crate) mod path_gate;
pub mod schema_bridge;
pub mod server;
pub mod ssrf;
pub mod state;
// Crate-internal: the env-gated panic probe is a test trigger, not API an
// integrator should reach for. See `handlers::build_tool_router` for the
// security posture and the registration site.
pub(crate) mod test_probe;
pub mod validation;

/// Vault-search AI port wiring (#433) — only compiled with the `ai` feature.
#[cfg(feature = "ai")]
pub mod ai_wiring;

use futures::FutureExt;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{CallToolResult, Content, ListToolsResult, ServerCapabilities, ServerInfo, Tool};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer};

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use webfang_core::di::ContainerExt;

pub use state::McpState;

/// Build a Container — fast container construction, no model resolution
/// happens here (#759).
///
/// This is used by both `mcp_server_http.rs` and `mcp_server_stdio.rs`
/// binaries to avoid code duplication. The AI wiring that used to run inside
/// this function (and block the MCP `initialize` handshake behind the hf_hub
/// model resolution) is now performed lazily by [`spawn_ai_wiring`] after the
/// server starts serving (#759).
///
/// # Errors
///
/// Returns the typed construction error (e.g. HTTP client setup failure) so
/// the calling binaries can log it and exit with a code instead of aborting
/// the process (#1123).
pub async fn build_container(
) -> Result<webfang_core::di::Container, Box<dyn std::error::Error + Send + Sync>> {
    let config = webfang_core::config::Config::default();
    webfang_core::di::Container::from_config(config).await
}

/// Build the shared asset `Downloader` for the long-lived MCP servers (#1120).
///
/// Single graph (#1149): built through the `Container` composition root, so
/// the bounded policy (capacity from the budget model's Asset tier) has one
/// home shared with the CLI ephemeral runs. The server process outlives any
/// single crawl, so it must NOT take the legacy unbounded `Downloader::new`
/// path (`usize::MAX` capacity disables the dedup-cache eviction).
///
/// # Errors
/// Propagates HTTP-client construction failures (`ScraperError::Config`).
pub fn build_shared_downloader(
) -> webfang_core::error::Result<webfang_core::adapters::downloader::Downloader> {
    webfang_core::application::container::Container::build_mcp_shared_asset_downloader()
}

/// Compose the shared [`McpState`] for the long-lived MCP servers.
///
/// Single composition root for BOTH transports (stdio and HTTP, #1300):
/// injecting the bounded shared downloader here makes pool reuse across
/// tool calls structural — a transport that forgets the call no longer
/// compiles against this helper. Previously the stdio binary built its
/// state without [`McpState::with_downloader`], so every
/// `download_assets` call re-created the connection pool and #1120's
/// churn persisted on that transport.
///
/// The DOM inspector is intentionally NOT wired here: MCP production
/// wiring of the inspector is #1294 (NS-01 / slice D), which owns the
/// verify-or-fix decision for both transports.
///
/// # Errors
/// Propagates [`build_shared_downloader`] failures
/// (`ScraperError::Config`).
pub fn build_mcp_state(
    container: std::sync::Arc<webfang_core::application::container::Container>,
    export_roots: Vec<std::path::PathBuf>,
) -> webfang_core::error::Result<McpState> {
    Ok(McpState::from_container(container)
        .with_downloader(std::sync::Arc::new(build_shared_downloader()?))
        .with_export_roots(export_roots))
}

/// Kick off the lazy AI port wiring in a background task (#759).
///
/// Shares the same `Arc<Container>` that the MCP server already holds and
/// injects the AI ports (semantic cleaner, embedding, chunker, notes) after
/// the server has started serving. This unblocks the MCP `initialize`
/// handshake, which previously waited on the hf_hub model resolution
/// (~390 MB download on a cold cache). During warmup the AI tools degrade to
/// their pre-existing honest "not available" error.
#[cfg(feature = "ai")]
pub fn spawn_ai_wiring(container: Arc<webfang_core::application::container::Container>) {
    use tracing::Instrument;

    // Loud env resolution (#874): a set-but-invalid AI_MODEL_ID must never be
    // silently downgraded to the default model — skip AI wiring with an error
    // log instead (English logs per project convention; the MCP tools then keep
    // their pre-existing honest "not available" error during this run).
    let variant = match webfang_ai::AiModel::from_env() {
        Ok(Some(variant)) => variant,
        Ok(None) => webfang_ai::AiModel::default(),
        Err(e) => {
            tracing::error!(
                variable = "AI_MODEL_ID",
                error = %e,
                "AI wiring skipped: AI_MODEL_ID is set to an unknown model and \
                 cannot be silently defaulted"
            );
            return;
        },
    };
    // Engine selection, resolved exactly like the CLI (#1569): `WEBFANG_AI_ENGINE`
    // (`single` | `pool:<N>`), unset meaning the MEASURE-calibrated `Pool`
    // default. Same #874 discipline: a set-but-invalid value skips AI wiring
    // with an error log instead of silently building a different engine.
    let engine_config = match webfang_ai::infrastructure_ai::EngineConfig::from_env() {
        Ok(config) => config,
        Err(e) => {
            tracing::error!(
                variable = "WEBFANG_AI_ENGINE",
                error = %e,
                "AI wiring skipped: WEBFANG_AI_ENGINE is set to an invalid engine \
                 spec and cannot be silently defaulted"
            );
            return;
        },
    };
    let span = tracing::info_span!(
        "ai_lazy_wiring",
        model = variant.display_name(),
        engine = ?engine_config
    );

    tokio::spawn(
        async move {
            let model_config = webfang_ai::ModelConfig::default().with_model_variant(variant);
            match webfang_ai::SemanticCleanerImpl::new_with_engine_config(
                model_config,
                engine_config,
            )
            .await
            {
                Ok(cleaner) => {
                    // Erased shared inference (#1569): the SAME engine +
                    // tokenizer back the cleaner, the vault-search embedding
                    // adapter and Tier 2 — one model load, Single and Pool
                    // modes alike.
                    let (engine, tokenizer) = cleaner.shared_inference();
                    let cleaner: Arc<dyn webfang_core::domain::semantic_cleaner::SemanticCleaner> =
                        Arc::new(cleaner);
                    container.inject_vault_ports(
                        webfang_core::application::container::VaultAiPorts {
                            cleaner: Some(cleaner),
                            ..Default::default()
                        },
                    );
                    ai_wiring::wire_ai_ports(&container, engine, tokenizer).await;
                    tracing::info!("AI ports wired (lazy, post-handshake)");
                },
                Err(e) => tracing::warn!(error = %e, "AI warmup failed; continuing without AI"),
            }
        }
        .instrument(span),
    );
}

/// No-op placeholder when the `ai` feature is not compiled in (#759).
#[cfg(not(feature = "ai"))]
pub fn spawn_ai_wiring(_container: Arc<webfang_core::application::container::Container>) {}

/// The DOM inspector every MCP server instance ships with (#1294 NS-01).
///
/// `McpState::inspector` defaults to `None` (`state.rs:164`) and no MCP
/// composition root ever set one, while the scrape handlers pass
/// `state.inspector.as_deref()` straight into the scrape use case
/// (`handlers/scraping.rs:155`). The CLI has wired [`DefaultDomInspector`] in
/// production since that port landed (`webfang_cli/src/main.rs:428`), so every
/// CSS-selector diagnostic — the DOM structure report and the near-miss
/// suggestions — silently degraded to "no diagnostics" for MCP clients only.
///
/// One shared constructor keeps both transports on the same implementation, the
/// way [`build_shared_downloader`] keeps the asset-download policy shared.
///
/// [`DefaultDomInspector`]: webfang_core::infrastructure::scraper::dom_inspector::DefaultDomInspector
#[must_use]
pub fn default_dom_inspector() -> Arc<dyn webfang_core::domain::DomInspectorPort> {
    Arc::new(webfang_core::infrastructure::scraper::dom_inspector::DefaultDomInspector::new())
}

/// Main MCP handler struct.
///
/// Holds the application state and combined tool router.
/// All 36 tools are registered via `#[tool]` attributes
/// in the handler submodules.
#[derive(Clone)]
pub struct McpHandler {
    /// Shared application state (DI container + semaphores)
    pub state: McpState,
    /// Combined tool router from all 9 categories
    pub tool_router: ToolRouter<Self>,
}

impl McpHandler {
    /// Create a new MCP handler with the given state.
    pub fn new(state: McpState) -> Self {
        Self {
            state,
            tool_router: handlers::build_tool_router(),
        }
    }

    /// Create a new MCP handler from a caller-supplied [`ToolRouter`].
    ///
    /// Exists so tests can mount an extra tool on the REAL server composition
    /// (same router, same middleware stack, same transport) instead of
    /// simulating one. The production path never calls it: both composition
    /// roots go through [`McpHandler::new`], so the advertised tool surface
    /// cannot drift because of it.
    ///
    /// The F2 use is panic containment (#1611): a test tool whose body panics
    /// must be reachable through the real stack to prove the panic becomes a
    /// tool error instead of a dead session.
    pub fn with_tool_router(state: McpState, tool_router: ToolRouter<Self>) -> Self {
        Self { state, tool_router }
    }
}

/// User-facing (Spanish) text a client receives when a tool panics.
///
/// Deliberately free of the panic payload: the payload may embed request data,
/// and it belongs in the trace, not in an answer an agent will read back into a
/// conversation (#1611, F2).
const PANIC_CONTAINED_TOOL_ERROR: &str =
    "La herramienta entró en un error interno y fue contenida. La sesión sigue activa: \
     reintenta la llamada o usa otra herramienta.";

/// Render a panic payload as a SHORT string for the trace.
///
/// # What lands in the trace
///
/// This is the ONLY structured record of a contained panic on every transport,
/// so it must stand on its own on two axes:
///
/// - **Total.** A `panic_any` with a non-string type is legal, and the logger
///   must not panic while reporting a panic.
/// - **Bounded in the OUTPUT, cheaply.** A handler can panic with a megabyte of
///   buffered HTML in its message, so at most `MAX_CHARS` characters are kept
///   and the rest is dropped. The bound is applied while walking, not after:
///   counting the whole payload to decide whether to truncate cost as much as
///   logging it, and the cost is exactly what a panic wants to avoid (#1626,
///   PC-2).
///
/// # What it does NOT do
///
/// It is not a redaction layer, and it must not be described as one — the
/// earlier version of this doc called it "non-sensitive", which was false: the
/// payload is copied verbatim, and a panic message can embed request data, a URL
/// with credentials, or buffered page content. If a handler panics with a secret
/// in its message, the first 200 characters of it reach the trace. That is a
/// deliberate trade: an operator debugging a panic needs the message, and the
/// trace is the operator's own sink. The rule that follows from it is at the
/// call site — **never write a secret into a panic message** — not a
/// transformation this function performs.
///
/// It is also not the full record. The panic hook (`super::panic_hook`) adds the
/// panic LOCATION, and it is installed by [`start_mcp_server`] only — the stdio
/// transport does not install it, so on stdio the location is whatever the
/// default stderr hook prints and nothing more. Do not make a location-bearing
/// diagnosis depend on this function, and do not "upgrade" it to carry one.
///
/// [`start_mcp_server`]: super::server::start_mcp_server
pub(crate) fn render_panic_payload(payload: &(dyn Any + Send)) -> String {
    const MAX_CHARS: usize = 200;
    let raw = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("<non-string panic payload>");
    // `char_indices().nth(MAX_CHARS)` stops on the (MAX_CHARS + 1)-th char, so
    // the walk is bounded by the constant instead of by the payload length —
    // the whole point of PC-2. The index it yields is a char boundary, so the
    // slice below is always valid UTF-8.
    match raw.char_indices().nth(MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &raw[..cut]),
        None => raw.to_string(),
    }
}

/// Implement ServerHandler for McpHandler.
///
/// Uses the combined `self.tool_router` field (all 9 category routers)
/// for tool dispatch, listing, and lookup.
impl ServerHandler for McpHandler {
    /// Dispatch a tool call, containing a panicking tool body (#1611, F2).
    ///
    /// Why the HTTP layer is not enough: rmcp's
    /// `StreamableHttpService::spawn_session_worker` runs this handler inside
    /// `tokio::spawn`, and rmcp 1.8.0 has no `catch_unwind` anywhere in its
    /// source. A panic raised in a tool therefore never unwinds the HTTP
    /// request future — it unwinds the session worker task, which drops
    /// `svc.waiting()`, closes the session, and leaves the client holding a
    /// dead transport with no error. `CatchPanicLayer` on the router (see
    /// `super::server::build_mcp_router`) can only see panics raised on the
    /// HTTP request path, so it cannot rescue this one.
    ///
    /// Catching it here makes the failure a normal tool error over HTTP 200
    /// (`isError: true`), keeps the session worker alive for the next call,
    /// and works on every transport (stdio included), because containment
    /// happens at the dispatch boundary rather than at the HTTP boundary.
    ///
    /// `AssertUnwindSafe` is the honest annotation: the guarded future borrows
    /// `&self` across awaits, which the compiler cannot prove unwind-safe,
    /// and the invariant we rely on is exactly the one this function
    /// establishes — a caught panic is logged and reported, never resumed.
    ///
    /// Known limit, accepted on purpose: containment restores the TRANSPORT, not
    /// the side effects. A tool that panicked after having already written an
    /// export, spawned a crawl, or charged a rate-limit token is reported as a
    /// clean failure, so a client that retries duplicates that work. Rolling the
    /// effect back is a per-tool idempotency concern (the CLI export path owns
    /// it), not something a boundary `catch_unwind` can provide — and a tool
    /// that panics is the one case where the caller is told exactly what
    /// happened, because the panic hook logged it.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = request.name.clone();
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        match AssertUnwindSafe(self.tool_router.call(tcc))
            .catch_unwind()
            .await
        {
            Ok(result) => result,
            Err(payload) => {
                tracing::error!(
                    tool = %tool_name,
                    panic.payload = %render_panic_payload(payload.as_ref()),
                    "MCP tool panicked — contained as a tool error; session worker survives"
                );
                Ok(CallToolResult::error(vec![Content::text(
                    PANIC_CONTAINED_TOOL_ERROR,
                )]))
            },
        }
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::from_build_env())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1294 NS-01: the helper must hand out the REAL inspector, not a stub.
    ///
    /// `NoOpInspector` answers every suggestion with an empty vec, so a
    /// non-empty near-miss list is what separates "wired" from "wired with
    /// something that does nothing". The per-binary tests check the wiring; this
    /// checks the thing being wired.
    #[test]
    fn default_dom_inspector_reports_near_miss_suggestions() {
        let inspector = default_dom_inspector();
        let document = scraper::Html::parse_document(
            r#"<html><body>
                   <div class="article-body"><p class="article-title">content</p></div>
                 </body></html>"#,
        );

        let suggestions = inspector.suggest(&document, ".article-body");
        assert!(
            !suggestions.is_empty(),
            "the production inspector must produce selector suggestions"
        );
        assert!(
            inspector.inspect(&document).element_count > 0,
            "the production inspector must produce a non-empty DOM report"
        );
    }

    /// #1120: the server composition root must never hand out the legacy
    /// unbounded (`usize::MAX`) downloader — the cache bound is the same
    /// budget-derived value the CLI orchestrator uses.
    #[test]
    fn build_shared_downloader_is_bounded() {
        use webfang_core::adapters::downloader::asset_cache_capacity;
        use webfang_core::domain::budget::{
            detector::SystemDetector, BudgetModel, BudgetOverrides,
        };

        let downloader = build_shared_downloader().expect("downloader builds");
        let expected = asset_cache_capacity(
            BudgetModel::build(BudgetOverrides::default(), &SystemDetector)
                .asset()
                .get(),
        );

        assert_ne!(
            downloader.asset_cache_capacity(),
            usize::MAX,
            "long-lived server must not use the unbounded legacy cache"
        );
        assert_eq!(downloader.asset_cache_capacity(), expected);
    }

    /// #1300: BOTH transports share one composition root, so the stdio
    /// server gets the same bounded shared downloader as HTTP — the #1120
    /// pool churn cannot return on any transport that composes through the
    /// helper. Export roots and the documented inspector boundary are
    /// pinned alongside.
    #[tokio::test]
    async fn build_mcp_state_shares_bounded_downloader_and_export_roots() {
        let container = std::sync::Arc::new(build_container().await.expect("container boots"));
        let roots = vec![std::path::PathBuf::from("/tmp/webfang-test-export-roots")];

        let state = build_mcp_state(container, roots.clone()).expect("state composes");

        let downloader = state
            .downloader
            .as_ref()
            .expect("composition root must inject the shared bounded downloader");
        assert_ne!(
            downloader.asset_cache_capacity(),
            usize::MAX,
            "long-lived server must not use the unbounded legacy cache"
        );
        assert_eq!(state.allowed_export_roots, roots.into());
        // The inspector is intentionally not wired here: MCP production
        // wiring is #1294 (NS-01 / slice D) for both transports.
        assert!(state.inspector.is_none());
    }

    /// Contract guard for #1123: `build_container` propagates the typed
    /// construction error as `Err` instead of aborting the process with a
    /// `panic!`. Against the pre-fix signature (`-> Container`) this test does
    /// not compile (`is_ok` does not exist on `Container`) — that compile
    /// failure on main is the reproduction evidence that the panic-as-error
    /// handling contract existed. The panic itself is defensive: probes with
    /// hostile proxy env vars showed `Container::from_config` cannot fail from
    /// the binaries' default config, so no runtime repro is reachable.
    #[tokio::test]
    async fn build_container_returns_result_and_boots_with_default_config() {
        let result = build_container().await;
        assert!(
            result.is_ok(),
            "default config must produce a valid container, got Err: {:?}",
            result.err().map(|e| e.to_string())
        );
    }
}

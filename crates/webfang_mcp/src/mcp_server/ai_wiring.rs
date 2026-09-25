//! AI port wiring for the MCP server (#433).
//!
//! Constructs the concrete vault-search ports — the ONNX embedding adapter and
//! the Markdown chunker — behind the `ai` feature and injects them into the
//! [`Container`](webfang_core::application::container::Container). The SQLite
//! note repository is wired ONLY when the consumer
//! explicitly enables the `persistence` feature (`--features ai,persistence`),
//! keeping the `ai` feature free of database dependencies.
//!
//! Mirrors the semantic-cleaner wiring precedent in `examples/mcp_server.rs`:
//! construction failures degrade gracefully (a warning is logged and the server
//! keeps running) so the affected tools answer with honest feature-gated errors
//! instead of the server failing to boot.
//!
//! When `persistence` is enabled, the note repository reuses the elastic
//! pipeline's SQLite database (resolved via
//! [`resolve_db_path`](webfang_core::infrastructure::autotuning::resolve_db_path)
//! / [`env_db_path`](webfang_core::infrastructure::autotuning::env_db_path),
//! default `~/.webfang/crawl.db`, overridable with `WEBFANG_DB_PATH`). The frozen
//! schema already carries the `notes` and `note_chunks` tables alongside the
//! elastic `chunks` table, so one database serves both pipelines.

use std::sync::Arc;

use webfang_core::application::container::{Container, VaultAiPorts};
use webfang_core::domain::embedding_port::EmbeddingPort;

/// Wire the vault-search AI ports into `container`.
///
/// Injects, in order, through [`inject_vault_ports`](Container::inject_vault_ports)
/// (interior mutability through `&self`, #759): the ONNX
/// [`EmbeddingAdapter`](webfang_ai::EmbeddingAdapter) — assembled from the
/// semantic cleaner's shared erased engine + tokenizer (#1569), so the ONNX
/// model is loaded exactly once — a
/// [`MarkdownChunker`](webfang_ai::MarkdownChunker), and a SQLite-backed
/// [`NoteRepository`](webfang_core::domain::note_repository::NoteRepository).
///
/// The engine argument is type-erased (`Arc<dyn InferenceEngine + Send +
/// Sync>`) so the daemon serves vault-search embeddings from the SAME engine
/// the cleaner built — `Single` or `Pool { N }` alike, per `WEBFANG_AI_ENGINE`.
///
/// The `&self` injection is what makes the lazy MCP AI wiring possible (#759):
/// the container is shared as `Arc<Container>` between the already-serving MCP
/// server and a background warmup task, so the injection happens through the
/// reference without moving or rebuilding the container. Embedding + chunker
/// assembly is infallible (the components are already valid); only the note
/// repository can fail, in which case the embedding port and chunker remain
/// wired and vault search degrades to an honest "not available" error at call
/// time.
pub async fn wire_ai_ports(
    container: &Container,
    engine: Arc<dyn webfang_ai::infrastructure_ai::InferenceEngine + Send + Sync>,
    tokenizer: Arc<webfang_ai::MiniLmTokenizer>,
) {
    // 0. Remote embedding first (#1462) — extracted so this orchestrator
    //    stays under the cognitive-complexity ratchet.
    inject_remote_embedding_first(container).await;

    // 1. Embedding port (ONNX adapter) — shares the cleaner's erased engine +
    //    tokenizer, so this is infallible (no model resolution happens here).
    let adapter = webfang_ai::EmbeddingAdapter::new(engine, tokenizer);
    let dim = adapter.embedding_dim();

    // 2. Note repository (SQLite persistence) — only when the consumer
    //    explicitly enables the `persistence` feature. Without it, vault
    //    search degrades to an honest "not available" error at call time.
    let note_repository: Option<Arc<dyn webfang_core::domain::note_repository::NoteRepository>> = {
        #[cfg(feature = "persistence")]
        {
            match build_note_repository().await {
                Ok(repo) => Some(repo),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "note repository unavailable, vault search persistence disabled"
                    );
                    None
                },
            }
        }
        #[cfg(not(feature = "persistence"))]
        {
            None
        }
    };

    // 3. Text chunker (Markdown segmentation) + everything above, injected in
    //    one shot through the shared container.
    let ports = VaultAiPorts {
        embedding_port: Some(Arc::new(adapter)),
        note_repository,
        text_chunker: Some(Arc::new(webfang_ai::MarkdownChunker::new())),
        ..Default::default()
    };
    container.inject_vault_ports(ports);

    // 4. Summary log — reflect what actually landed in the container.
    if container.note_repository().is_some() {
        tracing::info!(
            dim,
            "vault-search AI ports wired (embedding + chunker + notes)"
        );
    } else {
        tracing::info!(
            dim,
            "vault-search AI ports wired (embedding + chunker); enable `persistence` for note storage"
        );
    }
}

/// Inject the remote embedding adapter BEFORE the local one (#1462).
///
/// When the config-file default embedding slot resolves to
/// `open_ai_compatible`, the probed remote adapter is injected first —
/// `inject_vault_ports` is at-most-once per slot, so the remote wins and
/// the local assembly in [`wire_ai_ports`] degrades to chunker/notes only.
/// No argv reaches the daemon, so only the default slot applies (never
/// `--embedding-provider`). Any failure (no remote configured, credential,
/// probe) degrades to local with a warning — the MCP convention is to keep
/// serving, the opposite of the CLI's fail-closed startup.
async fn inject_remote_embedding_first(container: &Container) {
    if let Some(adapter) = try_remote_embedding_port().await {
        container.inject_vault_ports(VaultAiPorts {
            embedding_port: Some(adapter),
            ..Default::default()
        });
        tracing::info!("vault-search embedding served by remote provider (lazy MCP wiring)");
    }
}

/// Attempt the remote embedding branch of the lazy MCP wiring (#1462).
///
/// Loads the config-file providers and runs the shared embedding preflight
/// against the DEFAULT slot with `offline = false` (the daemon owns no
/// offline mode). Returns the probed adapter, or `None` when local serves
/// (no remote configured, `LocalOnnx` default, or any construction/probe
/// failure — each logged, never fatal).
async fn try_remote_embedding_port(
) -> Option<Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>> {
    let loaded = webfang_core::cli::config::ConfigDefaults::load(
        &webfang_core::cli::config::resolve_config_path(),
    );
    let providers = webfang_core::domain::providers::ProvidersConfig {
        providers: loaded.providers,
    };
    // No argv reaches the daemon: default slot only.
    let opts = webfang_core::application::crawl_options::CrawlOptions::default();
    match webfang_core::cli::llm_wire::build_embedding_provider(&opts, &providers, false).await {
        Ok(Some(adapter)) => Some(adapter),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                error = ?e,
                "remote embedding unavailable, vault search keeps the local adapter"
            );
            None
        },
    }
}

/// Build the SQLite-backed note repository on the elastic pipeline's database.
///
/// Resolves the DB path via the hardware-autotuning convention (CLI > env >
/// `~/.webfang/crawl.db`), opens a WAL-mode pool, and runs the idempotent schema
/// setup (creating the `notes`/`note_chunks` tables if missing).
///
/// Only compiled when the `persistence` feature is explicitly enabled — the
/// consumer opts in with `--features ai,persistence`.
#[cfg(feature = "persistence")]
async fn build_note_repository() -> Result<
    Arc<dyn webfang_core::domain::note_repository::NoteRepository>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    use webfang_core::infrastructure::autotuning::{env_db_path, resolve_db_path};
    use webfang_core::infrastructure::persistence::{
        create_pool, setup_schema, SqliteVectorRepository,
    };

    let db_path = resolve_db_path(None, env_db_path());
    let pool = create_pool(&db_path, 4)?;
    setup_schema(&pool).await?;
    Ok(Arc::new(SqliteVectorRepository::new(pool)))
}

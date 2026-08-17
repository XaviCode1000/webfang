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
/// semantic cleaner's shared inference pool + tokenizer, so the ONNX model is
/// loaded exactly once — a [`MarkdownChunker`](webfang_ai::MarkdownChunker),
/// and a SQLite-backed
/// [`NoteRepository`](webfang_core::domain::note_repository::NoteRepository).
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
    pool: Arc<webfang_ai::InferencePool>,
    tokenizer: Arc<webfang_ai::MiniLmTokenizer>,
) {
    // 1. Embedding port (ONNX adapter) — shares the cleaner's pool + tokenizer,
    //    so this is infallible (no model resolution happens here).
    let adapter = webfang_ai::EmbeddingAdapter::new(pool, tokenizer);
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

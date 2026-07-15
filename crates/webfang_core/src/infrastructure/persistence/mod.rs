//! SQLite persistence layer for the elastic ingestion pipeline (Issue #51).
//!
//! Provides a WAL-mode [`deadpool_sqlite`] connection pool and explicit schema
//! initialization for the `resources` and `chunks` tables. Per-connection
//! pragmas (`journal_mode=WAL`, `synchronous=NORMAL`, `cache_size=-4000`) are
//! applied via a `post_create` pool hook so **every** pooled connection honours
//! the spec's "each connection MUST use WAL-mode pragmas" requirement.

pub mod sqlite;

pub use sqlite::{create_memory_pool, create_pool, setup_schema, SqliteVectorRepository};

//! Infrastructure layer integration tests.
//!
//! Real I/O tests using temp dirs, wiremock servers, and in-memory databases.
//! Each submodule exercises one infrastructure component end-to-end.

mod cookie_bridge;
mod file_saver;
mod jsonl_exporter;
mod session_pool;
mod sitemap_parser;
mod vault_detector;

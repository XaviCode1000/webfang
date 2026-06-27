//! Infrastructure layer — External implementations (HTTP, FS, converters)
//!
//! This layer contains the technical implementations of external concerns:
//! - HTTP client creation
//! - Web scraping (Readability, fallback)
//! - Content conversion (HTML to Markdown, syntax highlighting)
//! - File I/O (saving results, frontmatter generation)
//! - Web crawling (FASE 1)
//! - Export pipeline (JSONL) (FASE 1)
//! - AI-powered semantic cleaning (FASE 1 - feature-gated)
//!
//! Following Clean Architecture: infrastructure depends on domain, not vice versa.

pub mod config;
pub mod converter;
pub mod crawler;
pub mod export;
pub mod http;
pub mod mcp_server;
pub mod observability;
pub mod obsidian;
pub mod output;
pub mod scraper;
pub mod user_agent;

// Elastic ingestion (Issue #51) — hardware autotuning + SQLite persistence.
pub mod autotuning;
pub mod persistence;

#[cfg(feature = "ai")]
pub mod ai;

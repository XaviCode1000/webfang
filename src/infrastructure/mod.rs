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
pub mod downloader;
pub mod error;
pub mod export;
pub mod http;
#[cfg(feature = "mcp")]
pub mod mcp_server;
pub mod network;
pub mod observability;
pub mod obsidian;
pub mod output;
pub mod scraper;
pub mod user_agent;

// Elastic ingestion (Issue #51) — hardware autotuning + SQLite persistence.
pub mod autotuning;
pub mod bridge;
pub mod cpu_pool;
// Dependency-free JSONL vector sink (headless RAG export). Always compiled — no
// heavy deps — so `--output-vectors` works in the lightweight core binary.
pub mod stream;
// SQLite persistence layer — gated behind `persistence` so the lightweight core
// binary ships without any bundled-libsqlite3 dependency (spec R2 / S2.1).
#[cfg(feature = "persistence")]
pub mod persistence;

// Competitive Features Phase 1 — checkpoint + session pool
pub mod checkpoint;
pub mod session;

#[cfg(feature = "ai")]
pub mod ai;

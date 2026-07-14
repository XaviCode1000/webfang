# rust_scraper

**Web scraper de alto rendimiento con arquitectura modular para datasets RAG, crawling inteligente y exportación multi-formato.**

[![CI](https://github.com/XaviCode1000/rust_scraper/actions/workflows/ci.yml/badge.svg)](https://github.com/XaviCode1000/rust_scraper/actions)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88+-orange)](https://rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-1%2C337+-green)](#testing)
[![Miri](https://img.shields.io/badge/Miri-domain%2Bcore-passing-blue)](#memory-safety)

[Quick Start](#-quick-start) · [Architecture](#-architecture) · [Features](#-features) · [CLI Reference](#cli-reference) · [MCP Server](#mcp-server) · [Developer Guide](#developer-guide)

---

## Quick Start

```bash
# Install
git clone https://github.com/XaviCode1000/rust_scraper.git
cd rust_scraper
cargo install --path crates/rust_scraper_cli

# Scrape a single page
rust_scraper --url https://example.com

# Crawl an entire site
rust_scraper --url https://example.com --use-sitemap --max-pages 50

# Export for RAG pipelines
rust_scraper --url https://example.com --export-format jsonl --clean-ai
```

Output is saved to `output/` as Markdown by default.

---

## Architecture

Clean Architecture with enforced dependency direction across 5 workspace crates:

```
rust_scraper_cli ──→ rust_scraper_tui ──→ rust_scraper_core ←── rust_scraper_ai
rust_scraper_cli ──→ rust_scraper_mcp ──→ rust_scraper_core
rust_scraper_cli ──────────────────────→ rust_scraper_core
```

| Crate | Purpose | Key Dependencies |
|-------|---------|-----------------|
| `rust_scraper_core` | Domain, application, infrastructure | wreq, tokio, scraper, lol_html |
| `rust_scraper_ai` | ONNX semantic cleaning | tract-onnx |
| `rust_scraper_tui` | Terminal UI | ratatui |
| `rust_scraper_mcp` | MCP server for AI agents | rmcp |
| `rust_scraper_cli` | Binary entry point + CLI parsing | clap |

**Dependency direction:** CLI → {TUI, MCP, AI} → Core. No circular dependencies.

---

## Features

| Feature | Description |
|---------|-------------|
| **Content extraction** | Readability-based extraction — strips menus, ads, sidebars |
| **AI semantic cleaning** | ONNX embeddings filter irrelevant content (feature `ai`) |
| **Multi-format export** | Markdown, JSON, JSONL (RAG), Vector (embeddings) |
| **Obsidian integration** | Direct vault saves with wiki-links and metadata |
| **Sitemap discovery** | Auto-discovers all pages via robots.txt + sitemap.xml |
| **Asset download** | Images and documents (PDF, DOCX, XLSX) |
| **WAF detection** | Detects Cloudflare, reCAPTCHA, hCaptcha, DataDome |
| **MCP server** | 34+ tools for AI agent integration |
| **Rate limiting** | Configurable with Retry-After respect |
| **Resume** | Continues interrupted crawls with `--resume` |
| **TLS fingerprinting** | wreq impersonates real browsers to bypass WAFs |
| **TUI selector** | Interactive URL selection with ratatui |

---

## CLI Reference

### Basic usage

```bash
# Single page
rust_scraper --url https://example.com

# With selector (CSS)
rust_scraper --url https://example.com --selector "article h1"

# Multi-page crawl
rust_scraper --url https://example.com --max-pages 50 --concurrency 4

# Sitemap-based crawl
rust_scraper --url https://example.com --use-sitemap --sitemap-url https://example.com/sitemap.xml
```

### Output formats

```bash
rust_scraper --url https://example.com --format markdown    # Default
rust_scraper --url https://example.com --format json
rust_scraper --url https://example.com --export-format jsonl
rust_scraper --url https://example.com --export-format vector
```

### AI cleaning

```bash
rust_scraper --url https://example.com --clean-ai --export-format jsonl
```

### Obsidian

```bash
rust_scraper --url https://example.com --obsidian-wiki-links --quick-save
```

### Control

```bash
rust_scraper --url https://example.com --max-pages 100 --delay-ms 1000 --timeout-secs 30
rust_scraper --url https://example.com --download-images --download-documents
rust_scraper --url https://example.com --dry-run
rust_scraper --url https://example.com --quiet
```

### Full reference

```bash
rust_scraper --help
```

---

## MCP Server

The MCP server provides **34+ tools** for AI agent integration:

```bash
# stdio mode (for OpenCode, Claude Desktop, Cursor)
cargo run -p rust_scraper_mcp --example mcp_server --quiet

# HTTP mode
cargo run -p rust_scraper_mcp --example mcp_server
```

| Category | Tools |
|----------|-------|
| Scraping | `scrape_url`, `scrape_batch`, `crawl_site`, `crawl_with_sitemap` |
| Content | `clean_html`, `extract_links`, `convert_html_to_markdown` |
| WAF | `detect_waf`, `verify_waf_integrity`, `list_waf_providers` |
| Export | `export_file`, `export_jsonl`, `export_vector` |
| Obsidian | `detect_obsidian_vault`, `search_obsidian`, `build_obsidian_uri` |
| URLs | `validate_url`, `normalize_url`, `is_internal_link` |

---

## Configuration

Config file: `~/.config/rust_scraper/config.toml`

```toml
format = "markdown"
max_pages = 50
delay_ms = 500
use_sitemap = true
```

CLI arguments override config file values.

---

## Build Features

| Feature | Activates | Install |
|---------|-----------|---------|
| `default` | images + documents | `cargo install --path crates/rust_scraper_cli` |
| `ai` | Semantic cleaning with ONNX (~90MB model) | `--features ai` |
| `ui` | Interactive TUI with ratatui | `--features ui` |
| `mcp` | MCP server for AI agents | `--features mcp` |
| `persistence` | SQLite checkpoint store | `--features persistence` |
| `otel` | OpenTelemetry observability | `--features otel` |
| `console` | Tokio console (debugging) | `--features console` |

---

## Testing

```bash
# Run all tests
cargo nextest run --workspace

# Run with coverage
cargo llvm-cov --all-features

# Run Miri (memory safety verification)
cargo +nightly miri test --lib
```

**Test suite:** 1,337 tests across unit, integration, and behavioral layers.

**Miri status:** Domain + Core layers verified for Undefined Behavior. Infrastructure layer partially verified (servo_arc/btls FFI limitations documented).

---

## Developer Guide

### Workspace structure

```
rust_scraper/
├── crates/
│   ├── rust_scraper_core/     # Domain + application + infrastructure
│   ├── rust_scraper_ai/       # AI/ONNX inference
│   ├── rust_scraper_tui/      # Terminal UI
│   ├── rust_scraper_mcp/      # MCP server
│   └── rust_scraper_cli/      # Binary entry point
├── Cargo.toml                 # Workspace manifest
└── .github/workflows/ci.yml  # CI pipeline
```

### Development commands

```bash
# Quick verification (check + clippy + fmt)
cargo check --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check

# Run tests
cargo nextest run --workspace

# Build release
cargo build --release -p rust_scraper_cli

# Build with all features
cargo build --release -p rust_scraper_cli --features full

# Re-index GitNexus (code intelligence)
gitnexus analyze --index-only --skip-agents-md
```

### Architecture rules

- **Dependency direction:** CLI → {TUI, MCP, AI} → Core (never reverse)
- **Port/Adapter pattern:** Domain defines traits, Infrastructure implements them
- **Error types:** DomainError, InfraError, AppError → ScraperError (dual wrapping)
- **User-facing errors:** Spanish. Internal logs: English.

**Stack:** Rust 1.88 · Tokio · wreq (TLS fingerprint) · ratatui · scraper 0.27 · lol_html · tract-onnx

---

## Documentation

| Resource | Covers |
|----------|--------|
| [AGENTS.md](AGENTS.md) | AI agent instructions, GitNexus integration |
| [Wiki](https://github.com/XaviCode1000/rust_scraper/wiki) | Architecture, API reference, guides |
| `rust_scraper --help` | Full CLI reference |

---

## Contributing

1. Fork → branch `feature/name` → commit → PR
2. Tests must pass: `cargo nextest run --workspace`
3. Conventional Commits: `feat:`, `fix:`, `refactor:`, `ci:`, `docs:`
4. Read [AGENTS.md](AGENTS.md) for architecture rules and GitNexus usage

---

## License

MIT OR Apache-2.0

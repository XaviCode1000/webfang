# webfang

**Web scraper de alto rendimiento con arquitectura modular para datasets RAG, crawling inteligente y exportación multi-formato.**

[![CI](https://github.com/XaviCode1000/webfang/actions/workflows/ci.yml/badge.svg)](https://github.com/XaviCode1000/webfang/actions)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88+-orange)](https://rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-1%2C337+-green)](#testing)
[![Miri](https://img.shields.io/badge/Miri-domain%2Bcore-passing-blue)](#memory-safety)

[Quick Start](#-quick-start) · [Architecture](#-architecture) · [Features](#-features) · [CLI Reference](#cli-reference) · [MCP Server](#mcp-server) · [Developer Guide](#developer-guide)

---

## Quick Start

```bash
# Install
git clone https://github.com/XaviCode1000/webfang.git
cd webfang
cargo install --path crates/webfang_cli

# Scrape a single page
webfang --url https://example.com

# Crawl an entire site
webfang --url https://example.com --use-sitemap --max-pages 50

# Export for RAG pipelines
webfang --url https://example.com --export-format jsonl --clean-ai
```

Output is saved to `output/` as Markdown by default.

---

## Architecture

Clean Architecture with enforced dependency direction across 5 workspace crates:

```
webfang_cli ──→ webfang_tui ──→ webfang_core ←── webfang_ai
webfang_cli ──→ webfang_mcp ──→ webfang_core
webfang_cli ──────────────────────→ webfang_core
```

| Crate | Purpose | Key Dependencies |
|-------|---------|-----------------|
| `webfang_core` | Domain, application, infrastructure | wreq, tokio, scraper, lol_html |
| `webfang_ai` | ONNX semantic cleaning | tract-onnx |
| `webfang_tui` | Terminal UI | ratatui |
| `webfang_mcp` | MCP server for AI agents | rmcp |
| `webfang_cli` | Binary entry point + CLI parsing | clap |

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
| **MCP server** | 34 tools for AI agent integration |
| **Rate limiting** | Configurable with Retry-After respect |
| **Resume** | Continues interrupted crawls with `--resume` |
| **TLS fingerprinting** | wreq impersonates real browsers to bypass WAFs |
| **TUI selector** | Interactive URL selection with ratatui |

---

## CLI Reference

### Basic usage

```bash
# Single page
webfang --url https://example.com

# With selector (CSS)
webfang --url https://example.com --selector "article h1"

# Multi-page crawl
webfang --url https://example.com --max-pages 50 --concurrency 4

# Sitemap-based crawl
webfang --url https://example.com --use-sitemap --sitemap-url https://example.com/sitemap.xml
```

### Output formats

```bash
webfang --url https://example.com --format markdown    # Default
webfang --url https://example.com --format json
webfang --url https://example.com --export-format jsonl
webfang --url https://example.com --export-format vector
```

### AI cleaning

```bash
webfang --url https://example.com --clean-ai --export-format jsonl
```

### Obsidian

```bash
webfang --url https://example.com --obsidian-wiki-links --quick-save
```

### Control

```bash
webfang --url https://example.com --max-pages 100 --delay-ms 1000 --timeout-secs 30
webfang --url https://example.com --download-images --download-documents
webfang --url https://example.com --dry-run
webfang --url https://example.com --quiet
```

### Retry & backoff

```bash
webfang --url https://example.com --max-retries 5 --backoff-base-ms 2000 --backoff-max-ms 30000
```

### Resume interrupted crawls

```bash
webfang --url https://example.com --resume
webfang --url https://example.com --resume --state-dir /tmp/webfang-state
```

### Batch mode

```bash
# Read URLs from stdin (one per line)
webfang --batch < urls.txt

# Read URLs from a file
webfang --batch-file urls.txt --batch-concurrency 10
```

### Elastic ingestion pipeline

```bash
webfang --url https://example.com --elastic --ram-budget 4GB --cpu-cores 4
webfang --url https://example.com --elastic --db-path ./webfang.db
webfang --url https://example.com --output-vectors vectors.jsonl
```

### TLS/HTTP2 profile

```bash
webfang --url https://example.com --h2-profile Chrome145
```

### Full reference

```bash
webfang --help
```

---

## MCP Server

The MCP server provides **34 tools** for AI agent integration:

```bash
# stdio mode (for OpenCode, Claude Desktop, Cursor)
cargo run -p webfang_mcp --example mcp_server --quiet

# HTTP mode
cargo run -p webfang_mcp --example mcp_server
```

| Category | Tools |
|----------|-------|
| Scraping (8) | `scrape_url`, `scrape_with_options`, `scrape_batch`, `crawl_site`, `crawl_with_sitemap`, `discover_urls`, `discover_sitemap`, `detect_spa` |
| Content (7) | `clean_html`, `convert_html_to_markdown`, `extract_links`, `highlight_code_blocks`, `convert_wiki_links`, `generate_frontmatter`, `generate_rich_metadata` |
| Export (4) | `export_file`, `export_jsonl`, `export_vector`, `process_export_pipeline` |
| URL Utils (6) | `validate_url`, `extract_domain`, `normalize_url`, `match_url_pattern`, `is_internal_link`, `url_to_file_path` |
| Security (4) | `detect_waf`, `verify_waf_integrity`, `list_waf_providers`, `get_scrape_metrics` |
| Obsidian (4) | `detect_obsidian_vault`, `build_obsidian_uri`, `open_in_obsidian`, `search_obsidian` |
| Assets (1) | `download_assets` |

---

## Configuration

Config file: `~/.config/webfang/config.toml`

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
| `default` | images + documents | `cargo install --path crates/webfang_cli` |
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
webfang/
├── crates/
│   ├── webfang_core/     # Domain + application + infrastructure
│   ├── webfang_ai/       # AI/ONNX inference
│   ├── webfang_tui/      # Terminal UI
│   ├── webfang_mcp/      # MCP server
│   └── webfang_cli/      # Binary entry point
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
cargo build --release -p webfang_cli

# Build with all features
cargo build --release -p webfang_cli --features full

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
| [Wiki](https://github.com/XaviCode1000/webfang/wiki) | Architecture, API reference, guides |
| `webfang --help` | Full CLI reference |

---

## Contributing

1. Fork → branch `feature/name` → commit → PR
2. Tests must pass: `cargo nextest run --workspace`
3. Conventional Commits: `feat:`, `fix:`, `refactor:`, `ci:`, `docs:`
4. Read [AGENTS.md](AGENTS.md) for architecture rules and GitNexus usage

---

## License

MIT OR Apache-2.0

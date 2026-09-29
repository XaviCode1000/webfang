# webfang

**Web scraper de alto rendimiento con arquitectura modular para datasets RAG, crawling inteligente y exportación multi-formato.**

[![CI](https://github.com/XaviCode1000/webfang/actions/workflows/ci.yml/badge.svg)](https://github.com/XaviCode1000/webfang/actions)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88+-orange)](https://rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-1%2C337+-green)](#testing)
[![Miri](https://img.shields.io/badge/Miri-domain%2Bcore-passing-blue)](#memory-safety)

[Installation](#installation) · [Quick Start](#-quick-start) · [Uninstall](#uninstall) · [Architecture](#-architecture) · [Features](#-features) · [CLI Reference](#cli-reference) · [MCP Server](#mcp-server) · [Developer Guide](#developer-guide)

---

## Installation

WebFang is a **binaries-only** distribution. Nothing is published to crates.io,
so `cargo install webfang` cannot work. Every release publishes four archives
plus a checksum manifest, on [GitHub Releases](https://github.com/XaviCode1000/webfang/releases/latest):

| Platform | Asset | Format |
| :--- | :--- | :--- |
| Linux x86_64 | `webfang-x86_64-unknown-linux-gnu.tar.gz` | `.tar.gz` |
| Linux ARM64 | `webfang-aarch64-unknown-linux-gnu.tar.gz` | `.tar.gz` |
| macOS Apple Silicon | `webfang-aarch64-apple-darwin.tar.gz` | `.tar.gz` |
| Windows x86_64 | `webfang-x86_64-pc-windows-msvc.zip` | `.zip` |
| Any | `SHA256SUMS.txt` | checksums |

**There is no Intel macOS artifact** — ONNX Runtime dropped x64 macOS, so the
`ai` feature cannot link there. Each archive holds a bare binary: verify the
checksum, then `chmod +x` and put it on your `PATH`.

<details>
<summary>Linux, end to end</summary>

```bash
VERSION=v2.4.0   # ← the tag from the releases page
curl -fLO "https://github.com/XaviCode1000/webfang/releases/download/${VERSION}/webfang-x86_64-unknown-linux-gnu.tar.gz"
curl -fLO "https://github.com/XaviCode1000/webfang/releases/download/${VERSION}/SHA256SUMS.txt"
sha256sum -c SHA256SUMS.txt --ignore-missing   # verify BEFORE extracting
tar xzf webfang-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 0755 webfang /usr/local/bin/webfang
webfang --version
```

</details>

**Full verified commands for all four platforms, per-shell checksum commands
(`sha256sum` does not exist on stock macOS or in PowerShell), platform
requirements (glibc floor, no musl, Windows VC++ runtime, macOS Gatekeeper),
and every on-disk root an install leaves behind:
[docs/src/installation.md](docs/src/installation.md).**

<details>
<summary>Install from source (contributors, not an end-user route)</summary>

Requires Rust 1.88, a C/C++ compiler, and **`cmake`** — `wreq` → `boring2` →
`boring-sys2` compiles BoringSSL from C++ on first build, and without
`cmake` the failure surfaces deep in a build script with an error that says
nothing about the real cause.

```bash
git clone https://github.com/XaviCode1000/webfang.git
cd webfang
cmake --version
cargo build --release --locked --features "ai mcp" -p webfang_cli
./target/release/webfang --version
```

</details>

---

## Quick Start

```bash
# Scrape a single page (after the Installation step above)
webfang --url https://example.com

# Crawl an entire site
webfang --url https://example.com --use-sitemap --max-pages 50

# Export for RAG pipelines
webfang --url https://example.com --export-format jsonl --clean-ai
```

Output is saved to `output/` as Markdown by default.

---

## Architecture

Clean Architecture with enforced dependency direction across 4 workspace crates (plus `webfang_test_utils`, a shared dev/test-support crate):

```
webfang_cli ──→ webfang_ai ───→ webfang_core
webfang_cli ──→ webfang_mcp ──→ webfang_core
webfang_cli ──────────────────────→ webfang_core
```

| Crate | Purpose | Key Dependencies |
|-------|---------|-----------------|
| `webfang_core` | Domain, application, infrastructure | wreq, tokio, scraper, lol_html |
| `webfang_ai` | ONNX semantic cleaning | ort |
| `webfang_mcp` | MCP server for AI agents | rmcp |
| `webfang_cli` | Binary entry point + CLI parsing | clap |

**Dependency direction:** CLI → {MCP, AI} → Core. No circular dependencies.

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
| **MCP server** | 36 tools for AI agent integration |
| **Rate limiting** | Configurable with Retry-After respect |
| **Resume** | Continues interrupted crawls with `--resume` |
| **TLS fingerprinting** | wreq impersonates real browsers to bypass WAFs |

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

### Debugging & tracing

Every run can emit a JSONL trace (no external collector needed). Each
operation shares a `trace_id`; each page gets its own `span_id`.

```bash
# Emit a trace + verbose logs
webfang --url https://example.com --trace-file debug.jsonl -vvv

# Reconstruct one operation, list errors, find the slowest spans
scripts/analyze-trace.sh debug.jsonl trace <trace_id>
scripts/analyze-trace.sh debug.jsonl errors
scripts/analyze-trace.sh debug.jsonl slow 20
```

See [docs/src/debugging.md](docs/src/debugging.md) for the full query cookbook and
[docs/src/troubleshooting.md](docs/src/troubleshooting.md) for common problems.

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

### DOM Pre-pruning

Before passing HTML to Readability, invisible elements are automatically removed:

```bash
# Pre-pruning is ENABLED by default
webfang --url https://example.com

# Explicitly enable (same as default)
webfang --url https://example.com --dom-preprune

# Disable pre-pruning
webfang --url https://example.com --dom-preprune=false

# Via environment variable
WEBFANG_DOM_PREPRUNE=false webfang --url https://example.com
```

Pre-pruning removes:
- Elements with `display: none` or `visibility: hidden`
- Empty wrapper elements (div, span, p, section, article) without attributes

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

The MCP server provides **36 tools** across **9 categories** for AI agent integration.

> **Note:** the `webfang` CLI does **not** expose a `--mcp` flag (running `webfang --mcp` fails with `error: unexpected argument '--mcp' found`). The MCP server lives in the `webfang_mcp` crate and is started via its examples below.

```bash
# HTTP mode (Streamable HTTP at http://127.0.0.1:8080/mcp)
cargo run -p webfang_mcp --example mcp_server --features "mcp ai persistence"

# stdio mode (for OpenCode, Claude Desktop, Cursor; JSON-RPC over stdin/stdout)
cargo run -p webfang_mcp --example mcp_server_stdio --features "mcp ai persistence"
```

| Category | Tools |
|----------|-------|
| Scraping (8) | `scrape_url`, `scrape_with_options`, `scrape_batch`, `crawl_site`, `crawl_with_sitemap`, `discover_urls`, `discover_sitemap`, `detect_spa` |
| Content (7) | `clean_html`, `convert_html_to_markdown`, `extract_links`, `highlight_code_blocks`, `convert_wiki_links`, `generate_frontmatter`, `generate_rich_metadata` |
| Export (4) | `export_file`, `export_jsonl`, `export_vector`, `process_export_pipeline` |
| URL Utils (6) | `validate_url`, `extract_domain`, `normalize_url`, `match_url_pattern`, `is_internal_link`, `url_to_file_path` |
| Security (4) | `detect_waf`, `verify_waf_integrity`, `list_waf_providers`, `get_scrape_metrics` |
| Obsidian (3) | `detect_obsidian_vault`, `build_obsidian_uri`, `open_in_obsidian` |
| AI (2) | `semantic_cleaner`, `search_obsidian` |
| Axtree (1) | `get_accessibility_snapshot` |
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

Features are compiled in at **build** time. A release binary is built with
`--features "ai mcp"`, so only `default`, `ai`, and `mcp` are present in it —
everything else needs a [source build](#installation).

| Feature | Activates | In the release binary? |
|---------|-----------|--------|
| `default` (`images` + `documents`) | Image and document extraction | ✅ Yes |
| `ai` | Semantic cleaning with ONNX (~390 MB model) | ✅ Yes |
| `mcp` | The `webfang_mcp` crate's MCP server | ⚠️ The CLI's `mcp` feature gates no shipped code — the crate itself is not in the release. See [MCP Server](#-mcp-server) |
| `persistence` | SQLite checkpoint store | ❌ No — `--features persistence` from source |
| `chromium` | Headless Chrome for `--js-strategy full` (rejected at preflight without it) | ❌ No — `--features chromium` from source |
| `adaptive-selectors` | Adaptive selector learning | ❌ No — from source |
| `console` | Tokio console (debugging) | ❌ No — `--features console` from source |

---

## Uninstall

WebFang removes nothing for you. The binary is one file; everything else it
touched is yours to delete.

```bash
# 1. The binary
sudo rm /usr/local/bin/webfang            # or ~/.local/bin/webfang

# 2. Cache + state + user-agent cache (NOT the model cache)
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/webfang"

# 3. The ONNX model cache (~372 MB default) — check you did not relocate it
rm -rf "${HF_HOME:-$HOME/.cache/huggingface}"

# 4. Your own output — NOT deleted by webfang, and not backed up
ls output/
```

`webfang/state/<domain>.json.lock` is a **permanent sentinel**; its survival
after an uninstall is correct behaviour, not a failed removal.

> **The model cache is not relocatable by any `WEBFANG_*` variable** — it is
> HuggingFace's `HF_HOME`. Check it before deleting, or you will delete the
> wrong directory and recover nothing. Windows/macOS paths, the full table of
> what each run creates, and the per-platform commands:
> [docs/src/installation.md#uninstall](docs/src/installation.md#uninstall).

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

**Concurrency verification (DoD #507):** Miri valida las unidades lock-free puras (AtomicUsize counters, mpsc channel); la concurrencia de alto nivel se cubre con tests de integración que ejercitan CancellationToken/shutdown y backpressure (abort de stragglers, canales acotados).

---

## Developer Guide

### Workspace structure

```
webfang/
├── crates/
│   ├── webfang_core/     # Domain + application + infrastructure
│   ├── webfang_ai/       # AI/ONNX inference
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

# Re-index CodeDB (code intelligence)
codedb index .
```

### Architecture rules

- **Dependency direction:** CLI → {MCP, AI} → Core (never reverse)
- **Port/Adapter pattern:** Domain defines traits, Infrastructure implements them
- **Error types:** DomainError, InfraError → ScraperError (dual wrapping)
- **User-facing errors:** Spanish. Internal logs: English.

**Stack:** Rust 1.88 · Tokio · wreq (TLS fingerprint) · scraper 0.27 · lol_html · ort

---

## Documentation

| Resource | Covers |
|----------|--------|
| [AGENTS.md](AGENTS.md) | AI agent instructions, code intelligence integration |
| [docs/src/installation.md](docs/src/installation.md) | Verified install + uninstall for the binaries-only release: asset names, per-shell checksums, platform floors |
| [docs/src/debugging.md](docs/src/debugging.md) | Tracing, correlation IDs, `jq` query cookbook (`scripts/analyze-trace.sh`) |
| [docs/src/troubleshooting.md](docs/src/troubleshooting.md) | Common failures: slow crawls, silent errors, WAF blocks, local/internal targets refused by the SSRF guard |
| [docs/ssrf-layers.md](docs/ssrf-layers.md) | Which SSRF layer blocks what, and which `WEBFANG_*` variable lifts which layer |
| [Wiki](https://github.com/XaviCode1000/webfang/wiki) | Architecture, API reference, guides |
| `webfang --help` | Full CLI reference |

---

## Contributing

1. Fork → branch `feature/name` → commit → PR
2. Tests must pass: `cargo nextest run --workspace`
3. Conventional Commits: `feat:`, `fix:`, `refactor:`, `ci:`, `docs:`
4. Read [AGENTS.md](AGENTS.md) for architecture rules and tooling

---

## License

MIT OR Apache-2.0

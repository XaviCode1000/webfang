# 🕷️ Rust Scraper

**Production-ready web scraper with Clean Architecture, TUI selector, and AI-powered semantic cleaning.**

[![Build Status](https://github.com/XaviCode1000/rust-scraper/actions/workflows/ci.yml/badge.svg)](https://github.com/XaviCode1000/rust-scraper/actions)
[![Tests](https://img.shields.io/badge/tests-361%20passing-brightgreen)](https://github.com/XaviCode1000/rust-scraper)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange)](https://www.rust-lang.org)
[![Version](https://img.shields.io/badge/version-1.1.0-blue)](https://github.com/XaviCode1000/rust-scraper/releases)

---

## 📖 Table of Contents

- [Features](#-features)
- [Installation](#-installation)
- [Usage](#-usage)
- [Testing](#-testing)
- [Architecture](#-architecture)
- [Known Limitations](#-known-limitations-spajs-rendered-sites)
- [Documentation](#-documentation)
- [Development](#-development)
- [Bug Fixes](#-bug-fixes)
- [Contributing](#-contributing)
- [License](#-license)

---

## ✨ Features

### 🚀 Core (v1.0.0)

- **Async Web Scraping** — Multi-threaded with Tokio runtime, bounded concurrency
- **Sitemap Support** — Zero-allocation streaming parser (`quick-xml`)
  - Gzip decompression (`.xml.gz`) via `async-compression`
  - Sitemap index recursion (max depth 3)
  - Auto-discovery from `robots.txt`
- **TUI Interactive Selector** — Ratatui + crossterm URL picker
  - Checkbox selection (`[✅]` / `[⬜]`)
  - Keyboard navigation (↑↓, Space, Enter)
  - Confirmation mode (Y/N) before download
- **RAG Export Pipeline** — JSONL and Vector formats optimized for Retrieval-Augmented Generation
  - State management with resume capability
  - Atomic saves (write to tmp + rename)
  - Compatible with Qdrant, Weaviate, Pinecone, LangChain
  - **Vector Export** — JSON format with metadata header, embeddings support, cosine similarity

### 🧠 AI-Powered (v1.0.5+)

- **Semantic Cleaning** — Local SLM inference (100% privacy, no API calls)
  - 87% accuracy vs 13% fixed-size chunking
  - AVX2 SIMD acceleration (4-8x speedup on CachyOS)
  - **✅ Embeddings Preservation Bug Fixed** — See [Bug Fixes](#-bug-fixes)
  - See [`docs/AI-SEMANTIC-CLEANING.md`](docs/AI-SEMANTIC-CLEANING.md)

### 🏗️ Architecture

- **Clean Architecture** — 4 layers: Domain → Application → Infrastructure → Adapters
- **Error Handling** — `thiserror` for libraries, `anyhow` for applications
- **Dependency Injection** — HTTP client, user agents, concurrency config
- **Type-Safe APIs** — Newtypes for IDs, validated types at boundaries

### ⚡ Performance

- **True Streaming** — Constant ~8KB RAM usage, no OOM risks
- **Zero-Allocation Parsing** — `quick-xml` for sitemaps
- **LazyLock Cache** — Syntax highlighting (2-10ms → ~0.01ms)
- **Bounded Concurrency** — Configurable parallel downloads (HDD-aware defaults)
- **Hardware-Aware** — Auto-detects CPU cores, adjusts concurrency accordingly

### 📝 Obsidian Integration (v1.1.0+)

- **Obsidian-compatible Markdown** — Wiki-links, relative asset paths, tags in frontmatter
- **Vault auto-detect** — Automatic vault discovery via CLI, env var, config, or filesystem scan
- **Quick-save mode** — `--obsidian --quick-save` for frictionless URL-to-vault workflow
- **Rich metadata** — `readingTime`, `language`, `wordCount`, `contentType`, `status` for Dataview queries
- **Obsidian URI** — Open saved notes directly in Obsidian after scraping
- **See** [`docs/OBSIDIAN.md`](docs/OBSIDIAN.md) for complete documentation

### 🎨 CLI UX (v1.1.0+)

- **Sysexits Exit Codes** — Proper exit codes (0, 64, 69, 74, 76, 78) for scripting
- **Shell Completions** — Auto-generated for bash, fish, zsh, elvish, powershell
- **Config File** — `~/.config/rust-scraper/config.toml` with TOML defaults
- **Dry-Run Mode** — `--dry-run` to preview URLs without scraping
- **Quiet Mode** — `--quiet` for clean scripting/pipe usage
- **NO_COLOR Support** — Respects `NO_COLOR` env var (emoji → ASCII)
- **Progress Bars** — `indicatif` spinner for discovery, bar for scraping
- **Pre-flight Check** — HEAD request to fail fast on DNS errors
- **Build Metadata** — `built` crate embeds version, profile, target info

### 🔒 Security

- **SSRF Prevention** — URL host comparison (not string contains)
- **Windows Safe** — Reserved names blocked (`CON` → `CON_safe`)
- **WAF Bypass Prevention** — Chrome 131+ UAs with TTL caching
- **RFC 3986 URLs** — `url::Url::parse()` validation
- **Input Validation** — All user input validated at boundaries

---

## 📦 Installation

### From Source

```bash
git clone https://github.com/XaviCode1000/rust-scraper.git
cd rust-scraper
cargo build --release
```

**Binary location:** `target/release/rust_scraper`

### Requirements

- **Rust:** 1.88+ (MSRV)
- **Cargo:** 1.88+

### Feature Flags

| Feature | Description | Dependencies |
|---------|-------------|--------------|
| `images` | Enable image downloading | `mimetype-detector` |
| `documents` | Enable document downloading | `mimetype-detector` |
| `full` | All features except AI | `images`, `documents` |
| `ai` | AI-powered semantic cleaning | `tract-onnx`, `tokenizers`, `hf-hub`, `ort` |

**Build with AI features:**

```bash
cargo build --release --features ai
```

---

## 🚀 Usage

### Interactive Mode (TUI) — Recommended for Beginners

The TUI lets you discover URLs, select which ones to scrape, and confirm before downloading:

```bash
# Launch interactive URL selector
./target/release/rust_scraper --url https://example.com --interactive

# With sitemap discovery
./target/release/rust_scraper --url https://example.com \
  --interactive \
  --use-sitemap
```

#### TUI Controls

| Key | Action |
|-----|--------|
| `↑` / `↓` | Navigate URLs |
| `Space` | Toggle selection |
| `A` | Select all |
| `D` | Deselect all |
| `Enter` | Confirm download |
| `Y` / `N` | Final confirmation |
| `q` | Quit |

### Basic (Headless Mode)

For scripting and automation:

```bash
# Scrape all URLs from a website
./target/release/rust_scraper --url https://example.com

# With sitemap (auto-discovers from robots.txt)
./target/release/rust_scraper --url https://example.com --use-sitemap

# Explicit sitemap URL
./target/release/rust_scraper --url https://example.com \
  --use-sitemap \
  --sitemap-url https://example.com/sitemap.xml.gz
```

### Advanced Options

```bash
# Full example with all options
./target/release/rust_scraper \
  --url https://example.com \
  --output ./output \
  --format markdown \
  --download-images \
  --download-documents \
  --use-sitemap \
  --concurrency 5 \
  --delay-ms 1000 \
  --max-pages 100 \
  --verbose

# Hardware-aware concurrency (auto-detects CPU)
./target/release/rust_scraper \
  --url https://example.com \
  --concurrency auto
```

### AI-Powered Semantic Cleaning (v1.0.5+)

```bash
# Enable AI semantic cleaning
./target/release/rust_scraper \
  --url https://example.com \
  --clean-ai \
  --ai-threshold 0.3 \
  --export-format jsonl

# Custom AI model (advanced)
./target/release/rust_scraper \
  --url https://example.com \
  --clean-ai \
  --ai-model sentence-transformers/all-MiniLM-L6-v2
```

**Requirements:** Compile with `--features ai`

### Obsidian Integration (v1.1.0+)

```bash
# Quick-save to detected vault (no TUI, no confirmation)
./target/release/rust_scraper --url https://example.com/article --obsidian-wiki-links --obsidian-rich-metadata --quick-save

# With explicit vault path
./target/release/rust_scraper --url https://example.com/article \
  --vault ~/Obsidian/MyVault \
  --obsidian-wiki-links \
  --obsidian-tags "rust,web,scraping" \
  --obsidian-relative-assets \
  --obsidian-rich-metadata

# Set vault via environment variable (persistent)
export OBSIDIAN_VAULT=~/Obsidian/MyKnowledge
./target/release/rust_scraper --url https://example.com/article --obsidian-wiki-links --quick-save
```

**Quick-save behavior:**
- Detects vault automatically (CLI > env > config > auto-scan)
- Saves to `{vault}/_inbox/YYYY-MM-DD-slug.md`
- Opens note in Obsidian if running (Linux)
- Falls back to `./output/` if no vault found

## RAG Export Pipeline (JSONL and Vector Format)

Export content in JSON Lines format, optimized for RAG (Retrieval-Augmented Generation) pipelines, or in Vector JSON format for vector database ingestion.

```bash
# Export to JSONL (one JSON object per line)
./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data

# Export to Vector JSON with embeddings (after AI semantic cleaning)
./target/release/rust_scraper \
  --url https://example.com \
  --export-format vector \
  --clean-ai \
  --output ./vector_data

# Resume interrupted scraping (skip already processed URLs)
./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data \
  --resume

# Custom state directory (isolate state per project)
./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data \
  --state-dir ./state \
  --resume
```

#### JSONL Schema

Each line is a valid JSON object:

```json
{
  "id": "uuid-v4",
  "url": "https://example.com/page",
  "title": "Page Title",
  "content": "Extracted content...",
  "metadata": {
    "domain": "example.com",
    "excerpt": "Meta description or excerpt"
  },
  "timestamp": "2026-03-11T10:00:00Z"
}
```

#### State Management

- **Location:** `~/.cache/rust-scraper/state/<domain>.json`
- **Tracks:** Processed URLs, timestamps, status
- **Atomic saves:** Write to tmp + rename (crash-safe)
- **Resume mode:** `--resume` flag enables state tracking

#### RAG Integration

JSONL format is compatible with:

```python
# Example: Load JSONL with LangChain
from langchain.document_loaders import JSONLoader

loader = JSONLoader(
    file_path='./rag_data/export.jsonl',
    jq_schema='.content',
    text_content=False
)
documents = loader.load()
```

### Get Help

```bash
./target/release/rust_scraper --help
```

---

## 🧪 Testing

### Test Commands (Recommended: nextest)

```bash
# Nextest (4x faster than cargo test) ✅ RECOMMENDED
cargo nextest run

# Run only failed tests
cargo nextest run --failed

# Run ignored tests (real sites integration tests)
cargo nextest run --run-ignored ignored-only

# Traditional (slower)
cargo test

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test test_validate_and_parse_url

# Run AI integration tests (requires --features ai)
cargo test --features ai --test ai_integration -- --test-threads=2

# Run library tests only
cargo test --lib
```

### Test Results

| Test Suite | Count | Status |
|------------|-------|--------|
| **Library Tests** | 252 | ✅ Passing |
| **Total** | **252** | ✅ **All Passing** |

### Linting

```bash
# Clippy with warnings as errors ✅ RECOMMENDED
cargo clippy -- -D warnings

# Check formatting
cargo fmt --all -- --check

# Run all checks
cargo clippy --all-targets --all-features -- -D warnings
```

**Note:** AI tests require `--features ai` and run with `--test-threads=2` for stability.

---

## 🏗️ Architecture

### Clean Architecture Layers

```
┌─────────────────────────────────────────┐
│  Adapters (TUI, CLI, Detectors)         │ ← External interfaces
├─────────────────────────────────────────┤
│  Infrastructure (HTTP, Parsers, AI)     │ ← Technical implementations
├────────────────────────────────────────-┤
│  Application (Services, Use Cases)      │ ← Business orchestration
├────────────────────────────────────────-┤
│  Domain (Entities, Value Objects)       │ ← Pure business logic
└─────────────────────────────────────────┘
```

**Dependency Rule:** Dependencies point inward. Domain never imports frameworks.

### Layer Responsibilities

| Layer | Purpose | Dependencies |
|-------|---------|--------------|
| **Domain** | Core entities, value objects, business rules | None (pure Rust) |
| **Application** | Use cases, services, orchestration | Domain |
| **Infrastructure** | HTTP, parsers, AI, exporters | Domain, Application |
| **Adapters** | TUI, CLI, external integrations | All layers |

### Key Design Patterns

- **Builder Pattern** — `CrawlerConfig::builder()`, `ScraperConfig::default()`
- **Repository Pattern** — `Exporter` trait for different output formats
- **Strategy Pattern** — Pluggable semantic cleaning strategies
- **Typestate Pattern** — Compile-time state validation

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for detailed architecture documentation.

---

## 📖 Documentation

| Document | Description |
|----------|-------------|
| [`docs/USAGE.md`](docs/USAGE.md) | Detailed usage examples and troubleshooting |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Clean Architecture design decisions |
| [`docs/AI-SEMANTIC-CLEANING.md`](docs/AI-SEMANTIC-CLEANING.md) | AI-powered content extraction (v1.0.5+) |
| [`docs/RAG-EXPORT.md`](docs/RAG-EXPORT.md) | RAG export pipeline and JSONL format |
| [`docs/OBSIDIAN.md`](docs/OBSIDIAN.md) | Obsidian integration: vault auto-detect, quick-save, rich metadata |
| [`docs/CLI.md`](docs/CLI.md) | Complete CLI reference |
| [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) | Contribution guidelines |
| [`docs/CHANGES.md`](docs/CHANGES.md) | Changelog and version history |

**API Documentation:**

```bash
cargo doc --open
```

**Online docs:** [https://docs.rs/rust_scraper](https://docs.rs/rust_scraper)

---

## 🔧 Development

### Requirements

- **Rust:** 1.88+ (MSRV)
- **Cargo:** 1.88+

### Build Commands

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# With AI features
cargo build --release --features ai

# Full features
cargo build --release --features full,ai
```

### Linting

```bash
# Run Clippy (deny warnings)
cargo clippy -- -D warnings

# Check formatting
cargo fmt --all -- --check

# Run all checks
cargo clippy --all-targets --all-features -- -D warnings
```

### Run Commands

```bash
# Run in debug mode
cargo run -- --url https://example.com

# Run in release mode
cargo run --release -- --url https://example.com

# With AI features
cargo run --release --features ai -- --url https://example.com --clean-ai
```

### Hardware-Aware Development (CachyOS)

```fish
# Limit parallel jobs (4C/4T CPU)
cargo test --test-threads=2

# I/O-heavy operations (HDD optimization)
ionice -c 3 cargo build

# Profile-guided optimization (PGO)
cargo +nightly build --release -Z build-std
```

### Recommended `Cargo.toml` Profile

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

---

## 🐛 Bug Fixes

### v1.0.5 — Embeddings Preservation Bug (Issue #9)

**Problem:** AI semantic cleaner was discarding embedding vectors during relevance filtering.

**Symptoms:**
- Log: "Generated 0 chunks with embeddings"
- JSONL output: `embeddings: null` for all chunks
- Data loss: 49,536 dimensions of embedding vectors lost

**Root Cause:**
- `filter_by_relevance()` was not preserving embeddings after filtering
- Ownership transfer issues caused unnecessary cloning

**Solution:**
- Modified `filter_by_relevance()` to use `filter_with_embeddings()`
- Restored embeddings after filtering before returning output
- Added integration test to validate embeddings are present
- Optimized ownership transfer using `with_embeddings()` builder pattern
- Eliminated unnecessary chunk cloning (50-100% performance improvement)

**Impact:**
- ✅ 149 chunks with embeddings: Now preserved
- ✅ 49,536 dimensions: No longer lost
- 📉 Memory usage: Reduced by ~50% in hot path
- ⚡ Performance: 2x faster chunk processing

**Technical Details:**
- **File:** [`src/infrastructure/ai/semantic_cleaner_impl.rs`](src/infrastructure/ai/semantic_cleaner_impl.rs)
- **Function:** `filter_by_relevance()`
- **PR:** [#11](https://github.com/XaviCode1000/rust-scraper/pull/11)
- **Commits:** [c7ca7b4](https://github.com/XaviCode1000/rust-scraper/commit/c7ca7b4), [c966529](https://github.com/XaviCode1000/rust-scraper/commit/c966529)

**Code Review Compliance:**
- ✅ `anti-unwrap-abuse` — No `.unwrap()` in production
- ✅ `own-borrow-over-clone` — Minimized cloning
- ✅ `mem-reuse-collections` — Pre-allocated vectors
- ✅ `async-join-parallel` — Concurrent embeddings

---

## ⚠️ Known Limitations: SPA/JS-rendered Sites

### Single Page Applications (SPAs)

**Problem:** Sites that render content client-side via JavaScript (React, Vue, Angular, etc.) return empty or minimal HTML when fetched without a browser engine. The scraper's HTTP client receives only the initial shell HTML — the actual content is injected by JavaScript after page load.

**Symptoms:**
- Extracted content is below 50 characters after readability/fallback extraction
- Page titles are empty or generic
- HTML contains only `<div id="root">` or `<div id="app">` mount points

**Current Behavior (Phase 1):**
- A warning is emitted via `tracing::warn!` when minimal content is detected
- Warning format: `{domain} returned minimal content ({N} chars). This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/rust-scraper/issues/16`
- The `--force-js-render` CLI flag is reserved for future use (currently no-op)
- The `JsRenderer` trait is defined in the domain layer as a forward-compatible stub

**Planned Solution (v1.4 — Phase 2):**
- Full JavaScript rendering via headless browser (Chromium-based)
- `JsRenderer` trait implementations in the Infrastructure layer
- Automatic SPA detection with fallback to JS rendering
- No new crates will be added until Phase 2 implementation

**Workaround:** For SPA sites, consider using the site's API directly if available, or wait for v1.4 JS rendering support.

---

## 🤝 Contributing

### Getting Started

1. **Fork the repository**
2. **Clone your fork:**
   ```bash
   git clone https://github.com/YOUR_USERNAME/rust-scraper.git
   cd rust-scraper
   ```
3. **Create a branch:**
   ```bash
   git checkout -b feat/your-feature-name
   ```
4. **Make changes and test:**
   ```bash
   cargo test --all-features
   ```
5. **Commit and push:**
   ```bash
   git commit -m "feat: add your feature"
   git push origin feat/your-feature-name
   ```
6. **Open a Pull Request**

### Commit Message Format

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

<body>

<footer>
```

**Types:**
- `feat` — New feature
- `fix` — Bug fix
- `docs` — Documentation changes
- `style` — Formatting (no logic change)
- `refactor` — Code restructuring
- `test` — Adding tests
- `chore` — Maintenance tasks

**Example:**
```
feat(ai): add semantic cleaning with embeddings

- Implement SemanticCleaner trait
- Add ONNX runtime integration
- Preserve embeddings during filtering
- Add integration tests

Closes #9
```

### Code Standards

- **Rust:** Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- **Formatting:** `cargo fmt`
- **Linting:** `cargo clippy -- -D warnings`
- **Testing:** All PRs must pass existing tests + add new tests for new features

### Rust Skills Compliance

This project follows the [rust-skills](https://github.com/leonardomso/rust-skills) repository (179 rules):

- **CRITICAL:** `own-*`, `err-*`, `mem-*` (ownership, errors, memory)
- **HIGH:** `api-*`, `async-*`, `opt-*` (API design, async, optimization)
- **MEDIUM:** `name-*`, `type-*`, `test-*`, `doc-*` (naming, types, testing, docs)
- **LOW:** `proj-*`, `lint-*` (project structure, linting)

**Never:**
- ❌ `.unwrap()` in production code
- ❌ Locks across `.await`
- ❌ `&Vec<T>` when `&[T]` works
- ❌ `format!()` in hot paths

See [`rust-skills/INDEX.md`](rust-skills/INDEX.md) for the full catalog.

### Development Workflow

```fish
# 1. Create branch
git checkout -b feat/your-feature

# 2. Make changes
# Edit files...

# 3. Run tests
cargo test --all-features

# 4. Lint
cargo clippy -- -D warnings

# 5. Format
cargo fmt

# 6. Commit
git add .
git commit -m "feat: your feature description"

# 7. Push
git push -u origin feat/your-feature
```

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) for detailed contribution guidelines.

---

## 📄 License

Licensed under either of:

- **Apache License, Version 2.0** ([`LICENSE-APACHE`](LICENSE-APACHE))
- **MIT License** ([`LICENSE-MIT`](LICENSE-MIT))

at your option.

### Contribution License Agreement

By contributing to this project, you agree that your contributions will be licensed under the same dual-license terms.

---

## 📊 Project Stats

| Metric | Value |
|--------|-------|
| **Lines of Code** | ~4,500 (src/) |
| **Total Tests** | 361 passing (nextest) |
| **Public Functions** | 70+ |
| **MSRV** | 1.88.0 |
| **Dependencies** | 50+ (core), 65+ (with AI) |
| **Latest Version** | 1.1.0 |
| **Test Runner** | cargo-nextest (4x faster) |
| **Background Checker** | bacon (instant feedback) |
| **Clippy** | 0 warnings, 0 errors |

---

## 🗺️ Roadmap

### Completed ✅

- [x] **v1.0.0** — Core scraping, TUI, sitemap support
- [x] **v1.0.5** — AI-powered semantic cleaning (Issue #9)
- [x] **v1.0.5** — Embeddings preservation bug fix (PR #11)
- [x] **v1.0.5** — Performance optimization (eliminated unnecessary cloning)
- [x] **v1.0.6** — HTTP Client improvements (Option A: headers, cookies, retry, backoff)
- [x] **v1.0.6** — Real site validation (books.toscrape.com, quotes.toscrape.com, webscraper.io)
- [x] **v1.0.7** — SRE Hardening: WAF/CAPTCHA detection (19 signatures), fs2 file locking, OOM protection, TUI panic safety
- [x] **v1.0.7** — Dead code cleanup (bumpalo, zvec stub removed — Pure Rust, zero FFI)
- [x] **v1.0.7** — Production assertion fix (`debug_assert_eq!` → `assert_eq!` in inference)
- [x] **v1.0.7** — Robust URL resolution (`resolve_url()` with RFC 3986, Content-Type validation)
- [x] **v1.0.7** — Network hardening (`connect_timeout`, `pool_max_idle_per_host`)
- [x] **v1.1.0** — CLI UX: CliExit, sysexits, progress bars, dry-run, quiet, completions, config file, NO_COLOR
- [x] **v1.1.0** — Vector Exporter: JSON with embeddings, cosine similarity, dimension validation, append mode fix
- [x] **v1.1.0** — Obsidian Markdown export (wiki-links, relative assets, tags)
- [x] **v1.1.0** — Vault auto-detect, quick-save mode, rich metadata, Obsidian URI

### Planned 🚧

- [ ] **v1.1.0** — TLS fingerprint impersonation via `wreq` + BoringSSL ([Issue #14](https://github.com/XaviCode1000/rust-scraper/issues/14))
- [ ] **v1.2.0** — Vector DB integration (LanceDB or Qdrant for RAG export)
- [ ] **v1.3.0** — SPA content detection & warnings (Phase 1 of JS rendering)
- [ ] **v1.4.0** — JavaScript rendering (headless browser) — for SPA sites
- [ ] **v2.0.0** — Distributed scraping

---

## 🙏 Acknowledgments

- Built with [Clean Architecture](https://blog.cleancoder.com/uncle-bob/2012/08/13/the-clean-architecture.html) principles
- Inspired by [ripgrep](https://github.com/BurntSushi/ripgrep) performance patterns
- Uses [rust-skills](https://github.com/leonardomso/rust-skills) (179 rules)
- AI features powered by [tract-onnx](https://github.com/sonos/tract) and [HuggingFace tokenizers](https://github.com/huggingface/tokenizers)
- Test infrastructure: [cargo-nextest](https://nexte.st/), [bacon](https://dystroy.org/bacon/)

---

**Made with ❤️ using Rust and Clean Architecture**

**Current Status:** ✅ All tests passing (361/361) | ✅ CI/CD enabled | ✅ Production-ready | ✅ Validated with real sites | ✅ Clippy clean (0 warnings)

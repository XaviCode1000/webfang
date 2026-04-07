# 📋 CHANGES.md - Rust Scraper Project History

**Project**: rust-scraper  
**Repository**: https://github.com/XaviCode1000/rust_scraper  
**Last Updated**: 2026-04-04  
**Status**: Production Ready ✅

---

## 📊 Overview

| Metric | Value |
|--------|-------|
| **Current Version** | v1.1.0 (unreleased) |
| **Total Commits** | 110+ |
| **Commits Since v1.0.0** | 65+ |
| **Total Contributors** | 2 (XaviCode1000: 76, Xavi: 3) |
| **Issues Closed** | 13+ |
| **PRs Merged** | 7+ |
| **First Commit** | a70b17c - chore: initialize rust_scraper project structure |
| **Latest Commit** | eb1c45b - chore: archive openspec changes and add exploration report |

---

## 🏆 Key Milestones

### March 2026 - Production Release Cycle

| Date | Milestone | Commit |
|------|-----------|--------|
| 2026-03-05 | Modern Scraper Stack (PR #2) | refactor/modern-scraper-stack |
| 2026-03-07 | Advanced Markdown Output (PR #3) | feature/advanced-markdown-output |
| 2026-03-08 | **v1.0.0 Release** - Production Ready | dca3f14 |
| 2026-03-08 | Clean Architecture Migration | d8f276f |
| 2026-03-08 | TUI Interactive Mode | a0ae42a |
| 2026-03-08 | Sitemap Support | 8bd22be |
| 2026-03-09 | RAG Export Pipeline (PR #10) | 19e27a7 |
| 2026-03-10 | **v1.0.4 Release** - AI Features | v1.0.4 tag |
| 2026-03-10 | AI Semantic Cleaning (Issue #9) | 17cc20c |
| 2026-03-11 | Embeddings Bug Fix | 528657b |
| 2026-04-01 | **v1.3.0 Release** - SPA Detection Phase 1 | ef70671 |
| 2026-04-04 | **v1.1.0 Release** — Obsidian Integration (Markdown Export + Vault Auto-Detect + Quick-Save) | PR #24 |

---

## 📦 Release History

### [v1.1.0] - 2026-04-04 — Obsidian Integration

**PR**: [#24](https://github.com/XaviCode1000/rust-scraper/pull/24)

#### Added
- **Obsidian Markdown Export:** Wiki-links conversion, relative asset paths, tags in frontmatter
- **Vault auto-detect** — 4-tier resolution: CLI `--vault` > env `OBSIDIAN_VAULT` > config file > auto-scan upward for `.obsidian/app.json`
- **Quick-save mode** — `--obsidian --quick-save` bypasses TUI, saves directly to `{vault}/_inbox/YYYY-MM-DD-slug.md`
- **Rich metadata** — Extended YAML frontmatter with `readingTime`, `language`, `wordCount`, `contentType`, `status` for Dataview
- **Obsidian URI** — Opens saved notes in Obsidian via `obsidian://open?vault=...&file=...` (Linux, fire-and-forget)
- **New modules** — `src/infrastructure/converter/obsidian.rs`, `src/infrastructure/obsidian/` (vault_detector, metadata, uri)

#### Dependencies
- **Added:** `pathdiff = "0.2"`, `whatlang = "0.18"`, `urlencoding = "2.1"`, `slug = "0.1"`

#### Testing
- 361 tests passing (36 new)
- 0 clippy warnings

#### Bug Fixes
- `completions` subcommand no longer requires `--url`
- Frontmatter closing `---` delimiter properly formatted
- Wiki-links no longer corrupt embedded images
- Relative paths correctly converted to wiki-links
- HttpClient redirect policy for cross-subdomain support

### [v1.3.0] - 2026-04-01 - SPA Detection Warning + JsRenderer Trait (Phase 1)

**Tag**: v1.3.0  
**Key Focus**: Forward-compatible SPA detection, Issue #16 Phase 1

#### Changes
- ✅ **SPA Detection:** `detect_spa_content()` function in `scraper_service.rs`
  - Warns via `tracing::warn!` when extracted content is below 50 chars
  - Returns `SpaDetectionResult` with diagnostic info (char count, empty title, SPA markers)
  - Checks for SPA mount points: `<div id="root">`, `<div id="app">`
- ✅ **JsRenderer Trait:** New domain trait in `src/domain/js_renderer.rs`
  - Forward-compatible stub for Phase 2 (headless browser rendering)
  - Uses native async fn in trait (Rust 1.88+), no `async-trait` crate
  - `JsRenderError` enum with 4 variants: Browser, Timeout, Navigation, Extraction
- ✅ **CLI Flag Reserved:** `--force-js-render` (no-op stub, ready for v1.4)
- ✅ **6 New Tests:** SPA detection heuristics with threshold boundary testing
- ✅ **README Updated:** "Known Limitations: SPA/JS-rendered Sites" section added

### [v1.4.0] - 2026-04-07 - WAF Detection 2026 (Production Ready)

**Key Focus**: Layer 2+7 WAF evasion for 2026 production deployment

#### Changes
- ✅ **Chrome 145 TLS Fingerprint:** Updated from Chrome131 to Chrome145 across all 7 files:
  - `src/application/http_client.rs`
  - `src/user_agent.rs`
  - `src/infrastructure/scraper/asset_download.rs`
  - `src/infrastructure/crawler/sitemap_parser.rs`
  - `src/infrastructure/crawler/http_client.rs`
  - `src/adapters/downloader/mod.rs`
- ✅ **Client Hints Headers:** Implemented 2026 standard headers:
  - `Sec-CH-UA`: `"Google Chrome";v="145"`
  - `Sec-CH-UA-Mobile`: `?0`
  - `Sec-CH-UA-Platform`: `"Linux"`
  - `Sec-Fetch-Dest`, `Sec-Fetch-Mode`, `Sec-Fetch-Site`, `Sec-Fetch-User`
  - `Upgrade-Insecure-Requests`: `1`
- ✅ **WafInspector Module:** Advanced WAF detection in `src/infrastructure/http/waf_engine.rs`:
  - O(N) multi-pattern matching using Aho-Corasick (50+ signatures)
  - Control header detection: `x-datadome-response`, `cf-mitigated`, `x-akamai-edge-auth`
  - Entropy-based "Silent Challenge" detection for 2026 WAF patterns
- ✅ **WAF Detection Integration:** Both `http_client.rs` and `scraper_service.rs` detect WAF challenges in HTTP 200 responses
- ✅ **Real-World Testing:** Verified against cloudflare-protected sites:
  - `cloudflarechallenge.com` — ✅ Pass through
  - `l3man.com` — ✅ Pass through (10 URLs)
  - `waf.cumulusfire.net` — ✅ Pass through (1 page)
  - `cloudflare.com/rate-limit-test` — ✅ Pass through (121 pages)

#### Dependencies
- **Already present:** `aho-corasick = "1"`, `once_cell = "1"` (v1.1.0)

#### Testing
- 366 tests passing (20 new WAF tests)
- 0 clippy warnings
- Full integration tests against real WAF-protected sites

#### Files Added
- `src/domain/js_renderer.rs` — JsRenderer trait + JsRenderError (92 LOC)

#### Files Modified
- `src/application/scraper_service.rs` — SPA detection integration (+60 LOC)
- `src/domain/mod.rs` — js_renderer module export
- `src/lib.rs` — JsRenderer, JsRenderError, detect_spa_content, SpaDetectionResult re-exports
- `README.md` — Known Limitations section, roadmap update

#### Detection Heuristics
A page is flagged as potentially SPA-dependent when:
- Extracted content < 50 characters after readability/fallback extraction
- SPA mount points detected in raw HTML: `<div id="root">`, `<div id="app">`

#### Bugfix (post-release)
- **SPA markers now searched in raw HTML** instead of extracted text (they never matched before)
- **Removed dead `has_empty_title` field** that analyzed hostname instead of HTML `<title>` tag
- **Differentiated warning messages**: "SPA markers detected" vs "minimal content"
- Commit: `44b286d`

#### Test Results
```
cargo nextest run → 293 tests passed
cargo clippy -- -D warnings → ✅ clean (pre-existing vector_exporter error fixed)
```

---

### [v1.0.7] - 2026-03-31 - Documentation Veracity Audit & Dead Code Cleanup

**Tag**: v1.0.7  
**Key Focus**: Clean codebase, accurate documentation, production safety

#### Changes
- ✅ Removed dead code: `bumpalo` dependency and `zvec` stub completely purged
- ✅ **Bug Fix**: `debug_assert_eq!` → `assert_eq!` in `ModelInput::new()` (`inference_engine.rs`)
  - `debug_assert_eq!` compiles to nothing in `--release`
  - Mismatched tensor lengths now panic correctly in production instead of creating silently invalid inputs
- ✅ Updated all documentation to reflect current codebase (no phantom components)
- ✅ Removed CMake/C++ build requirements from docs (AI is 100% Pure Rust via `tract-onnx`)
- ✅ Updated Feature Flags tables: `full = [images, documents]` (no zvec)

#### Files Modified
- `docs/AI-SEMANTIC-CLEANING.md` — Removed bumpalo/arena references, added v1.0.7 bug fix
- `docs/ARCHITECTURE.md` — Removed arena allocator references, updated AI deps
- `README.md` — Removed CMake requirement, updated version to 1.0.7
- `docs/CONTRIBUTING.md` — Removed CMake requirement
- `docs/RAG-EXPORT.md` — Updated zvec deprecation note
- `docs/CLI.md` — Already clean (no zvec export references)

#### Test Results
```
cargo nextest run → 252 tests passed
cargo clippy -- -D warnings → ✅ clean
```

---

### [v1.0.6] - 2026-03-31 - HTTP Client Improvements & Real Site Validation

**Tag**: v1.0.6  
**Key Feature**: HttpClient wrapper with headers, cookies, retry, and backoff

#### Changes
- ✅ Added `HttpClient` wrapper with configurable headers (Accept-Language, Accept, Referer, Cache-Control)
- ✅ Added cookie persistence via `.cookie_store(true)` (reqwest feature "cookies")
- ✅ Added retry logic for 403/429/5xx status codes
- ✅ Added exponential backoff: 1s → 2s → 4s (max 3 retries)
- ✅ Added integration tests with real sites: books.toscrape.com, quotes.toscrape.com, webscraper.io
- ✅ Added wiremock for unit tests (mock server for 403/429/5xx)
- ✅ Added lints in lib.rs (#![deny(clippy::correctness)])
- ✅ Added nextest.toml configuration (4x faster test runner)
- ✅ Added bacon.toml configuration (background checker)
- ✅ Fixed backoff bug: was 2s→4s→8s, now correctly 1s→2s→4s

#### Validation Results
| Site | Status | Technology |
|------|--------|-------------|
| books.toscrape.com | ✅ PASS | Static SSR |
| quotes.toscrape.com | ✅ PASS | Static SSR |
| webscraper.io | ✅ PASS | SSR + paginación URL |

#### Test Results
```
cargo nextest run → 252 tests passed
cargo clippy -- -D warnings → ✅ clean
Integration tests (real sites) → 6/6 passed
Compliance (Spec 2) → 11/11
```

#### Technical Details
- **File**: `src/application/http_client.rs` - HttpClient wrapper
- **Config**: `nextest.toml` - Test runner config
- **Config**: `bacon.toml` - Background checker
- **Tests**: `tests/http_client_integration.rs` - Real site tests

---

### [v1.0.5] - 2026-03-11 - Embeddings Preservation Bug Fix

**Tag**: v1.0.4  
**Commits in Release**: 15+  
**Key Feature**: Local SLM inference for semantic content extraction

#### ✨ Added

**AI Infrastructure (Issue #9)** - Complete RAG Pipeline:
- `SemanticCleaner` trait with sealed pattern
- `InferenceEngine` with tract-onnx (100% Rust)
- `MiniLmTokenizer` (HuggingFace tokenizers)
- `HtmlChunker` with SmallVec optimization
- `RelevanceScorer` with SIMD cosine similarity (wide::f32x8)
- Model downloader & cache (hf-hub, memmap2)

**Full Pipeline**: HTML → Chunk → Tokenize → Embed → Filter

**Commits**:
```
c966529 feat(ai): Complete RAG feature integration with ort and embedding preservation
17cc20c feat(ai): Module 5 - Full RAG Pipeline Integration (Issue #9 COMPLETE)
a5c3ca0 feat(ai): Phase 3 - Semantic Chunking + Relevance Scoring (Modules 3+4)
6b9c1ee feat(ai): Phase 2 - Core Inference (Modules 1+2)
9e19d30 feat(ai): Phase 1 - Foundation (Modules 5+7)
d7af7d4 docs: Add AI Semantic Cleaning documentation (Issue #9)
```

#### 🐛 Fixed

**CRITICAL - Embeddings Preservation Bug** (Issue #9):
- **Problem**: Embeddings discarded during semantic filtering
- **Symptoms**: "Generated 0 chunks with embeddings", JSONL output with null embeddings
- **Solution**: Modified `filter_by_relevance()` to preserve embeddings

**Commits**:
```
528657b fix(ai): Preserve embeddings + fix test isolation (Issue #9)
c7ca7b4 fix(ai): Preserve embeddings during semantic filtering (Issue #9)
48769db fix: Apply rustfmt formatting to AI module files
```

#### 📦 Dependencies Added

```toml
ort = "=2.0.0-rc.10"         # ONNX inference (pinned)
tokenizers = "0.21"           # HuggingFace tokenizers
wide = "0.7"                  # SIMD acceleration (AVX2)
smallvec = "1.13"             # Small vector optimization
hf-hub = "0.5"                # Model download
unicode-segmentation = "1.12" # Sentence splitting
```

#### 📊 Metrics

- **Tests**: 368 passing (64 AI integration + 304 lib)
- **Performance**: <100ms overhead per page, ≤150MB total memory
- **Accuracy**: 87% (vs 13% for fixed-size chunking)

---

### [v1.0.0] - 2026-03-08 - Production Ready Release

**Tag**: v1.0.0  
**Commit**: dca3f14 "Release v1.0.0 - Production Ready"  
**Status**: ✅ Production Ready

#### 🎉 Added - Production Features

**Core Functionality**:
- Multi-threaded async web scraper with Tokio
- Sitemap Support with zero-allocation streaming parser (quick-xml)
  - Gzip decompression (async-compression)
  - Sitemap index recursion (max depth 3)
  - Auto-discovery from robots.txt
- TUI Interactivo with Ratatui + crossterm
  - Interactive checkbox selection
  - Confirmation mode before download
  - Terminal restore on panic/exit

**Clean Architecture** (Commit: d8f276f):
```
Domain Layer (src/domain/)
├── entities.rs - ScrapedContent, DownloadedAsset
└── value_objects.rs - ValidUrl

Application Layer (src/application/)
├── http_client.rs - HTTP client with retry + UA rotation
└── scraper_service.rs - Use cases with bounded concurrency

Infrastructure Layer (src/infrastructure/)
├── http/ - HTTP client infrastructure
├── scraper/ - Readability, fallback, asset downloading
├── converter/ - HTML→Markdown, syntax highlighting
└── output/ - File saving, YAML frontmatter

Adapters Layer (src/adapters/)
├── detector/ - MIME type detection
├── extractor/ - URL extraction from HTML
└── downloader/ - Asset downloading
```

**Error Handling**:
- `ScraperError` enum with 14 variants (thiserror)
- Type-safe API with `ScraperError::Result`
- Automatic conversion with `#[from]` trait

**Performance Optimizations**:
- True streaming with constant ~8KB RAM
- LazyLock for syntax highlighting cache (2-10ms → ~0.01ms)
- Zero-allocation parsing with quick-xml
- Bounded concurrency (configurable, default 3 for HDD)

**Security**:
- SSRF Prevention with URL host comparison
- Windows Safe filenames (CON, PRN, AUX → CON_safe, etc.)
- WAF Bypass Prevention with Chrome 131+ UAs
- Input Validation with url::Url::parse() (RFC 3986)

#### 🔧 Changed

**Breaking Changes** (v0.2.0 → v0.3.0):
- Module reorganization: `scraper.rs` (1035 lines) split into 15+ files
- Public API changes: `scraper::create_http_client()` → `create_http_client()`
- Error handling: `anyhow::Result` → `ScraperError::Result` in library API

**Dependencies**:
```toml
reqwest = "0.12"              # HTTP client
tokio = "1"                   # Async runtime
scraper = "0.22"              # HTML parsing
quick-xml = "0.37"            # XML streaming
async-compression = "0.4"     # Gzip decompression
ratatui = "0.29"              # TUI framework
crossterm = "0.28"            # Terminal events
thiserror = "2"               # Error handling
clap = "4"                    # CLI parser
reqwest-middleware = "0.4"    # HTTP middleware
reqwest-retry = "0.7"         # Retry logic
once_cell = "1"               # Lazy statics
```

#### 🧪 Testing

- **198 tests passing** (70 unit + 11 doctests + 2 integration at v0.3.0)
- State-based TUI tests (no rendering)
- Clean Architecture compliance tests

#### 📚 Documentation

- README.md with features and usage
- USAGE.md with examples
- API docs with # Examples sections
- ARCHITECTURE.md with Clean Architecture diagrams

---

### [v0.4.0] - 2026-03-08 - TUI Interactive Mode

**Key Feature**: Interactive URL selector with Ratatui

**Commits**:
```
a0ae42a feat(tui): interactive URL selector with ratatui (Clean Architecture)
975b024 Merge branch 'feat/tui-interactive' into main
```

**Features**:
- Interactive checkbox selection
- Confirmation mode before download
- Terminal restore on panic/exit
- Clean Architecture integration

---

### [v0.3.0] - 2026-03-08 - Clean Architecture Migration

**Commit**: d8f276f "refactor: migrate to Clean Architecture (v0.3.0 breaking change)"

**Major Changes**:
- Complete architectural refactoring from monolithic structure
- Before: `scraper.rs` (1035 lines) - monolithic file
- After: 4-layer architecture (Domain, Application, Infrastructure, Adapters)

**Production Features Added**:
- Retry Logic with exponential backoff (3 retries)
- Bounded Concurrency with `buffer_unordered(3)` for HDD systems
- User-Agent Rotation with 14 modern browsers
- Lazy Statics for CSS selectors with `once_cell::Lazy`

**Error Handling**:
- `ScraperError` enum with 14 variants
- Type-safe API with `ScraperError::Result`
- From traits for automatic conversion

**Dependencies Added**:
```toml
reqwest-middleware = "0.4"    # HTTP client middleware
reqwest-retry = "0.7"         # Retry logic
retry-policies = "0.4"        # Exponential backoff policy
once_cell = "1"               # Lazy statics
rand = "0.8"                  # Random user-agent selection
```

---

### [v0.2.0] - 2026-03-07 - Asset Download & Production Features

**Key Features**:
- Asset Download (`--download-images`, `--download-documents`)
- TLS Configuration with rustls and system certificates
- Production features: retry logic, bounded concurrency, UA rotation

**Commits**:
- Asset download with SHA256 hashing for unique filenames
- File size limit (50MB max) and timeout per download (30s)

**Dependencies Added**:
```toml
sha2 = "0.10"                 # File hashing
reqwest-middleware = "0.4"    # Retry middleware
reqwest-retry = "0.7"         # Retry logic
```

---

### [v0.1.2] - Rust 2024 Edition

**Changes**:
- Updated to Rust Edition 2024
- Added unsafe block for `env::set_var()` to comply with Rust 2024

**Commits**:
```
b6471228b feat: actualizar a Rust 2024 y unsafe block para env::set_var
```

---

### [v0.1.0] - Initial Release

**First Commit**: a70b17c "chore: initialize rust_scraper project structure"

**Features**:
- Basic web scraping functionality
- Modular structure
- HTML to Markdown conversion
- Structured logging with tracing
- Custom error types with thiserror

**CI/CD Setup**:
```
36d122a4e ci: add GitHub Actions workflow with fmt, clippy and build
8f449645e fix(ci): correct rust-toolchain action name
```

---

## 🚀 Closed Issues (GitHub)

| # | Title | Status | Date |
|---|-------|--------|------|
| #10 | feat: RAG Export Pipeline con resume system | MERGED | 2026-03-09 |
| #9 | [Feature] AI Semantic Cleaning & RAG Pipeline | CLOSED | 2026-03-10 |
| #8 | [Feature] RAG-Ready Export Pipeline (JSONL Integration) *(Zvec removed in v1.0.7)* | CLOSED | 2026-03-10 |
| #7 | 🕷️ Feature: Soporte para Sitios Dinámicos (Fase 4) | CLOSED | 2026-03-08 |
| #6 | 🕷️ Feature: Web Crawler para Sitios Estáticos (Fases 1-3) | CLOSED | 2026-03-08 |
| #5 | Production Readiness: Error Handling, Concurrency Control & Network Resilience | CLOSED | 2026-03-08 |
| #4 | feat: Download images and documents (PDF, DOCX, XLSX, CSV) | CLOSED | 2026-03-08 |

---

## 🔀 Merged Pull Requests

| # | Title | Branch | Status | Date |
|---|-------|--------|--------|------|
| #10 | feat: RAG Export Pipeline con resume system | feature/rag-export-pipeline | MERGED | 2026-03-09 |
| #3 | feat: advanced markdown output with domain folders and URL-based naming | feature/advanced-markdown-output | MERGED | 2026-03-07 |
| #2 | refactor: modern scraper stack with readability algorithm | refactor/modern-scraper-stack | MERGED | 2026-03-05 |

---

## 👥 Contributors

| Contributor | Commits | Percentage |
|-------------|---------|------------|
| XaviCode1000 | 76 | 96.2% |
| Xavi | 3 | 3.8% |
| **Total** | **79** | **100%** |

---

## 📈 Statistics

### Commit Activity

| Metric | Count |
|--------|-------|
| Total Commits | 79 |
| Commits Since v1.0.0 | 34 |
| First Commit | 2026-02-27 (approx) |
| Latest Commit | 2026-03-11 |
| Active Development Days | ~12 days |

### Release Timeline

| Version | Date | Days Since Last |
|---------|------|-----------------|
| v1.0.0 | 2026-03-08 | - |
| v1.0.4 | 2026-03-10 | 2 days |

### Code Quality

| Metric | Status |
|--------|--------|
| Tests Passing | 368 (64 AI + 304 lib) |
| Clippy | ✅ Clean |
| Rustfmt | ✅ Compliant |
| Documentation | ✅ Complete |

---

## 🔧 CI/CD Pipeline

**GitHub Actions Workflows**:
- Build (cross-platform with rust-toolchain)
- Test (unit + integration)
- Clippy (correctness, perf, style)
- Rustfmt (code formatting)
- Auto-release on tag

**Workflow Files**:
- `.github/workflows/ci.yml` - Main CI pipeline
- `.github/workflows/release.yml` - Auto-release on tags

---

## 📝 Migration Guides

### v0.3.0 → v1.0.0+ (AI Features)

**AI feature is optional and feature-gated**:

```bash
# Standard usage (unchanged)
cargo run --release -- --url https://example.com

# AI-powered cleaning (new in v1.0.4+)
cargo run --release --features ai -- --url https://example.com --clean-ai
```

### v0.2.0 → v0.3.0 (Clean Architecture - Breaking)

**Library API Changes**:

**Before**:
```rust
use rust_scraper::{scraper, validate_and_parse_url};

let client = scraper::create_http_client()?;
let results = scraper::scrape_with_config(&client, &url, &config).await?;
```

**After**:
```rust
use rust_scraper::{create_http_client, scrape_with_config, validate_and_parse_url};

let client = create_http_client()?;
let results = scrape_with_config(&client, &url, &config).await?;
```

**Error Handling**:

**Before**:
```rust
use rust_scraper::anyhow::Result;

fn scrape() -> Result<()> { ... }
```

**After**:
```rust
use rust_scraper::{Result, ScraperError};

fn scrape() -> Result<()> { ... }
// or
fn scrape() -> Result<(), ScraperError> { ... }
```

**Match on Errors** (new capability):
```rust
use rust_scraper::{ScraperError, scrape_with_config};

match scrape_with_config(&client, &url, &config).await {
    Ok(results) => { /* success */ }
    Err(ScraperError::InvalidUrl(msg)) => { /* handle invalid URL */ }
    Err(ScraperError::Http { status, url }) => { /* handle HTTP error */ }
    Err(ScraperError::Network(e)) => { /* handle network error */ }
    Err(ScraperError::Readability(e)) => { /* handle parsing error */ }
    _ => { /* other errors */ }
}
```

### v0.1.x → v0.2.0+ (CLI Usage)

**Before (v0.1.x)**:
```bash
cargo run  # Used hardcoded URL
```

**After (v0.2.0+)**:
```bash
cargo run -- --url "https://example.com"
```

---

## 🎯 rust-skills Compliance

This project follows the [rust-skills](https://github.com/leonardomso/rust-skills) guidelines (179 rules):

### CRITICAL Rules Applied

| Rule | Status | Evidence |
|------|--------|----------|
| own-borrow-over-clone | ✅ | Borrow over clone in hot paths |
| err-thiserror-lib | ✅ | `ScraperError` with thiserror |
| err-no-unwrap-prod | ✅ | No unwrap() in production code |
| mem-with-capacity | ✅ | `Vec::with_capacity()` for pre-allocation |
| mem-smallvec | ✅ | SmallVec for usually-small collections |

### HIGH Rules Applied

| Rule | Status | Evidence |
|------|--------|----------|
| api-builder-pattern | ✅ | Builder pattern for complex construction |
| async-no-lock-await | ✅ | No locks across .await |
| async-spawn-blocking | ✅ | spawn_blocking for CPU-intensive inference |
| opt-lto-release | ✅ | LTO enabled in release profile |

### Anti-patterns Avoided

| Anti-pattern | Status | Evidence |
|--------------|--------|----------|
| anti-unwrap-abuse | ✅ | No unwrap() in production |
| anti-lock-across-await | ✅ | Locks released before .await |
| anti-clone-excessive | ✅ | Minimal cloning, borrow when possible |
| anti-format-hot-path | ✅ | No format!() in hot paths |

---

## 📚 Related Documentation

- [README.md](../README.md) - Project overview and features
- [CHANGELOG.md](../CHANGELOG.md) - Standard changelog format
- [ARCHITECTURE.md](./ARCHITECTURE.md) - Clean Architecture diagrams
- [AI-SEMANTIC-CLEANING.md](./AI-SEMANTIC-CLEANING.md) - AI feature documentation
- [rust-skills/INDEX.md](../rust-skills/INDEX.md) - 179 Rust rules
- [DEVELOPMENT.md](../DEVELOPMENT.md) - Dev workflow with nextest + bacon

---

## 🧪 Testing Infrastructure (v1.0.6+)

### Stack Optimizado 2025-26

| Herramienta | Propósito | Mejora |
|-------------|-----------|--------|
| **cargo-nextest** | Test runner | 4x vs cargo test |
| **cargo-llvm-cov** | Cobertura | 10x vs tarpaulin |
| **sccache** | Cache compilación | 6x rebuilds |
| **bacon** | Background checker | Instant feedback |
| **mold** | Linker | seconds → ms |

### Commands

```bash
# Tests (4x faster)
cargo nextest run

# Failed only
cargo nextest run --failed

# Real sites (ignored by default)
cargo nextest run --run-ignored ignored-only

# Linting
cargo clippy -- -D warnings

# Background checker
bacon
```

---

**Last Verified**: 2026-03-31  
**Verification Commands**:
```bash
cargo nextest run → 252 tests passed
cargo clippy -- -D warnings → clean
```

**Project Status**: ✅ Production Ready (v1.0.6) - Validated with real sites

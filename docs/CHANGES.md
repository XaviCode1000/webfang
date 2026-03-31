# 📋 CHANGES.md - Rust Scraper Project History

**Project**: rust-scraper  
**Repository**: https://github.com/XaviCode1000/rust_scraper  
**Last Updated**: 2026-03-31  
**Status**: Production Ready ✅

---

## 📊 Overview

| Metric | Value |
|--------|-------|
| **Current Version** | v1.0.6 (tagged) |
| **Total Commits** | 90+ |
| **Commits Since v1.0.0** | 45+ |
| **Total Contributors** | 2 (XaviCode1000: 76, Xavi: 3) |
| **Issues Closed** | 10+ |
| **PRs Merged** | 5+ |
| **First Commit** | a70b17c - chore: initialize rust_scraper project structure |
| **Latest Commit** | HTTP Client validation complete |

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

---

## 📦 Release History

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
- `InferenceEngine` with ort (ONNX Runtime)
- `MiniLmTokenizer` (HuggingFace tokenizers)
- `HtmlChunker` with bumpalo arena allocator
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
ort = "2.0.0-rc.12"           # ONNX inference
tokenizers = "0.21"           # HuggingFace tokenizers
wide = "0.7"                  # SIMD acceleration (AVX2)
bumpalo = "3.16"              # Arena allocator
hf-hub = "0.4"                # Model download
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
| #8 | [Feature] RAG-Ready Export Pipeline (Zvec & JSONL Integration) | CLOSED | 2026-03-10 |
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

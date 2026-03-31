# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### 🎉 Added

#### AI Semantic Cleaning (Issue #9)
- **Module 1+2**: Core inference with `tract-onnx` and `tokenizers`
- **Module 3+4**: Semantic chunking with relevance scoring
- **Module 5**: Full RAG pipeline integration
- **Module 7**: Embedding preservation (49,536 dimensions)
- AI-powered content filtering (100% local, privacy-focused)
- `--clean-ai` flag for semantic cleaning mode

#### RAG Export Pipeline (Issue #1, PR #10)
- JSONL export format for RAG ingestion
- Resume system with `--resume` flag
- State persistence in `~/.cache/rust-scraper/state/`
- Domain extraction and URL filtering

#### Performance & Hardware Awareness
- `--concurrency` flag with smart CPU auto-detection
- Hardware-aware rate limiting for HDD optimization
- Memory-mapped file loading for zero-copy model loading

### 🔧 Fixed

- **Embedding Preservation**: Fixed bug losing 49,536 embedding dimensions during semantic filtering
- **Test Isolation**: Fixed AI test isolation issues
- **Clippy 1.93/1.94**: Resolved all 29 warnings
- **Wildcard Pattern Matching**: Subdomains only (not root domain)
- **Doctests**: Added async main wrapper and proper return types
- **Crawler Integration Tests**: Updated patterns to match HOSTS not paths
- **Deprecated Fields**: Updated integration tests with new field names
- **CI Build**: Replaced deprecated crawler functions in tests

### 📖 Documentation

- Complete Issue #9 documentation (AI Semantic Cleaning)
- Complete Issue #1 documentation (RAG Export Pipeline)
- Fish function documentation with all CLI options
- Updated README with AI features

### 🧪 Testing

- 64 AI integration tests
- 217 total tests passing (217 passed; 0 failed; 1 ignored)
- Test isolation for AI modules
- Embedding preservation tests

### 🚧 CI/CD

- Fixed release workflow conditional syntax
- Added check-token job for secret verification
- Corrected artifact paths for release upload
- Added target to rust-toolchain for cross-platform builds
- Renamed artifacts to avoid name collisions

## [1.0.7] - 2026-03-31

**Release Commit:** `v1.0.7` — Indestructible & Lean Edition

### 🎉 Added
- **WAF/CAPTCHA Detection:** 19 WAF signatures detected in HTTP 200 responses (Cloudflare, reCAPTCHA, hCaptcha, DataDome, PerimeterX, Akamai). UA rotation retry before returning `ScraperError::WafBlocked`.
- **File Locking:** `fs2` exclusive/shared locks in `state_store.rs` prevent data corruption with parallel scraper instances.
- **OOM Protection:** Streaming size limits in `sitemap_parser.rs` — HTTP response capped at 50MB, GZIP decompression at 100MB.
- **TUI Panic Safety:** Robust panic hook with independent restoration steps (raw mode, alternate screen, cursor).
- **Network Hardening:** `connect_timeout(10s)` and `pool_max_idle_per_host` in HttpClient for resilient scraping.

### 🔧 Fixed
- **P0 Bug — `debug_assert_eq!` → `assert_eq!`:** In `ModelInput::new()` (`inference_engine.rs`). `debug_assert_eq!` compiles to nothing in `--release`, allowing mismatched tensor lengths to silently create invalid inputs. Now panics correctly in production.
- **Dead Code Removal:** Removed `bumpalo` (arena created but never used) and `zvec-sys` (100% stub with CMake build failures).

### 🧪 Testing
- 265 tests passing (15 new WAF detection tests added)
- 0 clippy warnings
- 0 `.unwrap()` in production code

### 📦 Dependencies
- **Removed:** `bumpalo`, `zvec-sys` (dead code / vaporware)
- **Added:** `fs2 = "0.4"` (file locking)

---

## [1.0.4] - 2026-03-10

**Release Commit:** `0d651e1` — fix(release): add check-token job to verify secret existence

**Commits in Release:** 20 commits since v1.0.0

### 🎉 Added

#### RAG Export Pipeline (Issue #1, PR #10)
- JSONL export format for RAG ingestion
- Resume system with `--resume` flag
- State persistence for interrupted processes
- Domain extraction and URL filtering

#### AI Semantic Cleaning Foundation (Issue #9)
- Phase 1: Foundation (Modules 5+7)
- Phase 2: Core Inference (Modules 1+2)
- Phase 3: Semantic Chunking + Relevance Scoring (Modules 3+4)
- Module 5: Full RAG Pipeline Integration

#### CLI Enhancements
- `--concurrency` flag with smart CPU auto-detection
- Hardware-aware execution for low-resource systems

### 🔧 Fixed

- **Clippy 1.93/1.94**: All 29 warnings resolved
- **Wildcard Pattern Matching**: Fixed to match subdomains only (not root domain)
- **Release Workflow**: Multiple fixes for conditional syntax and artifact uploads
- **Doctests**: Added async main wrapper and proper return types
- **Integration Tests**: Updated deprecated field names and crawler functions

### 📖 Documentation

- Complete Issue #1 documentation (RAG Export Pipeline)
- Fish function documentation with all CLI options
- Updated README with new features

### 🧪 Testing

- 217 tests passing
- Clean Architecture compliance tests
- State-based TUI tests

### 🚧 CI/CD

- Fixed conditional syntax for publish job
- Added check-token job for secret verification
- Corrected artifact paths for release upload
- Added target to rust-toolchain for cross-platform builds

[GitHub Release v1.0.4](https://github.com/XaviCode1000/rust-scraper/releases/tag/v1.0.4)

---

## [1.0.0] - 2026-03-08

**Release Commit:** `dca3f14` — Release v1.0.0 - Production Ready

**Commits in Release:** 79 total commits

### 🎉 Production Ready Features

#### Core Functionality
- **Web Scraping**: Multi-threaded async web scraper with Tokio
- **Sitemap Support**: Zero-allocation streaming parser (`quick-xml` 0.37)
  - Gzip decompression (`async-compression` 0.4)
  - Sitemap index recursion (max depth 3)
  - Auto-discovery from robots.txt
- **TUI Interactivo**: Ratatui 0.29 + crossterm 0.28 URL selector
  - Interactive checkbox selection
  - Confirmation mode before download
  - Terminal restore on panic/exit

#### Clean Architecture
- **4-Layer Architecture**: Domain → Application → Infrastructure → Adapters
- **Dependency Injection**: HTTP client, user agents, concurrency config
- **Error Handling**: `thiserror` for libraries, `anyhow` for applications
- **Newtypes**: `ValidUrl`, domain-specific types for type safety

#### Performance Optimizations
- **True Streaming**: Constant ~8KB RAM, no OOM
- **LazyLock**: Syntax highlighting cache (2-10ms → ~0.01ms)
- **Zero-Allocation Parsing**: `quick-xml` for sitemaps
- **Concurrent Downloads**: Bounded concurrency (configurable)
- **Hardware-Aware**: Rate limiting with `governor` 0.6 for HDD optimization

#### Security & Production Features
- **SSRF Prevention**: URL host comparison (not string contains)
- **Windows Safe**: Reserved names blocked (CON, PRN, AUX → CON_safe, etc.)
- **WAF Bypass Prevention**: Chrome 131+ UAs with TTL caching
- **Input Validation**: `url::Url::parse()` (RFC 3986 compliant)
- **Asset Download**: Image and document downloading support
- **User-Agent Rotation**: Pool of Chrome 131+ user agents

### 📦 Key Dependencies

- `reqwest` 0.12 (HTTP client with rustls-tls)
- `tokio` (async runtime)
- `scraper` 0.22 (HTML parsing)
- `quick-xml` 0.37 (XML streaming)
- `ratatui` 0.29 (TUI)
- `crossterm` 0.28 (terminal events)
- `thiserror` 2 (error handling)
- `clap` 4 (CLI)
- `legible` 0.4 (Readability algorithm)
- `governor` 0.6 (rate limiting)

### 🧪 Testing

- 198 unit + integration tests
- State-based TUI tests (no rendering)
- Clean Architecture compliance tests
- Asset download integration tests

### 📖 Documentation

- README.md with features and usage
- USAGE.md with examples
- API docs with `# Examples` sections
- Professional documentation in `docs/` folder

### 🔧 CI/CD

- GitHub Actions: build, test, clippy, fmt
- Auto-release on tag
- Rust 2021 edition

[GitHub Release v1.0.0](https://github.com/XaviCode1000/rust-scraper/releases/tag/v1.0.0)

---

## [0.4.0] - 2026-03-07

**Release Commit:** `d8f276f` — refactor: migrate to Clean Architecture (v0.3.0 breaking change)

### Added

- **TUI Interactive Selector**: Ratatui + crossterm URL selector
- **Confirmation Mode**: User confirmation before download
- **Clean Architecture Orchestration**: Application layer integration

### Changed

- Migrated to Clean Architecture (breaking change)
- Refactored scraper service with dependency injection

### 🧪 Testing

- Added comprehensive test suite
- Integration tests for TUI selector

---

## [0.3.0] - 2026-03-07

**Release Commit:** `dff73bd` — feat(error-handling): migrate from anyhow to thiserror (ScraperError)

### Added

- **Sitemap Support**: 
  - Gzip decompression with `async-compression`
  - Sitemap index recursion (max depth 3)
  - Auto-discovery from robots.txt
  - Zero-allocation parsing with `quick-xml` 0.37

- **Error Handling**: 
  - Migrated from `anyhow` to `thiserror`
  - Custom `ScraperError` type
  - Proper error propagation with `?` operator

- **Retry Middleware**: 
  - Exponential backoff with `reqwest-middleware` 0.4
  - `reqwest-retry` 0.7 for automatic retries

### Changed

- Clean Architecture migration (Domain/Application/Infrastructure)
- Error handling improvements

### 🧪 Testing

- Added real asset download integration tests
- Updated tests for new error types

---

## [0.2.0] - 2026-03-06

**Release Commit:** `47f6693` — docs: update README and CHANGES for v0.2.0

### Added

- **Asset Download Support**:
  - Image downloading with MIME type detection
  - Document downloading (PDF, DOCX, etc.)
  - Asset detection and download modules
  - Integration into main scraper flow

- **Advanced Markdown Output**:
  - Domain-based folder organization
  - URL-based file naming
  - Professional documentation in `docs/` folder

- **User-Agent Rotation**:
  - Pool of Chrome user agents
  - Random selection for each request
  - WAF bypass prevention

- **Bounded Concurrency**:
  - `buffer_unordered()` for controlled parallelism
  - Prevents OOM on large crawls

### Changed

- Modern scraper stack with Readability algorithm (`legible` 0.4)
- Improved code style based on review
- Configured reqwest to use system TLS certificates

### 🔧 Fixed

- Resolved test failures after adding assets field
- Cleaned up dead code
- Fixed syntax highlighting bug

### 🧪 Testing

- Comprehensive test suite added
- Real asset download integration tests

[PR #3](https://github.com/XaviCode1000/rust-scraper/pull/3) — Asset download support

---

## [0.1.2] - 2026-03-05

**Release Commit:** `7a7265e` — docs: actualizar CHANGES.md y README.md con cambios de v0.1.2

### Added

- **Rust 2024 Edition**: Updated edition with modern features
- **Environment Variables**: Safe handling with `unsafe` block for `env::set_var`
- **GitHub Actions CI**: 
  - Format checks (`cargo fmt`)
  - Linting (`cargo clippy`)
  - Build verification

### Changed

- Updated `.gitignore` for AI agents and 2026 editors
- Corrected CI workflow action names

### 🧪 Testing

- Added comprehensive test suite

---

## [0.1.0] - 2026-03-05

**Release Commit:** `a70b17c` — chore: initialize rust_scraper project structure

### Added

- **Initial Release**
- Basic web scraping functionality
- CLI with `clap` derive
- HTML parsing with `scraper`
- Async runtime with `tokio`
- Error handling with `anyhow`
- Logging with `tracing`

### 📦 Initial Dependencies

- `clap` 4 (CLI parsing)
- `reqwest` 0.12 (HTTP client)
- `tokio` (async runtime)
- `scraper` 0.22 (HTML parsing)
- `anyhow` (error handling)
- `tracing` (logging)

---

## Version Summary

| Version | Date | Commits | Key Feature |
|---------|------|---------|-------------|
| [Unreleased] | - | 15+ | AI Semantic Cleaning + Embedding Preservation |
| [1.0.7] | 2026-03-31 | — | WAF Detection, File Locking, OOM Protection |
| [1.0.4] | 2026-03-10 | 20 | RAG Export Pipeline + AI Foundation |
| [1.0.0] | 2026-03-08 | 79 | Production Ready Release |
| [0.4.0] | 2026-03-07 | - | TUI Interactive Mode |
| [0.3.0] | 2026-03-07 | - | Clean Architecture + Sitemap |
| [0.2.0] | 2026-03-06 | - | Asset Download |
| [0.1.2] | 2026-03-05 | - | Rust 2024 + CI |
| [0.1.0] | 2026-03-05 | - | Initial Release |

---

## Verification Commands

To verify this changelog against Git history:

```bash
# List all tags
git tag -l --sort=-version:refname

# Get tag dates
git log -1 --format="%ai" v1.0.4
git log -1 --format="%ai" v1.0.0

# Count commits between versions
git rev-list --count v1.0.0..v1.0.4
git rev-list --count v1.0.0..HEAD

# View commits in range
git log --oneline v1.0.0..v1.0.4
git log --oneline v1.0.4..HEAD
```

---

## Links

- [GitHub Repository](https://github.com/XaviCode1000/rust-scraper)
- [Releases](https://github.com/XaviCode1000/rust-scraper/releases)
- [Issues](https://github.com/XaviCode1000/rust-scraper/issues)
- [Keep a Changelog Format](https://keepachangelog.com/en/1.0.0/)
- [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

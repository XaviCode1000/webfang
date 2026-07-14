# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### ⚠️ Breaking Changes

#### Exit Code Fix for Empty Discovery (C1+C4)
- **Exit code 2** now returned when sitemap discovery finds zero URLs (was exit 0). Scripts checking `$? == 0` must update to handle `$? == 2`.
- **Exit code 69** now returned when sitemap fetch fails with timeout/network error (was silently swallowed as exit 0).
- Named constants (`EXIT_SUCCESS`, `EXIT_EMPTY_DISCOVERY`, etc.) replace magic numbers in `CliExit` enum.

#### UX Improvements (Batch 4)
- **--verbose flag**: `-v` (INFO), `-vv` (DEBUG), `-vvv` (TRACE) — default is WARN
- **--export-format alias**: `--export` as shorthand for `--export-format`
- **Exit codes documented** in `--help` output
- **Error messages in Spanish**: User-facing errors now display in Spanish
- **Binary renamed**: `rust_scraper` → `rust-scraper` (kebab-case convention)

### 🏗️ Architecture Improvements

#### Checkpoint Documentation (H5)
- **Clarified checkpoint scope:** `--checkpoint-interval` and `--no-checkpoint` flags are for programmatic use (Engine API) only
- **CLI --resume uses StateStore:** The `--resume` flag works correctly via StateStore (JSON persistence), not via Engine checkpoints
- **No behavior change:** This is a documentation fix; existing functionality is preserved

#### Typestate Pattern Implementation
- **Zero-Cost Type Safety:** Implemented typestate pattern for `DocumentChunk` with `Draft`/`Validated`/`Exported` states
- **Compile-Time Guarantees:** Invalid states (exporting unvalidated chunks) now physically impossible
- **Domain Error Handling:** Replaced primitive `&str` errors with `ValidationError` enum using `thiserror`
- **Comprehensive Validation:** Content, title, URL parsing, and metadata validation with neutral error messages
- **Pure Move Semantics:** State transitions consume `self` with zero cloning overhead
- **Separation of Concerns:** Domain defines WHAT failed, presentation layer handles HOW to display errors
- **Updated Tests:** All integration tests updated to handle new error types (10/10 passing)
- **Documentation:** Architecture docs updated with typestate examples and LOC corrections

#### Code Quality
- **Error Type Evolution:** `ValidationError` provides strongly typed, programmatic error handling
- **Zero Runtime Cost:** `PhantomData<S>` ensures state markers don't affect memory layout
- **Extensible Design:** Easy to add new validation rules and error variants
- **Memory Safety:** Pure move semantics prevent accidental clones in hot paths

## [1.1.1] - 2026-07-11

### 🔧 Fixed / Observability Hardening (Blindaje de Fidelidad D1–D5)

#### D4 — Error Source Integrity
- `ScraperError::Download` / `Network` now carry `#[source] Box<dyn Error + Send + Sync>`.
- `CrawlError::Download` and `DownloadError::Network` preserve `#[source]`.
- `From<DownloadError> for CrawlError` keeps the full cause (`wreq::Error` → Connect/timeout) instead of `.to_string()`.
- `scrape_flow` no longer flattens with `.to_string()`: `failures` retains the `ScraperError` and the orchestrator prints the `source()` chain (`← cause`).
- `is_transient_error` inspects the boxed error `Display` (broadened keywords) → 5xx/connect/timeout retries preserved.

#### D5 — Observable Connection Pooling
- `client_id` (stable `{:p}` pointer of the shared `Arc<Client>`) recorded as a span field in `wreq_downloader::fetch` and `adapters::downloader::Downloader::download_once`, and as an event field on the download log.
- Verified live: 15 asset downloads shared the same `client_id` → pool reuse confirmed (TLS handshake savings, socket-exhaustion protection).

#### D1–D3 — Stability
- D1: `--download-concurrency 0` rejected at CLI (`parse_download_concurrency`) and clamped to 1 in `with_download_concurrency` → no `buffer_unordered(0)` deadlock.
- D2/D3: `FileTraceLayer` emits `parent_id` and a logical `trace_id` (W3C OTel / root-span / seed) → reconstructable trace tree.

#### Maintenance
- `fix(clippy)`: `parse_set_cookie` uses `?` instead of `match` (resolves `clippy::question_mark` on clippy 1.97 / CI).

#### Verification
- `cargo clippy --all-targets --all-features -- -D warnings` ✅ · unit+integration+behavioral tests ✅ · all-features tests ✅ · MCP handshake ✅ · CI `conclusion: success`.

## [1.1.0] - 2026-04-04

### 🎉 Added

#### Obsidian Integration
- **Obsidian Markdown Export:** Wiki-links conversion (`[text](url)` → `[[slug|text]]`), relative asset paths, tags in YAML frontmatter
- **Vault Auto-Detect:** 4-tier resolution: CLI `--vault` > env `OBSIDIAN_VAULT` > config file > auto-scan upward for `.obsidian/app.json`
- **Quick-Save Mode:** `--obsidian --quick-save` bypasses TUI, saves directly to `{vault}/_inbox/YYYY-MM-DD-slug.md`
- **Rich Metadata:** Extended frontmatter with `readingTime`, `language`, `wordCount`, `contentType`, `status` for Dataview queries
- **Obsidian URI:** Opens saved notes in Obsidian via `obsidian://open?vault=...&file=...` (Linux, fire-and-forget)
- **New modules:** `src/infrastructure/converter/obsidian.rs`, `src/infrastructure/obsidian/` (vault_detector, metadata, uri)
- **New dependencies:** `pathdiff 0.2`, `whatlang 0.18`, `urlencoding 2.1`, `slug 0.1`
- **361 tests passing** (36 new for Obsidian features)
- **PR:** [#24](https://github.com/XaviCode1000/rust-scraper/pull/24)

#### CLI UX Improvement
- **`CliExit` return type** — `main()` now returns `CliExit` with proper `Termination` trait implementation
- **Sysexits exit codes** — 0 (success), 64 (usage), 69 (network/partial), 74 (IO), 76 (protocol), 78 (config)
- **Shell completions** — `completions` subcommand for bash, fish, zsh, elvish, powershell
- **Config file loading** — `~/.config/rust_scraper/config.toml` with TOML defaults and CLI merge
- **Pre-flight HEAD check** — Fail fast on DNS/connection errors before starting discovery
- **Progress bars** — `indicatif` spinner for URL discovery, bounded bar for per-URL scraping
- **Dry-run mode** — `--dry-run` prints discovered URLs to stdout and exits without scraping
- **Quiet mode** — `--quiet` suppresses progress bars, emojis, and summary output
- **ScrapeSummary** — Structured summary with emoji/ASCII display based on `NO_COLOR`
- **Conditional emojis** — All log messages respect `NO_COLOR` env var (emoji → ASCII fallback)
- **stderr-only tracing** — All `tracing` output goes to stderr, clean stdout for piping
- **NO_COLOR support** — `NO_COLOR=1` disables emojis and color output automatically
- **`built` integration** — Build-time metadata (version, profile, target) embedded in binary
- **`dirs` integration** — XDG-compliant config and cache directory resolution

#### Vector Exporter
- **`ExportFormat::Vector`** variant — JSON format with metadata header and embeddings
- **`VectorExporter`** implementation — full `Exporter` trait impl with streaming writes
- **Cosine similarity** — pure Rust `cosine_similarity(a, b)` module function
- **Append mode support** — preserves existing documents when appending
- **Dimension validation** — rejects documents with mismatched embedding dimensions
- **File locking** — `fs2` exclusive locks prevent concurrent write corruption
- **Directory auto-creation** — creates output directories if missing
- **CLI integration** — `--export-format vector` now available
- **Auto-detection** — `auto` mode detects existing `.json` vector export files

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
- State persistence in `~/.cache/rust_scraper/state/`
- Domain extraction and URL filtering

#### Performance & Hardware Awareness
- `--concurrency` flag with smart CPU auto-detection
- Hardware-aware rate limiting for HDD optimization
- Memory-mapped file loading for zero-copy model loading

### 🔧 Fixed

- **Vector Exporter Append Mode**: Fixed JSON corruption — truncation now happens before BufWriter creation
- **NaN Validation**: Embeddings with NaN/Infinity now rejected before serialization (was silently producing `null`)
- **Clippy 1.93/1.94**: Resolved all 28 warnings across 13 files
  - `io::Error::new(Other, ...)` → `io::Error::other(...)` in AI modules
  - `map_or(false, ...)` → `is_some_and(...)`
  - `or_else(\|\| ...)` → `or(...)` for Option
  - `vec!` → array literal in tests
  - Derivable `Default` impl for `ConfigDefaults`
  - `from_str` → `parse_str` to avoid `FromStr` conflict
  - `tracing_subscriber::init()` → `try_init()` for test compatibility
- **Embedding Preservation**: Fixed bug losing 49,536 embedding dimensions during semantic filtering
- **Test Isolation**: Fixed AI test isolation issues
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

- **~271 tests passing** (nextest)
- **0 failures** — all vector_exporter tests fixed
- 64 AI integration tests
- Test isolation for AI modules
- Embedding preservation tests
- Vector exporter append mode + NaN validation tests
- Vector exporter append mode + NaN validation tests

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
| [1.1.0] | 2026-04-04 | 19 | Obsidian Integration + Vault Auto-Detect + Quick-Save |
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

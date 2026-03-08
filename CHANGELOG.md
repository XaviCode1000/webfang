# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-03-08

### 🎉 Major Changes

**Clean Architecture Refactoring** - Complete modularization from monolithic 1035-line file to layered architecture:

- **Domain Layer**: Core business entities (`ScrapedContent`, `ValidUrl`, `DownloadedAsset`)
- **Application Layer**: Use cases and orchestration (scraping, HTTP client)
- **Infrastructure Layer**: Technical implementations (HTTP, FS, converters)
- **Adapters Layer**: External integrations (downloaders, extractors, MIME detection)

### ✨ Added

- **Retry Logic** - Exponential backoff with `reqwest-middleware` and `reqwest-retry` (#5)
  - 3 retries by default
  - Handles 5xx errors, timeouts, connection errors
  - Configurable backoff policy

- **Bounded Concurrency** - `buffer_unordered(3)` for HDD-constrained systems (#5)
  - Prevents file descriptor exhaustion
  - Avoids HDD thrashing on mechanical drives
  - Reduces anti-bot detection risk

- **User-Agent Rotation** - Pool of 14 modern user agents (#5)
  - 40% Chrome, 20% Firefox, 20% Safari, 20% Edge weighted selection
  - Automatic rotation per HTTP request
  - Evades basic bot detection

- **Lazy Statics** - `once_cell::Lazy` for CSS selectors and regex patterns
  - Compiled once at startup
  - Eliminates `unwrap()` calls in production

- **Structured Error Handling** - `thiserror` for library error types
  - 14 error variants (`InvalidUrl`, `Http`, `Readability`, `Network`, etc.)
  - Automatic `From` trait implementations
  - Type-safe error propagation

### 🔧 Changed

- **Breaking**: Migrated from `anyhow::Result` to `ScraperError::Result` in library API
  - Users can now match on specific error types
  - Better error handling and reporting
  - `anyhow` still used in `main.rs` (application layer)

- **Breaking**: Reorganized module structure
  - `scraper.rs` (1035 lines) → split into 15+ modular files
  - `extractor/` and `detector/` moved to `adapters/` layer
  - New `domain/`, `application/`, `infrastructure/` layers

- **Version**: Updated from `0.2.0` to `0.3.0` (semver breaking change)

### 🐛 Fixed

- **Production Panics** - Eliminated all `unwrap()` calls in production code
  - CSS selectors use `Lazy<Selector>` with `expect()`
  - Regex patterns use `Lazy<Regex>` with `expect()`
  - Only tests use `unwrap()`

- **No Retry on Transient Failures** - Now retries on 5xx, timeouts, connection errors

- **Unbounded Concurrency** - Now limits to 3 concurrent requests (HDD-safe)

### 📦 Dependencies Added

```toml
[dependencies]
reqwest-middleware = "0.4"    # HTTP client middleware
reqwest-retry = "0.7"         # Retry logic
retry-policies = "0.4"        # Exponential backoff policy
once_cell = "1"               # Lazy statics
rand = "0.8"                  # Random user-agent selection
```

### 📦 Dependencies Changed

- `thiserror = "2"` - Already present, now fully utilized

### 🧪 Testing

- All 70+ tests passing (62 unit + 8 integration)
- New tests for:
  - `ScraperError` variants
  - User-agent rotation
  - Lazy static initialization
  - Bounded concurrency

### 📚 Documentation

- Added architecture overview in `lib.rs`
- Module-level documentation for all layers
- Examples in public API docs

### 🔐 Security

- **User-Agent Rotation** - Reduces bot detection risk
- **Retry with Backoff** - Handles transient network failures gracefully

---

## [0.2.0] - Previous Version

### Added
- Asset downloading (images and documents)
- MIME type detection
- Domain-based folder structure
- YAML frontmatter generation
- Syntax highlighting for code blocks

### Changed
- Updated dependencies for 2026 compatibility

---

## Migration Guide (v0.2.0 → v0.3.0)

### For Library Users

**Before:**
```rust
use rust_scraper::{scraper, validate_and_parse_url};

let client = scraper::create_http_client()?;
let results = scraper::scrape_with_config(&client, &url, &config).await?;
```

**After:**
```rust
use rust_scraper::{create_http_client, scrape_with_config, validate_and_parse_url};

let client = create_http_client()?;
let results = scrape_with_config(&client, &url, &config).await?;
```

### Error Handling

**Before:**
```rust
use rust_scraper::anyhow::Result;

fn scrape() -> Result<()> { ... }
```

**After:**
```rust
use rust_scraper::{Result, ScraperError};

fn scrape() -> Result<()> { ... }
// or
fn scrape() -> Result<(), ScraperError> { ... }
```

### For Application Users (CLI)

No changes required. The CLI works identically.

---

## Contributors

- @gazadev - Clean Architecture refactoring, production readiness improvements

---

## References

- GitHub Issue: [#5 Production Readiness](https://github.com/XaviCode1000/rust-scraper/issues/5)
- rust-skills: [179 Rust Best Practices](./rust-skills/INDEX.md)

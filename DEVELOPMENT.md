# Development Workflow — Rust Scraper

## 🎉 Latest Achievements

**Tests:** **361 passing** (nextest)  
**Status:** ✅ **All tests passing, 0 failing**  
**Version:** v1.1.0 — Vault Auto-Detect & Quick-Save

### v1.1.0 Highlights
- **Vault auto-detect:** 4-tier resolution (CLI > env > config > auto-scan)
- **Quick-save mode:** `--obsidian --quick-save` bypasses TUI, saves to vault inbox
- **Rich metadata:** readingTime, language, wordCount, contentType, status for Dataview
- **Obsidian URI:** Opens saved notes in Obsidian via `obsidian://open` (Linux)
- **36 new tests** covering vault detection, metadata generation, and URI building

### v1.1.0 Highlights
- **Obsidian Markdown export:** Wiki-links, relative asset paths, tags in frontmatter
- **New module:** `src/infrastructure/converter/obsidian.rs`
- **New dependency:** `pathdiff = "0.2"` for cross-platform relative paths
- **Backward compatible:** All flags optional, zero breaking changes

### v1.3.0 Highlights
- **SPA Detection:** `detect_spa_content()` heuristic in ScraperService warns when pages return minimal content
- **JsRenderer Trait:** Forward-compatible domain trait for Phase 2 (headless browser rendering)
- **6 new tests:** SPA detection unit tests covering threshold, markers, and edge cases

### v1.0.7 Highlights
- **SRE Hardening:** WAF/CAPTCHA detection (19 signatures), fs2 file locking, OOM protection, TUI panic safety
- **Pure Rust:** Zero FFI dependencies (removed zvec-sys stub, bumpalo dead code)
- **AI Safety:** Fixed P0 bug — `debug_assert_eq!` → `assert_eq!` in `ModelInput::new()` (was silent in --release)
- **Network Hardening:** `connect_timeout(10s)` + `pool_max_idle_per_host` for resilient scraping

---

## 🚀 Quick Start

```bash
# Install tools (one-time)
cargo install cargo-nextest cargo-llvm-cov sccache

# Run tests (4x faster than cargo test)
cargo nextest run

# Run clippy
cargo clippy -- -D warnings

# Run bacon for background checking
bacon
```

---

## 📦 Stack Óptimo 2025-26

| Herramienta | Versión | Propósito |
|-------------|---------|-----------|
| **Rust** | 1.93.0 | Latest stable |
| **cargo-nextest** | 0.9.130 | Test runner (4x faster) |
| **cargo-llvm-cov** | latest | Cobertura nativa LLVM (10x faster) |
| **sccache** | 0.14.0 | Cache de compilación (6x faster) |
| **bacon** | latest | Background checker (replaces cargo-watch) |
| **mold** | latest | Linker (seconds → milliseconds) |

---

## 🛠️ Commands

### Tests

```bash
# Traditional (slow) - NOT RECOMMENDED
cargo test -- --test-threads 2

# Nextest (4x faster) ✅
cargo nextest run

# Run only failed tests
cargo nextest run --failed

# Run ignored tests (real sites)
cargo nextest run --run-ignored ignored-only
```

### Cobertura

```bash
# Tarpaulin (slow, ~5min)
cargo tarpaulin --out Html

# LLVM-Cov (fast, ~30s) ✅
cargo llvm-cov nextest --html --output-dir coverage-llvm
```

### Build

```bash
# Standard build
cargo build --release

# With sccache (6x faster) ✅
sccache --show-stats  # View cache stats
```

### Linting

```bash
# Clippy with warnings as errors ✅
cargo clippy -- -D warnings

# Auto-fix
cargo clippy --fix -- -D warnings
```

### Formatting

```bash
# Check format
cargo fmt --check

# Format code
cargo fmt
```

### Background Checking (Bacon)

```bash
# Run bacon (auto-runs clippy on changes)
bacon

# Custom jobs in bacon.toml:
# t = nextest, f = nextest --failed, c = clippy
```

---

## 📊 Performance Comparison

| Task | Traditional | Optimized 2025-26 | Mejora |
|------|-------------|-------------------|--------|
| **Tests** | `cargo test` (~30s) | `cargo nextest` (~6s) | **~5x** |
| **Coverage** | `tarpaulin` (5min) | `llvm-cov` (30s) | **~10x** |
| **Build** | Clean (60s) | `sccache` (10s) | **~6x** |
| **Linting** | Manual | `bacon` (instant) | **Instant** |

---

## 🔧 Configuration

### nextest.toml

Located at project root. Optimizado para HDD:

```toml
[profile.default]
threads-required = 2
retries = 2
slow-timeout = { period = "60s", terminate-after = 3 }

[profile.ci]
threads-required = 4
retries = 0
```

### bacon.toml

Background checker config:

```toml
summary = true

[keybindings]
t = "nextest"
f = "nextest --failed"
c = "clippy"
r = "build --release"
```

### `.cargo/config.toml`

```toml
[build]
rustc-linker = "clang"
rustc-linker-arg = ["-fuse-ld=mold"]
split-debuginfo = "unpacked"
jobs = 3

[profile.dev]
lto = true
codegen-units = 1

[profile.release]
lto = "fat"
codegen-units = 1
```

### sccache Stats

```bash
# Start server (usually auto-started)
sccache --start-server

# View stats
sccache --show-stats

# Zero stats
sccache --zero-stats
```

---

## 📁 Project Structure

```
rust_scraper/
├── .cargo/
│   └── config.toml          # sccache + mold linker
├── src/
│   ├── application/
│   │   └── http_client.rs   # HttpClient wrapper (Option A)
│   ├── domain/
│   ├── infrastructure/
│   └── adapters/
├── tests/
│   ├── http_client_integration.rs  # Real site tests (ignored)
│   └── ai_integration.rs            # AI tests (feature-gated)
├── docs/
├── nextest.toml             # Test configuration
├── bacon.toml              # Background checker
└── Cargo.toml
```

---

## 🎯 Testing Workflow

### 1. Daily Development

```bash
# Terminal 1: Run bacon (auto-runs clippy + tests)
bacon

# Terminal 2: Edit code → see results instantly
```

### 2. Before Commit

```bash
# Format + lint + test
cargo fmt
cargo clippy -- -D warnings
cargo nextest run
```

### 3. Coverage Check

```bash
# Generate and open coverage report
cargo llvm-cov nextest --html
open coverage-llvm/index.html
```

---

## 🐛 Troubleshooting

### sccache no funciona

```bash
# Verificar servidor
sccache --show-stats

# Reiniciar
sccache --stop-server
sccache --start-server
```

### Nextest falla

```bash
# Limpiar build
cargo clean

# Reintentar
cargo nextest run
```

### Cobertura no genera

```bash
# Limpiar artifacts
cargo clean

# Regenerar
cargo llvm-cov nextest --clean --html
```

## ⚡ Build Performance

### Why the first build takes ~7 minutes

The initial compilation is dominated by heavy crates that compile native code:

| Crate | Time | Reason |
|-------|------|--------|
| `tract-onnx` | ~3 min | ONNX runtime (C++ codegen) |
| `syntect` | ~1-2 min | Oniguruma regex engine (C bindings) |
| `tokenizers` | ~1 min | NLP tokenization |
| `ring` | ~30s | Cryptography (C bindings) |

### Speed it up with sccache

```bash
# Set sccache as the Rust compiler wrapper
export RUSTC_WRAPPER=sccache

# Start the sccache server
sccache --start-server

# Now build — first time is slow, subsequent builds are instant
cargo build --release
```

**Expected improvement:**
- First build: ~7 min (unchanged — must compile everything)
- Rebuild after small change: **~10-30 seconds** (sccache hits)
- Rebuild after `git pull`: **~1-2 min** (only changed crates recompile)

### Build without AI features (saves ~3 minutes)

```bash
# Standard build (no AI/ONNX)
cargo build --release

# With all stable features (images, documents)
cargo build --release --features full
```

### Duplicate dependencies (intentional)

`cargo tree` shows duplicate versions of `selectors`, `dashmap`, `lru`, and `quick-xml`.
This is **expected and unavoidable** — they come from different upstream crates that we all need.
See comments in `Cargo.toml` for details. Do NOT try to unify them.

---

## 📚 Resources

- [cargo-nextest docs](https://nexte.st/)
- [cargo-llvm-cov docs](https://github.com/taiki-e/cargo-llvm-cov)
- [sccache docs](https://github.com/mozilla/sccache)
- [bacon docs](https://dystroy.org/bacon/)
- [Rust 2025-26 Best Practices](https://rust-lang.github.io/api-guidelines/)

---

**Last updated**: 2026-04-04  
**Rust version**: 1.93.0  
**Stack version**: 2025-26 optimal
**Tests**: 361 passing (nextest)
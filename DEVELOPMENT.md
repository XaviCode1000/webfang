# Development Workflow — Rust Scraper

## 🎉 Latest Achievements

**Tests:** **265 passing** (nextest)  
**Status:** ✅ **All tests passing, 0 failing**  
**Version:** v1.0.7 — Indestructible & Lean

### v1.0.7 Highlights
- **SRE Hardening:** WAF/CAPTCHA detection (19 signatures), fs2 file locking, OOM protection, TUI panic safety
- **Pure Rust:** Zero FFI dependencies (removed zvec-sys stub, bumpalo dead code)
- **AI Safety:** Fixed P0 bug — `debug_assert_eq!` → `assert_eq!` in `ModelInput::new()` (was silent in --release)
- **Network Hardening:** `connect_timeout(10s)` + `pool_max_idle_per_host` for resilient scraping
**Version:** v1.0.7 — Indestructible & Lean

### v1.0.7 Highlights
- **SRE Hardening:** WAF/CAPTCHA detection (19 signatures), fs2 file locking, OOM protection, TUI panic safety
- **Pure Rust:** Zero FFI dependencies (removed zvec-sys stub, bumpalo dead code)
- **AI Safety:** Fixed P0 bug — `debug_assert_eq!` → `assert_eq!` in `ModelInput::new()` (was silent in --release)
- **Network Hardening:** `connect_timeout(10s)` + `pool_max_idle_per_host` for resilient scraping
**Version:** v1.0.7 — Indestructible & Lean

### v1.0.7 Highlights
- **SRE Hardening:** WAF/CAPTCHA detection (19 signatures), fs2 file locking, OOM protection, TUI panic safety
- **Pure Rust:** Zero FFI dependencies (removed zvec-sys stub, bumpalo dead code)
- **AI Safety:** Fixed P0 bug — `debug_assert_eq!` → `assert_eq!` in `ModelInput::new()` (was silent in --release)
- **Network Hardening:** `connect_timeout(10s)` + `pool_max_idle_per_host` for resilient scraping
**Version:** v1.0.7 — Indestructible & Lean

### v1.0.7 Highlights
- **SRE Hardening:** WAF/CAPTCHA detection, file locking, OOM protection, TUI safety
- **Pure Rust:** Zero FFI dependencies (removed zvec-sys, bumpalo dead code)
- **AI Safety:** Fixed P0 bug (`debug_assert_eq!` → `assert_eq!` in production)
- **Network:** connect_timeout + pool management for resilient scraping

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

---

## 📚 Resources

- [cargo-nextest docs](https://nexte.st/)
- [cargo-llvm-cov docs](https://github.com/taiki-e/cargo-llvm-cov)
- [sccache docs](https://github.com/mozilla/sccache)
- [bacon docs](https://dystroy.org/bacon/)
- [Rust 2025-26 Best Practices](https://rust-lang.github.io/api-guidelines/)

---

**Last updated**: 2026-03-31  
**Rust version**: 1.93.0  
**Stack version**: 2025-26 optimal
**Tests**: 265 passing (nextest)
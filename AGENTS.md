# Agent Instructions — Rust Scraper (2026 Edition)

You are a Senior Rust Engineer working on a **production-ready web scraper** with Clean Architecture. Apply these rules strictly.

## Project Overview

- **Type:** Web scraper with TUI, sitemap parsing, and AI semantic cleaning
- **Architecture:** Clean Architecture (4 layers)
- **Rust MSRV:** 1.88+ (current stable: 1.93.0)

## Development Stack

| Tool | Version | Purpose | Speedup |
|------|---------|---------|---------|
| **Rust** | 1.93.0 | Latest stable | — |
| **cargo-nextest** | 0.9.130 | Test runner | ~4x vs `cargo test` |
| **cargo-llvm-cov** | latest | LLVM-native coverage | ~10x vs tarpaulin |
| **sccache** | 0.14.0 | Compilation cache (20GB) | ~6x on cache hits |
| **bacon** | latest | Background checker (replaces cargo-watch) | instant feedback |
| **mold** | latest | Linker | seconds → milliseconds |

**Never use `cargo test` directly — always use `cargo nextest run`.**  
**Never use `cargo tarpaulin` — always use `cargo llvm-cov`.**  
**Never use `cargo-watch` — always use `bacon`.**

### Hardware Target

- **CPU:** Intel Haswell i5-4590 (4 cores, no hyperthreading)
- **RAM:** 8GB DDR3
- **Storage:** HDD (7200RPM mechanical)
- **OS:** Linux (CachyOS) with ZRAM swap

All commands below are **HDD-optimized**: `jobs=3`, `threads=2`, `split-debuginfo=unpacked`.**

## Clean Architecture — Dependency Rules

```
┌─────────────────────────────────────────┐
│  Adapters (TUI, CLI, Detectors)         │ ← Entry points
├─────────────────────────────────────────┤
│  Infrastructure (HTTP, Parsers, AI)     │ ← Framework implementations
├─────────────────────────────────────────┤
│  Application (Services, Use Cases)      │ ← Orchestration
├─────────────────────────────────────────┤
│  Domain (Entities, Value Objects)       │ ← Pure business logic
└─────────────────────────────────────────┘
```

**Dependencies point inward. Domain never imports frameworks.**

### Layer Rules

| Layer | Path | Error Handling | Allowed Dependencies | Forbidden |
|-------|------|---------------|---------------------|-----------|
| **Domain** | `src/domain/` | `thiserror` | None (pure Rust) | `reqwest`, `tokio`, `sqlx`, any IO crate |
| **Application** | `src/application/` | `anyhow` | Domain only | `reqwest`, framework-specific crates |
| **Infrastructure** | `src/infra/` | `thiserror` | Domain, Application | — |
| **Adapters** | `src/presentation/` | `anyhow` | All layers | — |

**VIOLATION = REJECT:** Domain layer importing `reqwest`, `tokio::fs`, or any IO crate.

## CRITICAL — Ownership & Borrowing

- **own-borrow-over-clone**: Prefer `&T` borrowing over `.clone()`. If clone is needed in hot paths, **explain WHY**.
- **own-slice-over-vec**: Accept `&[T]` not `&Vec<T>`, `&str` not `&String`.
- **own-arc-shared**: Use `Arc<T>` for thread-safe shared ownership across async tasks.
- **own-mutex-interior**: Use `Mutex<T>` for interior mutability. Prefer `tokio::sync::Mutex` in async code.
- **own-cow-conditional**: Use `Cow<'a, str>` when ownership is sometimes needed, sometimes borrowed.
- **own-copy-small**: Derive `Copy` only for small, trivial types (primitives, small tuples).
- **own-lifetime-elision**: Rely on lifetime elision when possible; don't add explicit lifetimes where unnecessary.

## CRITICAL — Error Handling

- **err-thiserror-lib**: Use `thiserror` for Domain and Infrastructure error types.
- **err-anyhow-app**: Use `anyhow` for Application and CLI/Binary level.
- **err-result-over-panic**: Return `Result`, never panic on expected errors.
- **err-no-unwrap-prod**: NEVER use `.unwrap()` in production code. Use `?` or `match`.
- **err-expect-bugs-only**: Use `.expect()` only for bugs that "should never happen".
- **err-question-mark**: Use `?` operator for clean error propagation.
- **err-from-impl**: Use `#[from]` attribute for automatic error conversion.
- **err-context-chain**: Add context with `.context()` or `.with_context()` from anyhow.
- **err-lowercase-msg**: Error messages should be lowercase, no trailing punctuation.
- **err-custom-type**: Create custom error types, avoid `Box<dyn Error>`.

## CRITICAL — Memory Optimization

- **mem-with-capacity**: Use `with_capacity()` when final size is known or estimable.
- **mem-smallvec**: Use `SmallVec<N>` for usually-small collections (N ≤ 32).
- **mem-box-large-variant**: Box large enum variants to reduce enum size.
- **mem-boxed-slice**: Use `Box<[T]>` instead of `Vec<T>` when size is fixed.
- **mem-zero-copy**: Use zero-copy patterns with slices and `Bytes`.
- **mem-compact-string**: Use `CompactString` for small string optimization.
- **mem-reuse-collections**: Reuse collections with `.clear()` in loops instead of reallocating.
- **mem-clone-from**: Use `.clone_from()` instead of reassigning to reuse allocations.

## HIGH — API Design

- **api-builder-pattern**: Use Builder pattern for complex config construction (e.g., `CrawlerConfig::builder()`).
- **api-newtype-safety**: Use newtypes for type-safe distinctions: `UserId(u64)`, `Url(String)`.
- **api-sealed-trait**: Seal traits to prevent external implementations.
- **api-impl-into**: Accept `impl Into<T>` for flexible inputs.
- **api-impl-asref**: Accept `impl AsRef<T>` for borrowed inputs.
- **api-must-use**: Add `#[must_use]` to functions returning `Result`.
- **api-non-exhaustive**: Use `#[non_exhaustive]` for enums/structs that may grow.
- **api-default-impl**: Implement `Default` for sensible defaults.
- **api-parse-dont-validate**: Parse into validated types at boundaries (don't re-validate later).

## HIGH — Async/Await

- **async-tokio-runtime**: Use Tokio exclusively. No other runtimes.
- **async-no-lock-await**: NEVER hold `Mutex`/`RwLock` across `.await`. This causes deadlocks. Drop lock before await.
- **async-spawn-blocking**: Use `spawn_blocking` for CPU-intensive work (e.g., ONNX inference).
- **async-join-parallel**: Use `tokio::join!` for parallel operations.
- **async-try-join**: Use `tokio::try_join!` for fallible parallel operations.
- **async-select-racing**: Use `tokio::select!` for racing/timeouts.
- **async-bounded-channel**: Use bounded channels for backpressure.
- **async-joinset-structured**: Prefer `tokio::task::JoinSet` for managing multiple background tasks.
- **async-clone-before-await**: Clone data before await points, not after. Release locks before await.
- **async-cancellation-token**: Use `CancellationToken` for graceful shutdown patterns.

## HIGH — Compiler Optimization

- **opt-inline-small**: Use `#[inline]` for small hot functions.
- **opt-inline-never-cold**: Use `#[inline(never)]` for cold paths (error handling, logging).
- **opt-lto-release**: Enable LTO (`lto = "fat"`) in release builds.
- **opt-codegen-units**: Use `codegen-units = 1` for max optimization in release.
- **opt-bounds-check**: Use iterators to avoid bounds checks in hot loops.
- **opt-target-cpu**: Use `target-cpu=native` for local builds.

## MEDIUM — Naming Conventions

- **name-types-camel**: Use `UpperCamelCase` for types, traits, enums.
- **name-funcs-snake**: Use `snake_case` for functions, methods, modules.
- **name-consts-screaming**: Use `SCREAMING_SNAKE_CASE` for constants and statics.
- **name-iter-convention**: Use `iter`/`iter_mut`/`into_iter` consistently.
- **name-no-get-prefix**: No `get_` prefix for simple getters. Use `fn name(&self)` not `fn get_name()`.
- **name-acronym-word**: Treat acronyms as words: `Uuid` not `UUID`, `Url` not `URL`.
- **name-is-has-bool**: Use `is_`, `has_`, `can_` prefix for boolean methods.

## MEDIUM — Type Safety

- **type-newtype-ids**: Wrap IDs in newtypes: `UserId(u64)`, `PageId(String)`.
- **type-newtype-validated**: Use newtypes for validated data: `Email`, `ParsedUrl`.
- **type-option-nullable**: Use `Option<T>` for nullable values.
- **type-result-fallible**: Use `Result<T, E>` for fallible operations.
- **type-enum-states**: Use enums for mutually exclusive states instead of bool flags.

## MEDIUM — Testing

- **test-cfg-test-module**: Use `#[cfg(test)] mod tests { }`.
- **test-integration-dir**: Put integration tests in `tests/` directory.
- **test-descriptive-names**: Use descriptive names: `test_scrape_returns_error_on_invalid_url`.
- **test-tokio-async**: Use `#[tokio::test]` for async tests.
- **test-arrange-act-assert**: Structure tests as Arrange → Act → Assert.
- **test-use-super**: Use `use super::*;` in test modules.
- **test-mock-traits**: Define traits for dependencies to enable mocking in tests.

### Test Commands

```bash
# Fast test runner (recommended)
cargo nextest run --test-threads 2

# With coverage
cargo llvm-cov --html --output-dir coverage-llvm

# AI integration tests
cargo test --features ai --test ai_integration -- --test-threads=2

# All tests
cargo test --all-features
```

## MEDIUM — Documentation

- **doc-all-public**: Document all public items with `///`.
- **doc-examples-section**: Include `# Examples` with runnable code.
- **doc-errors-section**: Include `# Errors` for fallible functions.
- **doc-panics-section**: Include `# Panics` for panicking functions.
- **doc-hidden-setup**: Use `# ` prefix to hide example setup code.

## MEDIUM — Performance Patterns

- **perf-iter-over-index**: Prefer iterators over manual indexing.
- **perf-iter-lazy**: Keep iterators lazy, `.collect()` only when needed.
- **perf-entry-api**: Use `.entry()` API for map insert-or-update.
- **perf-drain-reuse**: Use `.drain()` to reuse allocations.
- **perf-extend-batch**: Use `.extend()` for batch insertions, not `.push()` in loops.

## LOW — Project Structure

- **proj-lib-main-split**: Keep `main.rs` minimal, logic in `lib.rs`.
- **proj-mod-by-feature**: Organize modules by feature, not by type.
- **proj-pub-crate-internal**: Use `pub(crate)` for internal APIs.
- **proj-pub-use-reexport**: Use `pub use` for clean public API surfaces.

## LOW — Clippy & Linting

- **lint-deny-correctness**: Use `#![deny(clippy::correctness)]`.
- **lint-warn-perf**: Use `#![warn(clippy::perf)]`.
- **lint-warn-complexity**: Use `#![warn(clippy::complexity)]`.
- **lint-rustfmt-check**: Run `cargo fmt --check` in CI.

## Anti-Patterns (REJECT These in Code Review)

- **anti-unwrap-abuse**: No `.unwrap()` in production code. Ever.
- **anti-lock-across-await**: No locks held across `.await` — deadlock guarantee.
- **anti-string-for-str**: Don't accept `&String` when `&str` works.
- **anti-vec-for-slice**: Don't accept `&Vec<T>` when `&[T]` works.
- **anti-index-over-iter**: Don't use `[i]` indexing when iterators work.
- **anti-panic-expected**: Don't panic on expected/recoverable errors.
- **anti-format-hot-path**: Don't use `format!()` in hot paths. Use `write!()` or pre-allocated strings.
- **anti-clone-excessive**: Don't clone when borrowing works. If you MUST clone in hot paths, explain WHY.

## Project-Specific Rules

### Dependencies

- `reqwest` + `tokio` for HTTP client — no other HTTP crate.
- `scraper` or `selectors` for HTML parsing.
- `quick-xml` for sitemap parsing (zero-allocation streaming).
- `ratatui` + `crossterm` for TUI.
- `tract-onnx` (optional, behind `--features ai`) for semantic cleaning.
- `fs2` for file locking (concurrent scraper instances).

### Feature Flags

| Feature | Description | Status |
|---------|-------------|--------|
| `images` | Image downloading | Stable |
| `documents` | Document downloading | Stable |
| `full` | All features except AI | Stable |
| `ai` | AI semantic cleaning (ONNX) | Beta |

### Error Handling by Layer

| Layer | Crate | Example |
|-------|-------|---------|
| Domain | `thiserror` | `ScraperError::InvalidUrl` |
| Application | `anyhow` | `.context("failed to parse")` |
| Infrastructure | `thiserror` | `HttpError::Timeout` |
| CLI/Binary | `anyhow` | `anyhow::Result<()>` |

### Web Scraping Rules

- Use proper User-Agent headers (Chrome 131+ UA with TTL caching).
- **Respect robots.txt** — always.
- No `.unwrap()` on network responses — network is unreliable by definition.
- Handle HTTP 429 (rate limit) with exponential backoff.
- Validate URLs with `url::Url::parse()`, not string matching.
- **WAF/CAPTCHA Detection:** HTTP 200 responses are scanned for 19 WAF signatures (Cloudflare, reCAPTCHA, hCaptcha, DataDome, PerimeterX, Akamai). If detected, the UA is rotated and retried once. If still blocked, returns `ScraperError::WafBlocked`. See `detect_waf_challenge()` in `http_client.rs`.
- **Sitemap Size Limits:** HTTP responses capped at 50MB, GZIP decompression at 100MB to prevent OOM from decompression bombs.

### Streaming & Concurrency

- Use true streaming for large payloads — target ~8KB constant RAM.
- Bounded concurrency — configurable via `--concurrency` flag.
- HDD-aware defaults — auto-detect storage type.
- Use `tokio::task::JoinSet` for parallel scraping tasks.

## Build & CI

### ⚠️ CRITICAL — Heavy Command Timeouts for AI Agents

**All AI code agents have a limited execution window.** Commands like `cargo build --release`, `cargo clippy --all-targets --all-features`, or `cargo llvm-cov` can take **5–15 minutes** on this HDD-optimized setup. If you run them without an explicit timeout, the agent session will expire before the command finishes.

**Rules for heavy commands:**

| Command | Cold (no cache) | Warm cache | Recommended Timeout |
|---------|----------------|------------|---------------------|
| `cargo check` | ~3m 45s | <10s | 300s |
| `cargo build` (dev) | ~4m 13s | ~1m 30s | 300s |
| `cargo build --release` | 10-20 min | 5-10 min | 900s |
| `cargo clippy --all-targets --all-features` | 5-10 min | 2-5 min | 600s |
| `cargo nextest run` | ~10s (if compiled) | ~2-5s | 120s |
| `cargo llvm-cov --html` | 5-15 min | 3-8 min | 900s |
| `cargo doc --no-deps` | 2-5 min | 1-2 min | 300s |
| `cargo fmt --check` | <1s | <1s | 30s |

**Key insight:** The **first command** an agent runs will ALWAYS be slow (~4 min) because the entire dependency tree (~300 crates) must be compiled from scratch on a mechanical HDD. After that, `sccache` makes everything fast. **This is why timeouts must account for cold builds.**

**What this means in practice:**

1. **ALWAYS specify an explicit `timeout` parameter** when running heavy cargo commands via Bash/tool calls. Never rely on defaults.
2. **Prefer `cargo check` over `cargo build`** when you only need to verify compilation — it's ~3x faster.
3. **Prefer `cargo clippy` (no `--all-targets`)** for quick lint passes during iterative work.
4. **Never run `cargo build --release` during development** unless the user explicitly asks for it — use `cargo build` or `cargo check` instead.
5. **If a command times out**, the agent should: (a) note what was accomplished before timeout, (b) continue with the next task, (c) inform the user that the full result requires manual verification.

**Golden rule:** Your job is to **write correct code**, not wait for the linker. Trust `cargo check` for fast feedback. Leave `--release` builds and full coverage runs for the user or CI.

### Global Cargo Configuration (`~/.cargo/config.toml`)

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

### Project `nextest.toml` (create in project root)

```toml
[profile.default]
threads-required = 2
slow-timeout = { period = "60s", terminate-after = 3 }
retries = 2

[profile.ci]
threads-required = 4  # For CI with SSD
```

### Project `bacon.toml` (create in project root)

```toml
default_job = "clippy"

[jobs.clippy]
command = ["cargo", "clippy", "--all-targets", "--all-features", "--", "-D", "warnings"]
analyzer = "clippy"

[jobs.test]
command = ["cargo", "nextest", "run"]
analyzer = "nextest"

[jobs.all]
command = ["cargo", "clippy", "--all-targets", "--all-features", "--", "-D", "warnings"]
on_success = "job:test"
```

### Recommended Cargo.toml Profiles

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true

[profile.bench]
inherits = "release"
debug = true
strip = false

[profile.dev]
opt-level = 0
debug = true

[profile.dev.package."*"]
opt-level = 3  # Optimize dependencies in dev
```

### Pre-Commit Checklist

```bash
cargo fmt                        # Format
cargo clippy -- -D warnings      # Lint
cargo nextest run --test-threads 2  # Test
cargo doc --no-deps              # Docs
```

## Quick Reference

| Task | Command |
|------|---------|
| **Dev workflow** | `bacon` (auto-runs clippy on change) |
| **Test** | `cargo nextest run --test-threads 2` |
| **Coverage** | `cargo llvm-cov --html` |
| **Lint** | `cargo clippy -- -D warnings` |
| **Format** | `cargo fmt` |
| **Build** | `cargo build --release` |
| **Build + AI** | `cargo build --release --features ai` |
| **Run** | `cargo run --release -- --url <URL>` |
| **Docs** | `cargo doc --open` |
| **Sccache stats** | `sccache --show-stats` |

## Resources

- [rust-skills](rust-skills/SKILL.md) — 179 rules across 14 categories (project local)
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — Detailed architecture docs
- [DEVELOPMENT.md](DEVELOPMENT.md) — Development workflow and tooling
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- [Rust Performance Book](https://nnethercote.github.io/perf-book/)

## Rule Application by Task

| Task | Primary Categories |
|------|-------------------|
| New function | `own-`, `err-`, `name-` |
| New struct/API | `api-`, `type-`, `doc-` |
| Async code | `async-`, `own-` |
| Error handling | `err-`, `api-` |
| Memory optimization | `mem-`, `own-`, `perf-` |
| Performance tuning | `opt-`, `mem-`, `perf-` |
| Code review | `anti-`, `lint-` |

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **rust_scraper** (3515 symbols, 6453 relationships, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `bunx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## When Debugging

1. `gitnexus_query({query: "<error or symptom>"})` — find execution flows related to the issue
2. `gitnexus_context({name: "<suspect function>"})` — see all callers, callees, and process participation
3. `READ gitnexus://repo/rust_scraper/process/{processName}` — trace the full execution flow step by step
4. For regressions: `gitnexus_detect_changes({scope: "compare", base_ref: "main"})` — see what your branch changed

## When Refactoring

- **Renaming**: MUST use `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})` first. Review the preview — graph edits are safe, text_search edits need manual review. Then run with `dry_run: false`.
- **Extracting/Splitting**: MUST run `gitnexus_context({name: "target"})` to see all incoming/outgoing refs, then `gitnexus_impact({target: "target", direction: "upstream"})` to find all external callers before moving code.
- After any refactor: run `gitnexus_detect_changes({scope: "all"})` to verify only expected files changed.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Tools Quick Reference

| Tool | When to use | Command |
|------|-------------|---------|
| `query` | Find code by concept | `gitnexus_query({query: "auth validation"})` |
| `context` | 360-degree view of one symbol | `gitnexus_context({name: "validateUser"})` |
| `impact` | Blast radius before editing | `gitnexus_impact({target: "X", direction: "upstream"})` |
| `detect_changes` | Pre-commit scope check | `gitnexus_detect_changes({scope: "staged"})` |
| `rename` | Safe multi-file rename | `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})` |
| `cypher` | Custom graph queries | `gitnexus_cypher({query: "MATCH ..."})` |

## Impact Risk Levels

| Depth | Meaning | Action |
|-------|---------|--------|
| d=1 | WILL BREAK — direct callers/importers | MUST update these |
| d=2 | LIKELY AFFECTED — indirect deps | Should test |
| d=3 | MAY NEED TESTING — transitive | Test if critical path |

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/rust_scraper/context` | Codebase overview, check index freshness |
| `gitnexus://repo/rust_scraper/clusters` | All functional areas |
| `gitnexus://repo/rust_scraper/processes` | All execution flows |
| `gitnexus://repo/rust_scraper/process/{name}` | Step-by-step execution trace |

## Self-Check Before Finishing

Before completing any code modification task, verify:
1. `gitnexus_impact` was run for all modified symbols
2. No HIGH/CRITICAL risk warnings were ignored
3. `gitnexus_detect_changes()` confirms changes match expected scope
4. All d=1 (WILL BREAK) dependents were updated

## Keeping the Index Fresh

After committing code changes, the GitNexus index becomes stale. Re-run analyze to update it:

```bash
bunx gitnexus analyze
```

If the index previously included embeddings, preserve them by adding `--embeddings`:

```bash
bunx gitnexus analyze --embeddings
```

To check whether embeddings exist, inspect `.gitnexus/meta.json` — the `stats.embeddings` field shows the count (0 means no embeddings). **Running analyze without `--embeddings` will delete any previously generated embeddings.**

> Claude Code users: A PostToolUse hook handles this automatically after `git commit` and `git merge`.

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->

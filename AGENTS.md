# Agent Instructions — Rust Scraper

Production-ready web scraper with Clean Architecture, TUI, and AI semantic cleaning.

**Stack:** Rust 1.88+ · Tokio · wreq (TLS fingerprint) · ratatui · tract-onnx (feature-gated)
**Hardware:** Intel i5-4590 (4C), 8GB DDR3, HDD — all commands are HDD-optimized

---

## Key Commands

```bash
cargo check                                          # Verify compilation (FAST — use this)
cargo check --features ai                            # Verify with AI feature
cargo clippy -- -D warnings                          # Lint (quick pass)
cargo clippy --all-targets --all-features -- -D warnings  # Full lint
cargo nextest run --test-threads 2                   # Run tests
cargo nextest run --test-threads 2 --features ai     # Tests with AI
cargo llvm-cov --html --output-dir coverage-llvm     # Coverage
cargo fmt --check                                    # Format check
bacon                                                # Background checker (auto-runs clippy)
```

**⚠️ HDD timeout rules:** First `cargo check` takes ~4 min (cold compile, 300 crates). After that, `sccache` makes everything fast. **ALWAYS set explicit timeouts** for heavy commands. Prefer `cargo check` over `cargo build` during development. Never run `cargo build --release` unless explicitly asked.

---

## Non-Obvious Patterns

### Crate version conflicts (DO NOT try to unify)
- `dashmap` 5.x (via governor) + 6.x (direct) — both needed
- `quick-xml` 0.37 (direct) + 0.38 (via syntect→plist) — both needed
- `scraper` 0.22 → selectors 0.26, `legible` → dom_query → selectors 0.35 — both needed

### HTTP client: `wreq` not `reqwest`
Uses TLS fingerprint emulation (Chrome 131) for WAF evasion. Layer 2 evasion built in.

### WAF detection on HTTP 200
Responses are scanned for 19 WAF signatures (Cloudflare, reCAPTCHA, hCaptcha, DataDome, PerimeterX, Akamai). If detected, UA is rotated and retried once. Still blocked → `ScraperError::WafBlocked`.

### AI feature (`--features ai`)
- Loads ~90MB ONNX model (all-MiniLM-L6-v2) into memory
- `SemanticCleanerImpl::new()` is **async** — loads model once, reuses
- `cleaner.clean(html)` is **async** — returns `Vec<DocumentChunk>` with embeddings
- One page → multiple chunks when AI cleaning is active
- Model cached in `~/.cache/rust-scraper/models/`

---

## Boundaries

### ✅ Always
- Run `cargo check` before marking any task complete
- Run `cargo clippy -- -D warnings` before committing
- Use `cargo nextest run` (never `cargo test`)
- Use `cargo llvm-cov` (never `cargo tarpaulin`)
- Use `bacon` for background checking (never `cargo-watch`)

### ⚠️ Ask first
- Adding or removing dependencies
- Changing feature flag structure
- Modifying `Cargo.toml` profiles

### 🚫 Never
- Commit secrets, `.env` files, or credentials
- Use `.unwrap()` in production code — use `?` or `match`
- Force push to main or protected branches
- Modify `target/`, `dist/`, or `build/` directories
- Run `cargo build --release` during development (use `cargo check`)

---

## Skills

| Skill | Location | Trigger |
|-------|----------|---------|
| **rust-skills** | `~/.config/opencode/skills/rust-skills/SKILL.md` | Any Rust code (179 rules) |
| **gitnexus-exploring** | `~/.config/opencode/skill/gitnexus-exploring/` | "How does X work?" |
| **gitnexus-impact-analysis** | `~/.config/opencode/skill/gitnexus-impact-analysis/` | "What breaks if I change X?" |
| **gitnexus-debugging** | `~/.config/opencode/skill/gitnexus-debugging/` | "Why is X failing?" |
| **gitnexus-refactoring** | `~/.config/opencode/skill/gitnexus-refactoring/` | Rename, extract, split, move |
| **gitnexus-cli** | `~/.config/opencode/skill/gitnexus-cli/` | Index, status, clean, wiki |
| **gitnexus-guide** | `~/.config/opencode/skill/gitnexus-guide/` | Tools, schema, resources |

> Index: `bunx gitnexus analyze` · Status: `bunx gitnexus status`

---

## Resources

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — Architecture details
- [DEVELOPMENT.md](DEVELOPMENT.md) — Dev workflow and tooling

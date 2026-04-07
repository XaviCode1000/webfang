# CLI Reference — rust-scraper

**Version:** 1.1.0  
**MSRV:** 1.88  
**Last Updated:** 2026-04-04

---

## Quick Start

```bash
# Basic scraping (default: markdown output, 10 pages, 1s delay)
cargo run -- --url "https://example.com"

# Scrape with JSON output
cargo run -- --url "https://example.com" -f json

# Scrape for RAG pipeline (JSONL format)
cargo run -- --url "https://example.com" --export-format jsonl

# Interactive mode with TUI selector
cargo run -- --url "https://example.com" --interactive

# Quick-save to Obsidian vault
cargo run -- --url "https://example.com" --obsidian-wiki-links --quick-save
```

---

## Required Arguments

| Flag | Description | Required |
|------|-------------|----------|
| `-u, --url <URL>` | Target URL to scrape (must include `http://` or `https://`) | ✅ Yes (unless using subcommand) |

**Note:** `--url` is not required when using subcommands like `completions bash`.

---

## Output Options

### Individual File Output (`-f, --format`)

Creates separate output files per scraped page — ideal for human-readable output.

| Flag | Values | Default | Description |
|------|--------|---------|-------------|
| `-f, --format <FORMAT>` | `markdown`, `json`, `text` | `markdown` | Output format for individual files |

**Formats:**

| Format | Description | Use Case |
|--------|-------------|----------|
| `markdown` | Markdown with YAML frontmatter | RAG, documentation, human-readable |
| `json` | Structured JSON with metadata | Programmatic processing |
| `text` | Plain text without formatting | Simple text extraction |

**Example:**
```bash
# Markdown (default)
cargo run -- --url "https://example.com" -f markdown

# JSON output
cargo run -- --url "https://example.com" -f json

# Plain text
cargo run -- --url "https://example.com" -f text
```

### RAG Pipeline Export (`--export-format`)

Creates batch export suitable for LLM/RAG pipelines.

| Flag | Values | Default | Description |
|------|--------|---------|-------------|
| `--export-format <FORMAT>` | `jsonl`, `vector`, `auto` | `jsonl` | Export format for RAG pipeline |

**Formats:**

| Format | Description |
|--------|-------------|
| `jsonl` | JSON Lines (one JSON per line), optimal for RAG |
| `vector` | JSON with metadata header, embeddings support |
| `auto` | Auto-detect from existing export files |

**Example:**
```bash
# JSONL export (default)
cargo run -- --url "https://example.com" --export-format jsonl

# Vector export with embeddings (for vector DB ingestion)
cargo run -- --url "https://example.com" --export-format vector
```

### Output Directory (`-o, --output`)

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-o, --output <DIR>` | Path | `output` | Output directory for scraped content |

---

## Obsidian Integration

### Vault Detection

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--vault <PATH>` | Path | Auto-detect | Explicit Obsidian vault path |

**Detection order:** `--vault` > `$OBSIDIAN_VAULT` env var > config file > auto-scan

### Obsidian Export Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--obsidian-wiki-links` | Boolean | `false` | Convert same-domain links to `[[wiki-link]]` syntax |
| `--obsidian-tags <TAGS>` | Comma-separated | None | Tags for YAML frontmatter (e.g., `"rust,web,scraping"`) |
| `--obsidian-relative-assets` | Boolean | `false` | Rewrite asset paths as relative to `.md` file |
| `--obsidian-rich-metadata` | Boolean | `false` | Add wordCount, readingTime, language, contentType, status to frontmatter |
| `--quick-save` | Boolean | `false` | Bypass TUI, save directly to vault `_inbox/` folder |

**Example:**
```bash
# Quick-save to detected vault
cargo run -- --url "https://example.com" --obsidian-wiki-links --quick-save

# Full Obsidian mode
cargo run -- --url "https://example.com" \
  --vault ~/Obsidian/MyVault \
  --obsidian-wiki-links \
  --obsidian-tags "rust,web" \
  --obsidian-relative-assets \
  --obsidian-rich-metadata \
  --quick-save
```

**Quick-save behavior:**
- Saves to `{vault}/_inbox/YYYY-MM-DD-slug.md`
- Creates `_inbox/` directory if it doesn't exist
- Opens note in Obsidian if running (Linux via `xdg-open`)
- Falls back to `--output` directory if no vault detected

**Environment Variables:**
| Variable | Description |
|----------|-------------|
| `OBSIDIAN_VAULT` | Path to Obsidian vault (used if `--vault` not specified) |

**Config File:**
```toml
# ~/.config/rust-scraper/config.toml
[obsidian]
vault_path = "~/Obsidian/MyVault"
wiki_links = true
relative_assets = true
rich_metadata = true
tags = ["web-clip", "automation"]
```

See [`docs/OBSIDIAN.md`](OBSIDIAN.md) for complete Obsidian documentation.

---

## Scraping Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-s, --selector <SELECTOR>` | String | `body` | CSS selector for content extraction |
| `--max-pages <N>` | Integer | `10` | Maximum pages to scrape |
| `--delay-ms <MS>` | Integer | `1000` | Delay between requests in milliseconds |
| `--concurrency <VALUE>` | `auto` or Integer | `auto` | Concurrency level (parallel requests) |

### CSS Selector (`-s, --selector`)

Extract specific content using CSS selectors:

```bash
# Extract only article content
cargo run -- --url "https://example.com" -s "article"

# Extract main content by ID
cargo run -- --url "https://example.com" -s "#main-content"
```

### Concurrency (`--concurrency`)

Hardware-aware concurrency control:

| Value | Description |
|-------|-------------|
| `auto` (default) | Auto-detect based on CPU cores |
| `1-16` | Explicit concurrency value |

**Auto-detection logic:**
- 1-2 cores: 1 worker
- 3-4 cores: 3 workers (HDD-aware default)
- 5-7 cores: 5 workers
- 8+ cores: `min(cores - 1, 8)` workers

---

## Asset Download

| Flag | Default | Description |
|------|---------|-------------|
| `--download-images` | `false` | Download images (PNG, JPG, GIF, WEBP, SVG, BMP) |
| `--download-documents` | `false` | Download documents (PDF, DOCX, XLSX, PPTX, etc.) |

**Feature Requirements:**
- Requires `--features images` for `--download-images`
- Requires `--features documents` for `--download-documents`
- Or use `--features full` for all features

**Example:**
```bash
# Download images only
cargo run --features images -- --url "https://example.com" --download-images

# Download both images and documents
cargo run --features full -- --url "https://example.com" --download-images --download-documents
```

---

## State Management

| Flag | Default | Description |
|------|---------|-------------|
| `--resume` | `false` | Resume mode - skip URLs already processed |
| `--state-dir <DIR>` | `~/.cache/rust-scraper/state` | Custom state directory for resume mode |

### Resume Mode (`--resume`)

Avoids re-processing URLs already scraped successfully:

```bash
# First run
cargo run -- --url "https://example.com" --max-pages 50 --resume

# Interrupted? Resume from where you left off
cargo run -- --url "https://example.com" --max-pages 50 --resume
```

---

## Sitemap Options

| Flag | Default | Description |
|------|---------|-------------|
| `--use-sitemap` | `false` | Use sitemap for URL discovery |
| `--sitemap-url <URL>` | Auto-discover | Explicit sitemap URL |

### Sitemap Discovery (`--use-sitemap`)

Automatically discovers sitemap from `robots.txt`:

```bash
# Auto-discover sitemap
cargo run -- --url "https://example.com" --use-sitemap

# Explicit sitemap URL
cargo run -- --url "https://example.com" --use-sitemap --sitemap-url "https://example.com/sitemap.xml"
```

---

## Interactive Mode

| Flag | Default | Description |
|------|---------|-------------|
| `--interactive` | `false` | Interactive mode with TUI URL selector |

```bash
cargo run -- --url "https://example.com" --interactive
```

**Features:**
- Interactive checkbox selection of URLs
- Confirmation mode before download
- Terminal restore on panic/exit

---

## CLI UX Options

| Flag | Default | Description |
|------|---------|-------------|
| `--dry-run` | `false` | Print discovered URLs to stdout and exit without scraping |
| `--quiet` | `false` | Suppress progress bars, emojis, and summary output |
| `-v, --verbose...` | 0 | Verbosity level (`-v` info, `-vv` debug, `-vvv` trace) |
| `completions <SHELL>` | — | Generate shell completion scripts |

### Dry-Run Mode (`--dry-run`)

Preview which URLs would be scraped without actually scraping them:

```bash
cargo run -- --url "https://example.com" --dry-run
```

### Quiet Mode (`--quiet`)

Suppress all non-essential output for clean scripting/pipe usage:

```bash
cargo run -- --url "https://example.com" --quiet
```

### Verbosity (`-v, --verbose`)

```bash
# Info level
cargo run -- --url "https://example.com" -v

# Debug level
cargo run -- --url "https://example.com" -vv

# Trace level (most verbose)
cargo run -- --url "https://example.com" -vvv
```

### Shell Completions (`completions`)

```bash
# Bash
cargo run -- completions bash > ~/.local/share/bash-completion/completions/rust-scraper

# Fish
cargo run -- completions fish > ~/.config/fish/completions/rust-scraper.fish

# Zsh
cargo run -- completions zsh > ~/.zsh/completions/_rust-scraper
```

### NO_COLOR Support

```bash
# Disable emojis (ASCII fallback)
NO_COLOR=1 cargo run -- --url "https://example.com"
```

---

## AI Options (Feature-Gated)

| Flag | Default | Description | Feature Required |
|------|---------|-------------|------------------|
| `--clean-ai` | `false` | Use AI-powered semantic cleaning for RAG output | `ai` |

```bash
cargo run --features ai -- --url "https://example.com" --clean-ai
```

---

## JavaScript Rendering (Reserved for v1.4)

| Flag | Default | Description | Status |
|------|---------|-------------|--------|
| `--force-js-render` | `false` | Force JS rendering for SPA sites | ⏳ Reserved (no-op) |

```bash
# Currently has no effect — reserved for v1.4
cargo run -- --url "https://example.com/spa" --force-js-render
```

**Track implementation:** [Issue #16](https://github.com/XaviCode1000/rust-scraper/issues/16)

---

## Crawler Settings

| Flag | Default | Env Var | Description |
|------|---------|---------|-------------|
| `--max-depth <N>` | `2` | `RUST_SCRAPER_MAX_DEPTH` | Maximum crawl depth (0 = only seed URL) |
| `--timeout-secs <N>` | `30` | `RUST_SCRAPER_TIMEOUT_SECS` | Request timeout in seconds |
| `--include-pattern <PATTERN>` | — | `RUST_SCRAPER_INCLUDE` | URL patterns to include (glob-style, comma-separated) |
| `--exclude-pattern <PATTERN>` | — | `RUST_SCRAPER_EXCLUDE` | URL patterns to exclude (glob-style, comma-separated) |

### URL Pattern Filtering

Filter URLs with glob-style patterns:

```bash
# Only scrape blog posts
cargo run -- --url "https://example.com" \
  --include-pattern "*/blog/*,*/posts/*"

# Exclude admin and API pages
cargo run -- --url "https://example.com" \
  --exclude-pattern "*/admin/*,*/api/*"

# Combine include and exclude
cargo run -- --url "https://example.com" \
  --include-pattern "*/docs/*" \
  --exclude-pattern "*/docs/drafts/*"
```

## HTTP Client Settings

| Flag | Default | Env Var | Description |
|------|---------|---------|-------------|
| `--max-retries <N>` | `3` | `RUST_SCRAPER_MAX_RETRIES` | Maximum retry attempts for failed requests |
| `--backoff-base-ms <N>` | `1000` | `RUST_SCRAPER_BACKOFF_BASE_MS` | Base delay for exponential backoff (ms) |
| `--backoff-max-ms <N>` | `10000` | `RUST_SCRAPER_BACKOFF_MAX_MS` | Maximum delay for exponential backoff (ms) |
| `--accept-language <VALUE>` | `en-US,en;q=0.9` | `RUST_SCRAPER_ACCEPT_LANGUAGE` | Accept-Language header value |

### Retry Behavior

Failed requests are retried with exponential backoff:
- Attempt 1: immediate
- Attempt 2: `backoff-base-ms` delay (default 1s)
- Attempt 3: 2× `backoff-base-ms` delay (default 2s)
- Max delay capped at `backoff-max-ms` (default 10s)

## Download Settings

| Flag | Default | Env Var | Description |
|------|---------|---------|-------------|
| `--max-file-size <BYTES>` | `52428800` (50MB) | `RUST_SCRAPER_MAX_FILE_SIZE` | Maximum file size to download |
| `--download-timeout <N>` | `30` | `RUST_SCRAPER_DOWNLOAD_TIMEOUT` | Timeout for individual asset downloads (seconds) |

## AI Settings

> **Requires** `--features ai` at compile time.

| Flag | Default | Env Var | Description |
|------|---------|---------|-------------|
| `--threshold <FLOAT>` | `0.3` | `RUST_SCRAPER_THRESHOLD` | Relevance threshold for semantic filtering (0.0–1.0) |
| `--max-tokens <N>` | `512` | `RUST_SCRAPER_MAX_TOKENS` | Maximum tokens per chunk for AI processing |
| `--offline` | `false` | `RUST_SCRAPER_OFFLINE` | Run AI model in offline mode (fail if not cached) |

```bash
# AI with custom threshold
cargo run --features ai -- --url "https://example.com" \
  --clean-ai --threshold 0.5

# AI offline mode (model must be pre-cached)
cargo run --features ai -- --url "https://example.com" \
  --clean-ai --offline
```

## Sitemap Settings

| Flag | Default | Env Var | Description |
|------|---------|---------|-------------|
| `--sitemap-depth <N>` | `3` | `RUST_SCRAPER_SITEMAP_DEPTH` | Maximum recursion depth for sitemap indexes |

## Environment Variables

**Every CLI flag has a corresponding `RUST_SCRAPER_*` environment variable.**
Precedence: CLI argument > environment variable > config file > defaults.

### Complete Environment Variables Reference

| Env Var | CLI Flag | Default | Description |
|---------|----------|---------|-------------|
| `RUST_SCRAPER_URL` | `--url` | — | Target URL |
| `RUST_SCRAPER_SELECTOR` | `-s` | `body` | CSS selector |
| `RUST_SCRAPER_OUTPUT` | `-o` | `output` | Output directory |
| `RUST_SCRAPER_FORMAT` | `-f` | `markdown` | Output format |
| `RUST_SCRAPER_EXPORT_FORMAT` | `--export-format` | `jsonl` | RAG export format |
| `RUST_SCRAPER_OBSIDIAN_WIKI_LINKS` | `--obsidian-wiki-links` | `false` | Wiki-links conversion |
| `RUST_SCRAPER_OBSIDIAN_TAGS` | `--obsidian-tags` | — | Frontmatter tags |
| `RUST_SCRAPER_OBSIDIAN_RELATIVE_ASSETS` | `--obsidian-relative-assets` | `false` | Relative asset paths |
| `RUST_SCRAPER_OBSIDIAN_VAULT` | `--vault` | auto-detect | Vault path |
| `RUST_SCRAPER_OBSIDIAN_QUICK_SAVE` | `--quick-save` | `false` | Quick-save mode |
| `RUST_SCRAPER_OBSIDIAN_RICH_METADATA` | `--obsidian-rich-metadata` | `false` | Rich frontmatter |
| `RUST_SCRAPER_DELAY_MS` | `--delay-ms` | `1000` | Request delay |
| `RUST_SCRAPER_MAX_PAGES` | `--max-pages` | `10` | Max pages |
| `RUST_SCRAPER_CONCURRENCY` | `--concurrency` | `auto` | Concurrency |
| `RUST_SCRAPER_USE_SITEMAP` | `--use-sitemap` | `false` | Sitemap discovery |
| `RUST_SCRAPER_SITEMAP_URL` | `--sitemap-url` | — | Explicit sitemap |
| `RUST_SCRAPER_RESUME` | `--resume` | `false` | Resume mode |
| `RUST_SCRAPER_STATE_DIR` | `--state-dir` | `~/.cache/...` | State directory |
| `RUST_SCRAPER_DOWNLOAD_IMAGES` | `--download-images` | `false` | Download images |
| `RUST_SCRAPER_DOWNLOAD_DOCUMENTS` | `--download-documents` | `false` | Download documents |
| `RUST_SCRAPER_INTERACTIVE` | `--interactive` | `false` | TUI mode |
| `RUST_SCRAPER_CLEAN_AI` | `--clean-ai` | `false` | AI cleaning (feature-gated) |
| `RUST_SCRAPER_FORCE_JS_RENDER` | `--force-js-render` | `false` | JS rendering (no-op) |
| `RUST_SCRAPER_VERBOSE` | `-v` | `0` | Verbosity level |
| `RUST_SCRAPER_QUIET` | `-q` | `false` | Quiet mode |
| `RUST_SCRAPER_DRY_RUN` | `-n` | `false` | Dry-run mode |
| `RUST_SCRAPER_MAX_DEPTH` | `--max-depth` | `2` | Crawl depth |
| `RUST_SCRAPER_TIMEOUT_SECS` | `--timeout-secs` | `30` | Request timeout |
| `RUST_SCRAPER_INCLUDE` | `--include-pattern` | — | URL include patterns |
| `RUST_SCRAPER_EXCLUDE` | `--exclude-pattern` | — | URL exclude patterns |
| `RUST_SCRAPER_MAX_RETRIES` | `--max-retries` | `3` | Retry attempts |
| `RUST_SCRAPER_BACKOFF_BASE_MS` | `--backoff-base-ms` | `1000` | Backoff base |
| `RUST_SCRAPER_BACKOFF_MAX_MS` | `--backoff-max-ms` | `10000` | Backoff max |
| `RUST_SCRAPER_ACCEPT_LANGUAGE` | `--accept-language` | `en-US,en;q=0.9` | Accept-Language header |
| `RUST_SCRAPER_MAX_FILE_SIZE` | `--max-file-size` | `52428800` | Max download size (bytes) |
| `RUST_SCRAPER_DOWNLOAD_TIMEOUT` | `--download-timeout` | `30` | Download timeout (sec) |
| `RUST_SCRAPER_THRESHOLD` | `--threshold` | `0.3` | AI relevance threshold |
| `RUST_SCRAPER_MAX_TOKENS` | `--max-tokens` | `512` | AI max tokens per chunk |
| `RUST_SCRAPER_OFFLINE` | `--offline` | `false` | AI offline mode |
| `RUST_SCRAPER_SITEMAP_DEPTH` | `--sitemap-depth` | `3` | Sitemap recursion depth |

### Other Environment Variables

| Env Var | Description |
|---------|-------------|
| `OBSIDIAN_VAULT` | Fallback vault path (checked after `RUST_SCRAPER_OBSIDIAN_VAULT`) |
| `XDG_CACHE_HOME` | Base cache directory (state store, models) |
| `RUST_LOG` | Override tracing log level (e.g., `rust_scraper=debug`) |
| `NO_COLOR` | Disable emojis and color output |

---

## Complete Examples

### 1. Basic Scraping
```bash
cargo run -- --url "https://example.com"
```

### 2. Obsidian Quick-Save
```bash
cargo run -- --url "https://example.com" --obsidian-wiki-links --obsidian-rich-metadata --quick-save
```

### 3. RAG Pipeline with AI Cleaning
```bash
cargo run --features ai -- --url "https://example.com" --clean-ai --export-format jsonl
```

### 4. Asset Downloads
```bash
cargo run --features full -- --url "https://example.com" --download-images --download-documents
```

### 5. Production Dataset
```bash
cargo run --features full -- \
  --url "https://example.com" \
  --export-format jsonl \
  --download-images \
  --delay-ms 2000 \
  --max-pages 100 \
  --concurrency 3 \
  --resume \
  -o ./production-dataset \
  -vv
```

### 6. Interactive Mode with Sitemap
```bash
cargo run -- --url "https://example.com" --use-sitemap --interactive
```

---

## Feature Flags

| Feature | Description | Enables |
|---------|-------------|---------|
| `images` | Image downloading support | `mime-type-detector` |
| `documents` | Document downloading support | `mime-type-detector` |
| `ai` | AI semantic cleaning | `ort`, `tokenizers`, `tract-onnx`, etc. |
| `full` | All features | `images`, `documents` |

```bash
# Enable all features
cargo run --features full -- --url "https://example.com" --download-images --download-documents
```

---

## Exit Codes

| Code | Constant | Description |
|------|----------|-------------|
| `0` | `EX_OK` | Success — all URLs scraped without errors |
| `64` | `EX_USAGE` | Invalid arguments or URL parsing error |
| `69` | `EX_UNAVAILABLE` | Network error or partial success |
| `74` | `EX_IOERR` | I/O error — failed to write output files |
| `76` | `EX_PROTOCOL` | Protocol error — TUI failure, WAF blocked |
| `78` | `EX_CONFIG` | Configuration error |

---

## Troubleshooting

### Invalid URL Error
```bash
# ❌ Wrong (missing protocol)
cargo run -- --url "example.com"

# ✅ Correct
cargo run -- --url "https://example.com"
```

### Feature Not Enabled
```bash
# ❌ Wrong
cargo run -- --url "https://example.com" --download-images

# ✅ Correct
cargo run --features images -- --url "https://example.com" --download-images
```

### Network Timeouts
```bash
cargo run -- --url "https://example.com" --delay-ms 3000 --concurrency 1
```

---

**Last Verified:** 2026-04-04 with `cargo run -- --help`  
**rust-scraper** v1.1.0 — Production-ready web scraper with Clean Architecture

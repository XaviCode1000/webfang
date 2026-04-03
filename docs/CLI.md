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
```

---

## Usage

```bash
rust-scraper [OPTIONS] --url <URL>
```

---

## Required Arguments

| Flag | Description | Required |
|------|-------------|----------|
| `-u, --url <URL>` | Target URL to scrape (must include `http://` or `https://`) | ✅ Yes |

**Example:**
```bash
cargo run -- --url "https://example.com"
```

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

##### RAG Pipeline Export (`--export-format`)

Creates batch export suitable for LLM/RAG pipelines.

| Flag | Values | Default | Description |
|------|--------|---------|-------------|
| `--export-format <FORMAT>` | `jsonl`, `vector`, `auto` | `jsonl` | Export format for RAG pipeline |

**Formats:**

| Format | Description | Feature Required |
|--------|-------------|------------------|
| `jsonl` | JSON Lines (one JSON per line), optimal for RAG | None (default) |
| `vector` | JSON with metadata header, embeddings support | None |
| `auto` | Auto-detect from existing export files | None |

**Example:**
```bash
# JSONL export (default)
cargo run -- --url "https://example.com" --export-format jsonl

# Vector export with embeddings (for vector DB ingestion)
cargo run -- --url "https://example.com" --export-format vector

# Auto-detect format
cargo run -- --url "https://example.com" --export-format auto
```

### Output Directory (`-o, --output`)

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-o, --output <DIR>` | Path | `output` | Output directory for scraped content |

**Example:**
```bash
cargo run -- --url "https://example.com" -o ./my-scrapes
```

### Obsidian Integration (v1.1.0+)

#### Vault Detection

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--vault <PATH>` | Path | Auto-detect | Explicit Obsidian vault path |

**Detection order:** `--vault` > `$OBSIDIAN_VAULT` env var > config file > auto-scan

#### Obsidian Export Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--obsidian-wiki-links` | Boolean | `false` | Convert same-domain links to `[[wiki-link]]` syntax |
| `--obsidian-tags <TAGS>` | Comma-separated | None | Tags for YAML frontmatter (e.g., `"rust,web,scraping"`) |
| `--obsidian-relative-assets` | Boolean | `false` | Rewrite asset paths as relative to `.md` file |
| `--quick-save` | Boolean | `false` | Bypass TUI, save directly to vault inbox |

**Example:**
```bash
# Quick-save to detected vault
cargo run -- --url "https://example.com" --obsidian --quick-save

# Full Obsidian mode
cargo run -- --url "https://example.com" \
  --vault ~/Obsidian/MyVault \
  --obsidian-wiki-links \
  --obsidian-tags "rust,web" \
  --obsidian-relative-assets \
  --quick-save
```

**Quick-save behavior:**
- Saves to `{vault}/_inbox/YYYY-MM-DD-slug.md`
- Creates `_inbox/` directory if it doesn't exist
- Opens note in Obsidian if running (Linux)
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

# Extract by class
cargo run -- --url "https://example.com" -s ".content-body"
```

### Page Limit (`--max-pages`)

Control how many pages to scrape:

```bash
# Scrape only 5 pages
cargo run -- --url "https://example.com" --max-pages 5

# Scrape up to 100 pages
cargo run -- --url "https://example.com" --max-pages 100
```

### Request Delay (`--delay-ms`)

Rate limiting to avoid overwhelming servers:

```bash
# 2 second delay between requests
cargo run -- --url "https://example.com" --delay-ms 2000

# Fast scraping (500ms delay)
cargo run -- --url "https://example.com" --delay-ms 500
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

**Example:**
```bash
# Auto-detect (default)
cargo run -- --url "https://example.com" --concurrency auto

# Explicit concurrency
cargo run -- --url "https://example.com" --concurrency 5

# Single-threaded (safe for slow networks)
cargo run -- --url "https://example.com" --concurrency 1
```

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

# Download documents only
cargo run --features documents -- --url "https://example.com" --download-documents

# Download both
cargo run --features full -- --url "https://example.com" --download-images --download-documents
```

**Output Structure:**
```
output/
├── images/
│   ├── 027e504eabfc.png
│   ├── 0c2f4f0301fe.png
│   └── e15cbdd2d653.svg
└── documents/
    └── 9870371a7a8c.pdf
```

**Asset Download Features:**
- **MIME Detection:** Automatic detection from URL extension
- **File Size Limit:** 50MB maximum per file
- **Timeout:** 30 seconds per download
- **Unique Filenames:** SHA256 content hash (first 12 chars)
- **Directory Organization:** Separate folders for images/documents
- **Concurrency Limit:** 3 concurrent downloads (HDD-safe)

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

### Custom State Directory (`--state-dir`)

```bash
# Use custom state directory
cargo run -- --url "https://example.com" --resume --state-dir ./my-state
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
```

### Explicit Sitemap URL (`--sitemap-url`)

```bash
# Specify explicit sitemap URL
cargo run -- --url "https://example.com" --use-sitemap --sitemap-url "https://example.com/sitemap.xml"
```

**Sitemap Features:**
- Auto-discovery from `robots.txt`
- Sitemap index recursion (max depth 3)
- Gzip decompression support
- Zero-allocation streaming parser (quick-xml)

---

## Interactive Mode

| Flag | Default | Description |
|------|---------|-------------|
| `--interactive` | `false` | Interactive mode with TUI URL selector |

### TUI Interactive Mode (`--interactive`)

Launch interactive TUI for URL selection:

```bash
cargo run -- --url "https://example.com" --interactive
```

**Features:**
- Interactive checkbox selection of URLs
- Confirmation mode before download
- Terminal restore on panic/exit
- Ratatui + crossterm backend

---

## CLI UX Options (v1.1.0+)

| Flag | Default | Description |
|------|---------|-------------|
| `--dry-run` | `false` | Print discovered URLs to stdout and exit without scraping |
| `--quiet` | `false` | Suppress progress bars, emojis, and summary output |
| `completions <SHELL>` | — | Generate shell completion scripts |

### Dry-Run Mode (`--dry-run`)

Preview which URLs would be scraped without actually scraping them:

```bash
# Discover URLs only, no scraping
cargo run -- --url "https://example.com" --dry-run

# With sitemap discovery
cargo run -- --url "https://example.com" --use-sitemap --dry-run
```

**Output:** Discovered URLs are printed to stdout, one per line. Exit code 0.

### Quiet Mode (`--quiet`)

Suppress all non-essential output for clean scripting/pipe usage:

```bash
# No progress bars, no emojis, no summary
cargo run -- --url "https://example.com" --quiet

# Combine with NO_COLOR for fully ASCII output
NO_COLOR=1 cargo run -- --url "https://example.com" --quiet
```

**What `--quiet` suppresses:**
- Progress bar spinners and bars (indicatif)
- Emoji icons in log messages
- `ScrapeSummary` output at end

**What still appears:**
- `tracing` log messages (stderr)
- Error messages (stderr)
- Scraped content (stdout, if piped)

### Shell Completions (`completions`)

Generate completion scripts for your shell:

```bash
# Bash
cargo run -- completions bash > ~/.local/share/bash-completion/completions/rust-scraper

# Fish
cargo run -- completions fish > ~/.config/fish/completions/rust-scraper.fish

# Zsh
cargo run -- completions zsh > ~/.zsh/completions/_rust-scraper

# Elvish
cargo run -- completions elvish > ~/.elvish/lib/rust-scraper.elv

# PowerShell
cargo run -- completions powershell > rust-scraper.ps1
```

### NO_COLOR Support

The scraper respects the `NO_COLOR` environment variable:

```bash
# Disable emojis (ASCII fallback)
NO_COLOR=1 cargo run -- --url "https://example.com"

# Disable colors in log output
NO_COLOR=1 cargo run -- --url "https://example.com" -vv
```

**Emoji → ASCII mapping:**
| Emoji | ASCII |
|-------|-------|
| ✅ | OK |
| ⚠️ | WARN |
| 📌 | INFO |
| 🚀 | >> |
| 📁 | DIR |

---

## CLI UX Options (v1.1.0+)

| Flag | Default | Description |
|------|---------|-------------|
| `--dry-run` | `false` | Print discovered URLs to stdout and exit without scraping |
| `--quiet` | `false` | Suppress progress bars, emojis, and summary output |
| `completions <SHELL>` | — | Generate shell completion scripts |

### Dry-Run Mode (`--dry-run`)

Preview which URLs would be scraped without actually scraping them:

```bash
# Discover URLs only, no scraping
cargo run -- --url "https://example.com" --dry-run

# With sitemap discovery
cargo run -- --url "https://example.com" --use-sitemap --dry-run
```

**Output:** Discovered URLs are printed to stdout, one per line. Exit code 0.

### Quiet Mode (`--quiet`)

Suppress all non-essential output for clean scripting/pipe usage:

```bash
# No progress bars, no emojis, no summary
cargo run -- --url "https://example.com" --quiet

# Combine with NO_COLOR for fully ASCII output
NO_COLOR=1 cargo run -- --url "https://example.com" --quiet
```

**What `--quiet` suppresses:**
- Progress bar spinners and bars (indicatif)
- Emoji icons in log messages
- `ScrapeSummary` output at end

**What still appears:**
- `tracing` log messages (stderr)
- Error messages (stderr)
- Scraped content (stdout, if piped)

### Shell Completions (`completions`)

Generate completion scripts for your shell:

```bash
# Bash
cargo run -- completions bash > ~/.local/share/bash-completion/completions/rust-scraper

# Fish
cargo run -- completions fish > ~/.config/fish/completions/rust-scraper.fish

# Zsh
cargo run -- completions zsh > ~/.zsh/completions/_rust-scraper

# Elvish
cargo run -- completions elvish > ~/.elvish/lib/rust-scraper.elv

# PowerShell
cargo run -- completions powershell > rust-scraper.ps1
```

### NO_COLOR Support

The scraper respects the `NO_COLOR` environment variable:

```bash
# Disable emojis (ASCII fallback)
NO_COLOR=1 cargo run -- --url "https://example.com"

# Disable colors in log output
NO_COLOR=1 cargo run -- --url "https://example.com" -vv
```

**Emoji → ASCII mapping:**
| Emoji | ASCII |
|-------|-------|
| ✅ | OK |
| ⚠️ | WARN |
| 📌 | INFO |
| 🚀 | >> |
| 📁 | DIR |

---

## AI Options (Feature-Gated)

| Flag | Default | Description | Feature Required |
|------|---------|-------------|------------------|
| `--clean-ai` | `false` | Use AI-powered semantic cleaning for RAG output | `ai` |

### AI Semantic Cleaning (`--clean-ai`)

Requires `--features ai` to be enabled at compile time:

```bash
cargo run --features ai -- --url "https://example.com" --clean-ai
```

**What it does:**
- Uses `SemanticCleaner` to process HTML content
- Generates semantic chunks with embeddings
- Exports in JSONL format with embeddings field

**AI Feature Dependencies:**
- ONNX runtime (tract-onnx)
- Tokenizers (sentence-transformers)
- HuggingFace Hub for model downloads
- Memory-mapped file loading (zero-copy)
- Multi-dimensional arrays for embeddings

---

## JavaScript Rendering (Reserved for v1.4)

| Flag | Default | Description | Status |
|------|---------|-------------|--------|
| `--force-js-render` | `false` | Force JS rendering for SPA sites | ⏳ Reserved (no-op) |

### JavaScript Rendering (`--force-js-render`)

**⚠️ Not yet implemented.** This flag is reserved for future use (v1.4) when headless browser rendering will be available.

```bash
# Currently has no effect — reserved for v1.4
cargo run -- --url "https://example.com/spa" --force-js-render
```

**What it will do (v1.4):**
- Enable headless browser rendering for sites that require JavaScript
- Use the `JsRenderer` trait (defined in domain layer)
- Automatically detect SPA sites and fallback to JS rendering

**Current behavior:** The flag is accepted but has no effect. SPA sites are detected and a warning is emitted via `tracing::warn!`.

**Track implementation:** [Issue #16](https://github.com/XaviCode1000/rust-scraper/issues/16)

---

## JavaScript Rendering (Reserved for v1.4)

| Flag | Default | Description | Status |
|------|---------|-------------|--------|
| `--force-js-render` | `false` | Force JS rendering for SPA sites | ⏳ Reserved (no-op) |

### JavaScript Rendering (`--force-js-render`)

**⚠️ Not yet implemented.** This flag is reserved for future use (v1.4) when headless browser rendering will be available.

```bash
# Currently has no effect — reserved for v1.4
cargo run -- --url "https://example.com/spa" --force-js-render
```

**What it will do (v1.4):**
- Enable headless browser rendering for sites that require JavaScript
- Use the `JsRenderer` trait (defined in domain layer)
- Automatically detect SPA sites and fallback to JS rendering

**Current behavior:** The flag is accepted but has no effect. SPA sites are detected and a warning is emitted via `tracing::warn!`.

**Track implementation:** [Issue #16](https://github.com/XaviCode1000/rust-scraper/issues/16)

---

## Logging & Verbosity

| Flag | Description |
|------|-------------|
| `-v` | Info level logging |
| `-vv` | Debug level logging |
| `-vvv` | Trace level logging |

### Verbosity Flags

```bash
# Info level
cargo run -- --url "https://example.com" -v

# Debug level
cargo run -- --url "https://example.com" -vv

# Trace level (most verbose)
cargo run -- --url "https://example.com" -vvv
```

### Environment Variable (`RUST_LOG`)

For fine-grained control:

```bash
# Debug for specific module
RUST_LOG=rust_scraper=debug cargo run -- --url "https://example.com"

# Trace for entire application
RUST_LOG=trace cargo run -- --url "https://example.com"

# Multiple levels
RUST_LOG=rust_scraper=debug,reqwest=info cargo run -- --url "https://example.com"
```

---

## Help & Version

| Flag | Description |
|------|-------------|
| `-h, --help` | Print help (see summary with `-h`) |
| `--version` | Print version information |

```bash
# Full help
cargo run -- --help

# Quick help summary
cargo run -- -h

# Version
cargo run -- --version
```

---

## Complete Examples

### 1. Basic Scraping

```bash
# Default settings (markdown output, 10 pages, 1s delay)
cargo run -- --url "https://example.com"
```

### 2. Custom Output Format

```bash
# JSON output
cargo run -- --url "https://example.com" -f json

# Plain text output
cargo run -- --url "https://example.com" -f text
```

### 3. RAG Pipeline Export

```bash
# JSONL format (optimal for RAG)
cargo run -- --url "https://example.com" --export-format jsonl

# With custom output directory
cargo run -- --url "https://example.com" --export-format jsonl -o ./rag-data
```

### 4. Asset Downloads

```bash
# Download images only
cargo run --features images -- --url "https://example.com" --download-images

# Download documents only
cargo run --features documents -- --url "https://example.com" --download-documents

# Download both images and documents
cargo run --features full -- --url "https://example.com" --download-images --download-documents
```

### 5. Rate Limiting & Concurrency

```bash
# Slower scraping (2s delay)
cargo run -- --url "https://example.com" --delay-ms 2000

# Limit to 5 pages
cargo run -- --url "https://example.com" --max-pages 5

# Custom concurrency
cargo run -- --url "https://example.com" --concurrency 2
```

### 6. Resume Mode

```bash
# First run with resume enabled
cargo run -- --url "https://example.com" --max-pages 100 --resume

# Resume after interruption
cargo run -- --url "https://example.com" --max-pages 100 --resume
```

### 7. Sitemap Discovery

```bash
# Auto-discover sitemap from robots.txt
cargo run -- --url "https://example.com" --use-sitemap

# Explicit sitemap URL
cargo run -- --url "https://example.com" --use-sitemap --sitemap-url "https://example.com/sitemap.xml"
```

### 8. Interactive Mode

```bash
# Launch TUI for URL selection
cargo run -- --url "https://example.com" --interactive
```

### 9. AI Semantic Cleaning

```bash
# Enable AI-powered cleaning
cargo run --features ai -- --url "https://example.com" --clean-ai
```

### 10. Production Dataset Creation

```bash
# Full production run with all features
cargo run --features full -- \
  --url "https://example.com" \
  --export-format jsonl \
  --download-images \
  --download-documents \
  --delay-ms 2000 \
  --max-pages 100 \
  --concurrency 3 \
  --resume \
  -o ./production-dataset \
  -vv
```

### 11. CSS Selector Extraction

```bash
# Extract only article content
cargo run -- --url "https://example.com/blog" -s "article.post-content"

# Extract main content by ID
cargo run -- --url "https://example.com" -s "#main"
```

### 12. Verbose Debugging

```bash
# Debug logging
cargo run -- --url "https://example.com" -vv

# Trace logging with custom RUST_LOG
RUST_LOG=rust_scraper=trace cargo run -- --url "https://example.com" -vvv
```

---

## Feature Flags

rust-scraper supports optional features for extended functionality:

| Feature | Description | Enables |
|---------|-------------|---------|
| `images` | Image downloading support | `mime-type-detector` |
| `documents` | Document downloading support | `mime-type-detector` |
| `ai` | AI semantic cleaning | `ort`, `tokenizers`, `tract-onnx`, etc. |
| `full` | All features | `images`, `documents` |

### Using Feature Flags

```bash
# Enable single feature
cargo run --features images -- --url "https://example.com" --download-images

# Enable multiple features
cargo run --features "images,documents" -- --url "https://example.com" --download-images --download-documents

# Enable all features
cargo run --features full -- --url "https://example.com" --download-images --download-documents

# Build with features (faster subsequent runs)
cargo build --release --features full
./target/release/rust-scraper --url "https://example.com" --download-images --download-documents
```

---

## Troubleshooting

### Invalid URL Error

```bash
# ❌ Wrong (missing protocol)
cargo run -- --url "example.com"

# ✅ Correct
cargo run -- --url "https://example.com"
```

**Error Message:**
```
Error: Invalid URL: Failed to parse URL 'example.com': relative URL without a base
```

### SSL/TLS Certificate Errors

```bash
# Update system certificates (Arch Linux / CachyOS)
sudo pacman -Sy ca-certificates

# Update system certificates (Debian/Ubuntu)
sudo update-ca-certificates
```

**Error Message:**
```
Error: error sending request: certificate validation failed
```

### Permission Denied

```bash
# Check directory permissions
ls -la ./output

# Create directory with correct permissions
mkdir -p ./output && chmod 755 ./output
```

**Error Message:**
```
Error: Failed to write output: Permission denied (os error 13)
```

### Network Timeouts

For slow networks, increase delay and reduce concurrency:

```bash
cargo run -- --url "https://example.com" --delay-ms 3000 --concurrency 1
```

### Feature Not Enabled

```bash
# ❌ Wrong (trying to use feature without enabling)
cargo run -- --url "https://example.com" --download-images

# ✅ Correct
cargo run --features images -- --url "https://example.com" --download-images
```

**Error Message:**
```
Error: Feature 'images' is not enabled
```

### AI Feature Compilation

The `ai` feature is 100% Pure Rust — no CMake, no C++ toolchain needed. All dependencies (`tract-onnx`, `tokenizers`, `hf-hub`) compile natively with `cargo build --features ai`.

**Common Errors:**
- `ONNX runtime not found` → Build with `--features ai`
- Out of memory during build → Use `sccache` or increase swap

### Memory Issues on Large Scrapes

For systems with limited RAM (8GB or less):

```bash
# Reduce concurrency
cargo run -- --url "https://example.com" --concurrency 1 --max-pages 20

# Process in batches
cargo run -- --url "https://example.com" --max-pages 10 --resume
```

---

## Exit Codes

| Code | Constant | Description |
|------|----------|-------------|
| `0` | `EX_OK` | Success — all URLs scraped without errors |
| `64` | `EX_USAGE` | Invalid arguments or URL parsing error |
| `69` | `EX_UNAVAILABLE` | Network error or partial success (some URLs failed) |
| `74` | `EX_IOERR` | I/O error — failed to write output files |
| `76` | `EX_PROTOCOL` | Protocol error — TUI failure, WAF blocked |
| `78` | `EX_CONFIG` | Configuration error — config file parsing failed |

**Partial Success (exit 69):** When some URLs scrape successfully but others fail. The `ScrapeSummary` shows the breakdown.

### Example Exit Code Usage

```bash
# Check exit code
cargo run -- --url "https://example.com"
echo $?  # 0 = success, 69 = partial, etc.

# Script with error handling
cargo run -- --url "https://example.com" --quiet
case $? in
    0) echo "All URLs scraped successfully" ;;
    69) echo "Some URLs failed, check logs" ;;
    64) echo "Invalid URL or arguments" ;;
    *) echo "Unexpected error" ;;
esac
```

---

## Full Help Output

<details>
<summary><strong>Click to expand full --help output (verified 2026-03-11)</strong></summary>

```
Production-ready web scraper with Clean Architecture                    

Usage: rust_scraper [OPTIONS] --url <URL>                               

Options:                                                                
  -u, --url <URL>                                                       
          URL to scrape (required)                                      
                                                                        
  -s, --selector <SELECTOR>                                             
          CSS selector for content extraction                           
                                                                        
          [default: body]                                               
                                                                        
  -o, --output <OUTPUT>                                                 
          Output directory for scraped content                          
                                                                        
          [default: output]                                             
                                                                        
  -f, --format <FORMAT>                                                 
          Output format for individual files (markdown, text, json)     
                                                                        
          Creates separate output files for each scraped page: - markdown: Markdown with YAML frontmatter (default) - text: Plain text without formatting - json: Structured JSON with metadata
                                                                        
          Use this for human-readable output or when you need individual files per page.
                                                                        
          Possible values:                                              
          - markdown: Markdown format with YAML frontmatter (recommended for RAG)
          - json:     Structured JSON with metadata                     
          - text:     Plain text without formatting                     
                                                                        
          [default: markdown]                                           
                                                                        
      --export-format <EXPORT_FORMAT>                                   
          Export format for RAG pipeline (jsonl, auto)            
                                                                   
          Creates output suitable for retrieval-augmented generation: - jsonl: JSON Lines format (one JSON per line), optimal for RAG - auto: Detect from existing export files
                                                                        
          Use this for LLM/RAG pipelines that need batch export.        
                                                                        
          Possible values:                                              
          - jsonl: JSONL format (JSON Lines - one JSON object per line) Optimal for RAG pipelines and vector database ingestion
          - auto:  Auto-detect format from existing export files        
                                                                        
          [default: jsonl]                                              
                                                                        
      --resume                                                          
          Resume mode - skip URLs already processed                     
                                                                        
          Saves processing status to cache directory (~/.cache/rust-scraper/state) Avoids re-processing URLs already scraped successfully.
                                                                        
      --state-dir <STATE_DIR>                                           
          Custom state directory for resume mode                        
                                                                        
          Default: ~/.cache/rust-scraper/state                          
                                                                        
      --delay-ms <DELAY_MS>                                             
          Delay between requests in milliseconds                        
                                                                        
          [default: 1000]                                               
                                                                        
      --max-pages <MAX_PAGES>                                           
          Maximum pages to scrape                                       
                                                                        
          [default: 10]                                                 
                                                                        
      --download-images                                                 
          Download images from the page                                 
                                                                        
      --download-documents                                              
          Download documents from the page (PDF, DOCX, XLSX, etc.)      
                                                                        
  -v, --verbose...                                                      
          Verbosity level (use multiple times for more detail: -v, -vv, -vvv)
                                                                        
      --concurrency <CONCURRENCY>                                       
          Concurrency level (number of parallel requests)               
                                                                        
          Default: auto-detect based on CPU cores: - 1-2 cores: 1 - 4 cores: 3 (HDD-aware) - 8+ cores: min(CPU cores - 1, 8)
                                                                        
          Note: Can be overridden via CLI or detected at runtime. The actual value used is determined at startup.
                                                                        
          [default: auto]                                               
                                                                        
      --use-sitemap                                                     
          Use sitemap for URL discovery (auto-discovers from robots.txt if URL not provided)
                                                                        
      --sitemap-url <SITEMAP_URL>                                       
          Explicit sitemap URL (optional, auto-discovers if not provided)
                                                                        
      --interactive                                                     
          Interactive mode with TUI URL selector                        
                                                                        
  -h, --help                                                            
          Print help (see a summary with '-h')
```

</details>

*To regenerate: `cargo run -- --help 2>&1 | tee /tmp/cli_full_help.txt`*

---

## Related Documentation

- [Architecture](./ARCHITECTURE.md) — Clean Architecture layers
- [Configuration](./CONFIGURATION.md) — Advanced configuration options
- [RAG Pipeline](./RAG_PIPELINE.md) — Using rust-scraper for RAG datasets
- [TUI Guide](./TUI.md) — Interactive mode guide

---

## Version History

### v1.1.0 (2026-04-03)

- ✅ `CliExit` return type with sysexits codes (0, 64, 69, 74, 76, 78)
- ✅ `--dry-run` mode — print discovered URLs, exit without scraping
- ✅ `--quiet` mode — suppress progress bars, emojis, and summary
- ✅ `completions` subcommand — bash, fish, zsh, elvish, powershell
- ✅ `NO_COLOR` support — emoji → ASCII fallback
- ✅ Pre-flight HEAD check — fail fast on DNS errors
- ✅ Progress bars — spinner for discovery, bounded bar for scraping
- ✅ Config file loading — `~/.config/rust-scraper/config.toml`
- ✅ `ScrapeSummary` — structured summary with emoji/ASCII display
- ✅ `built` integration — build-time metadata in binary
- ✅ 304 tests passing, clippy clean

### v1.3.0 (2026-04-01)

- ✅ `--force-js-render` flag reserved (no-op, ready for v1.4)
- ✅ SPA detection warnings via `detect_spa_content()`
- ✅ `JsRenderer` trait defined in domain layer

### v1.0.0 (2026-03-11)

- ✅ Full CLI documentation with all verified flags
- ✅ Feature flags documented (`ai`, `images`, `documents`)
- ✅ Concurrency auto-detection (hardware-aware)
- ✅ Sitemap support with auto-discovery
- ✅ TUI interactive mode
- ✅ State management with resume capability
- ✅ AI semantic cleaning (feature-gated)

---

**Last Verified:** 2026-03-11 with `cargo run -- --help`  
**rust-scraper** v1.0.0 — Production-ready web scraper with Clean Architecture

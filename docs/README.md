# 🦀 Rust Scraper

[![CI](https://github.com/XaviCode1000/rust-scraper/actions/workflows/ci.yml/badge.svg)](https://github.com/XaviCode1000/rust-scraper/actions/workflows/ci.yml)
[![Tests](https://img.shields.io/badge/tests-252%20passing-brightgreen)](https://github.com/XaviCode1000/rust-scraper)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-2021-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-1.0.6-blue)](https://github.com/XaviCode1000/rust-scraper/releases)

**Production-ready web scraper with Clean Architecture, TUI selector, and AI-powered semantic cleaning.**

## ✨ Features

### Core (v1.0.0)
- 📖 **Readability Algorithm** - Extracts clean content like Firefox Reader Mode
- 🌐 **Modern HTTP Client** - reqwest with TLS (rustls), gzip/brotli compression
- 📝 **Multiple Output Formats** - Markdown (with YAML frontmatter), JSON, plain text
- 🔧 **CLI Interface** - Full control via command line arguments
- 🕸️ **Sitemap Support** - Zero-allocation streaming parser (quick-xml)
- 🖥️ **TUI Interactive Selector** - Ratatui + crossterm URL picker

### HTTP Client Improvements (v1.0.6) - Option A
- 🔄 **Retry Logic** - Exponential backoff 1s→2s→4s for 403/429/5xx
- 🍪 **Cookie Persistence** - Session maintenance across requests
- 🛡️ **Headers** - Accept-Language, Accept, Referer, Cache-Control
- ✅ **Validated** - Tested against real sites: books.toscrape.com, quotes.toscrape.com, webscraper.io

### Production Ready (v1.0.0+)
- 🎯 **Bounded Concurrency** - Prevents resource exhaustion (HDD-aware)
- 🎭 **User-Agent Rotation** - Chrome 131+ UAs with TTL caching
- 🛡️ **Type-Safe Errors** - `ScraperError` enum with 14 variants
- 🧪 **Well Tested** - 252 tests (unit, integration)

### Asset Download
- 🖼️ **Image Download** - Automatic download to `output/images/`
- 📄 **Document Download** - PDF, DOCX, XLSX to `output/documents/`
- 🔍 **MIME Detection** - Automatic classification by extension
- 🔐 **SHA256 Hashing** - Unique filenames, no collisions

### AI-Powered (v1.0.5+)
- 🧠 **Semantic Cleaning** - Local SLM inference (100% privacy, no API calls)
- 📊 **Embeddings** - Preserved during relevance filtering
- ⚡ **AVX2 SIMD** - 4-8x speedup on supported CPUs

## 🚀 Quick Start

```bash
# Clone repository
git clone https://github.com/XaviCode1000/rust-scraper.git
cd rust-scraper

# Build in release mode
cargo build --release

# Run with URL (required)
cargo run --release -- --url "https://example.com"
```

## 🎯 Usage

### Basic Scraping

```bash
# Basic usage (URL is REQUIRED)
cargo run --release -- --url "https://example.com"

# Specify output directory
cargo run --release -- --url "https://example.com" -o ./output

# Choose output format
cargo run --release -- --url "https://example.com" -f json
cargo run --release -- --url "https://example.com" -f text
```

### Asset Downloads

```bash
# Download images only
cargo run --release -- --url "https://example.com" --download-images

# Download documents only
cargo run --release -- --url "https://example.com" --download-documents

# Download both
cargo run --release -- --url "https://example.com" --download-images --download-documents
```

### Advanced Options

```bash
# Full example with all options
cargo run --release -- \
  --url "https://example.com/article" \
  --selector "article.main" \
  --output "./my-output" \
  --format markdown \
  --download-images \
  --download-documents \
  --delay-ms 1000 \
  --max-pages 10 \
  --verbose
```

### CLI Options

| Option | Description | Default |
|--------|-------------|---------|
| `-u, --url` | **URL to scrape (REQUIRED)** | - |
| `-s, --selector` | CSS selector for content | `body` |
| `-o, --output` | Output directory | `output` |
| `-f, --format` | Output format (markdown/json/text) | `markdown` |
| `--download-images` | Download images to `output/images/` | ❌ |
| `--download-documents` | Download documents to `output/documents/` | ❌ |
| `--delay-ms` | Delay between requests (ms) | `1000` |
| `--max-pages` | Maximum pages to scrape | `10` |
| `-v, --verbose` | Increase verbosity (use multiple times) | - |

```bash
# Get help
cargo run --release -- --help
```

## 📁 Output Structure

### Markdown Output (Default)

```
output/
└── example.com/
    └── article/
        └── index.md
```

**Markdown file with YAML frontmatter:**
```markdown
---
title: Article Title
url: https://example.com/article
date: "2026-03-08"
author: John Doe
excerpt: A short excerpt
---

# Article Title

Main content here with **markdown** formatting...

```rust
// Code blocks with syntax highlighting
fn main() {
    println!("Hello, world!");
}
```
```

### With Asset Downloads

```
output/
├── example.com/
│   └── article/
│       └── index.md
├── images/
│   ├── 027e504eabfc.png
│   ├── 0c2f4f0301fe.png
│   └── e15cbdd2d653.svg
└── documents/
    └── 9870371a7a8c.pdf
```

### JSON Output

```bash
cargo run --release -- --url "https://example.com" -f json
```

**output/results.json:**
```json
[
  {
    "title": "Article Title",
    "content": "Main content...",
    "url": "https://example.com/article",
    "excerpt": "A short excerpt",
    "author": "John Doe",
    "date": "2026-03-08",
    "assets": [
      {
        "url": "https://example.com/image.png",
        "local_path": "output/images/027e504eabfc.png",
        "asset_type": "image",
        "size": 1024
      }
    ]
  }
]
```

## 🏗️ Architecture

This project uses **Clean Architecture** with four layers:

```
┌─────────────────────────────────────┐
│         CLI (main.rs)               │  - User interface
└──────────────┬──────────────────────┘
               │
┌──────────────▼──────────────────────┐
│    Library (lib.rs)                 │  - Public API
└──────────────┬──────────────────────┘
               │
    ┌──────────┴──────────┐
    │                     │
┌───▼──────┐      ┌──────▼────────┐
│ DOMAIN   │      │  APPLICATION  │
│ (pure)   │      │  (use cases)  │
└──────────┘      └──────┬────────┘
                         │
              ┌──────────┴──────────┐
              │                     │
       ┌──────▼──────┐      ┌──────▼──────┐
       │INFRASTRUCTURE│      │  ADAPTERS   │
       └─────────────┘      └─────────────┘
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for detailed architecture documentation.

## 🧪 Testing

```bash
# Nextest (4x faster than cargo test) ✅ RECOMMENDED
cargo nextest run

# Run only failed tests
cargo nextest run --failed

# Run ignored tests (real sites)
cargo nextest run --run-ignored ignored-only

# Traditional (slower)
cargo test

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test test_validate_url

# Run with coverage
cargo llvm-cov nextest --html
```

**Test Coverage**: 252 tests passing
- Unit tests: 252
- Integration tests (real sites): 6 (ignored by default)

## 📦 Installation (as Library)

Add to your `Cargo.toml`:

```toml
[dependencies]
rust_scraper = "0.3.0"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

**Example usage:**

```rust
use rust_scraper::{
    create_http_client,
    scrape_with_config,
    save_results,
    validate_and_parse_url,
    ScraperConfig,
    OutputFormat,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create HTTP client (with retry + user-agent rotation)
    let client = create_http_client()?;
    
    // Parse URL
    let url = validate_and_parse_url("https://example.com")?;
    
    // Configure scraping
    let config = ScraperConfig {
        download_images: true,
        download_documents: false,
        output_dir: PathBuf::from("./output"),
        max_file_size: Some(50 * 1024 * 1024), // 50MB
    };
    
    // Scrape
    let results = scrape_with_config(&client, &url, &config).await?;
    
    // Save results
    save_results(&results, &config.output_dir, &OutputFormat::Markdown)?;
    
    Ok(())
}
```

## 🔧 Development

```bash
# Clone and setup
git clone https://github.com/XaviCode1000/rust-scraper.git
cd rust-scraper

# Build in debug mode
cargo build

# Build in release mode (optimized)
cargo build --release

# Run clippy (linting)
cargo clippy -- -D clippy::correctness

# Format code
cargo fmt

# Run all tests
cargo test --all
```

### Release Profile

The project uses aggressive optimizations for production builds:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

## 📋 Requirements

- [Rust](https://rustup.rs/) 1.70+ (edition 2021)
- Linux, macOS, or Windows

```bash
# Check Rust version
rustc --version

# Update if needed
rustup update
```

## 🤝 Contributing

Contributions are welcome! See [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) for guidelines.

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📄 License

MIT License - see [LICENSE](LICENSE) for details.

## 📚 Documentation

- [Architecture](docs/ARCHITECTURE.md) - Clean Architecture details
- [Changelog](docs/CHANGES.md) - Version history and migration guide
- [CLI Reference](docs/CLI.md) - Complete CLI documentation
- [Contributing](docs/CONTRIBUTING.md) - How to contribute

## 🙏 Acknowledgments

- [legible](https://github.com/relaxnow/legible) - Readability algorithm
- [reqwest](https://github.com/seanmonstar/reqwest) - HTTP client
- [html-to-markdown-rs](https://github.com/ramonh/html-to-markdown) - HTML→Markdown conversion
- [scraper](https://github.com/programble/scraper) - HTML parsing

---

**Made with ❤️ using Rust and Clean Architecture**

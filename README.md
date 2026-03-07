# 🦀 Rust Scraper

[![CI](https://github.com/XaviCode1000/rust-scraper/actions/workflows/ci.yml/badge.svg)](https://github.com/XaviCode1000/rust-scraper/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-2021-orange.svg)](https://www.rust-lang.org/)

Modern web scraper optimized for RAG (Retrieval-Augmented Generation) datasets. Uses the Readability algorithm (same as Firefox Reader Mode) to extract clean, structured content from web pages.

## ✨ Features

- 📖 **Readability Algorithm** - Extracts clean content like Firefox Reader Mode
- 🌐 **HTTP Client** - Robust reqwest-based fetching with TLS support
- 📝 **Multiple Output Formats** - Markdown, JSON, or plain text
- 🔧 **CLI Interface** - Full control via command line arguments
- ⚡ **High Performance** - Optimized release profile with LTO
- 🧪 **Well Tested** - 38 unit and integration tests
- 📦 **Zero External Dependencies** - No browser required

## 🚀 Requirements

- [Rust](https://rustup.rs/) (1.70+ for edition 2021)

```bash
rustc --version
```

## 📦 Installation

```bash
# Clone repository
git clone https://github.com/XaviCode1000/rust-scraper.git
cd rust-scraper

# Build in release mode
cargo build --release
```

## 🎯 Usage

```bash
# Basic usage (URL is REQUIRED)
cargo run --release -- --url "https://example.com"

# Specify output directory
cargo run --release -- --url "https://example.com" -o ./output

# Choose output format
cargo run --release -- --url "https://example.com" -f json
cargo run --release -- --url "https://example.com" -f text

# More options
cargo run --release -- --help
```

### CLI Options

| Option | Description | Default |
|--------|-------------|---------|
| `-u, --url` | **URL to scrape (REQUIRED)** | - |
| `-s, --selector` | CSS selector for content | `body` |
| `-o, --output` | Output directory | `output` |
| `-f, --format` | Output format (markdown/json/text) | `markdown` |
| `--delay-ms` | Delay between requests (ms) | `1000` |
| `--max-pages` | Maximum pages to scrape | `10` |
| `-v, --verbose` | Increase verbosity | - |

## 📁 Structure

```
rust-scraper/
├── src/
│   ├── lib.rs        # Library with tests
│   ├── main.rs       # CLI entry point
│   ├── scraper.rs    # Core scraping logic
│   ├── config.rs     # Configuration & logging
│   └── markdown.rs   # (Deprecated - use scraper)
├── tests/
│   └── integration_test.rs
├── Cargo.toml
├── README.md
└── LICENSE
```

## 🧪 Testing

```bash
# Run all tests
cargo test

# Run with coverage
cargo test -- --nocapture

# Run specific test
cargo test test_validate_url
```

**Test Coverage**: 38 tests (30 unit + 8 integration)

## 📋 Example Output

### Input
```bash
cargo run --release -- --url "https://example.com" -o ./output
```

### Output (Markdown)
```markdown
# Example Domain

This domain is for use in documentation examples without needing permission. Avoid use in operations. Learn more

---

*Source: [https://example.com/](https://example.com/)*
```

## 🔄 Migration from v0.1.x

**Breaking Change**: URL is now a required CLI argument

```bash
# v0.1.x (OLD - hardcoded URL)
cargo run  # Used hardcoded URL

# v0.2.0+ (NEW - URL required)
cargo run -- --url "https://example.com"
```

## 📄 License

MIT License - see [LICENSE](LICENSE) for details.

## 📚 Documentation

See the [doc/](doc/) folder for detailed documentation:

- [doc/README.md](doc/README.md) - Quick start guide
- [doc/ARCHITECTURE.md](doc/ARCHITECTURE.md) - Technical architecture
- [doc/CLI.md](doc/CLI.md) - CLI reference
- [doc/CONTRIBUTING.md](doc/CONTRIBUTING.md) - Contributing guide

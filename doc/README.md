# Rust Scraper

A modern web scraper optimized for RAG (Retrieval-Augmented Generation) datasets. Uses the Readability algorithm (same as Firefox Reader Mode) to extract clean, structured content from web pages.

## Features

- **Readability Algorithm** - Extracts clean content like Firefox Reader Mode
- **Structured Markdown** - Preserves headings, code blocks, lists, emphasis
- **Domain-based Organization** - Files saved in `output/{domain}/{path}`
- **YAML Frontmatter** - Rich metadata (title, url, date, author, excerpt)
- **Multiple Output Formats** - Markdown, JSON, or plain text
- **Syntax Highlighting** - Code blocks with language detection
- **High Performance** - Optimized release profile with LTO

## Quick Start

```bash
# Build
cargo build --release

# Scrape a URL
cargo run --release -- --url "https://example.com"

# Output to custom directory
cargo run --release -- --url "https://example.com" -o ./my-output
```

## Output Structure

```
output/
└── example.com/
    ├── index.md          # Root page
    └── docs/
        └── api/
            └── index.md  # Nested page
```

Each markdown file includes YAML frontmatter:

```yaml
---
title: Example Domain
url: https://example.com/
date: 2026-03-07
author: null
excerpt: This domain is for use in documentation...
---

# Example Domain

Content here...
```

## CLI Options

| Option | Description | Default |
|--------|-------------|---------|
| `-u, --url` | **URL to scrape (required)** | - |
| `-o, --output` | Output directory | `output` |
| `-f, --format` | Output format | `markdown` |
| `-s, --selector` | CSS selector | `body` |
| `-v, --verbose` | Verbose logging | - |

## Output Formats

- **markdown** - Structured Markdown with frontmatter
- **json** - JSON array with all metadata
- **text** - Plain text content

## Requirements

- Rust 1.70+ (edition 2021)

## License

MIT

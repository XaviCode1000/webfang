# Architecture

## Overview

The rust-scraper follows a layered architecture with clear separation of concerns:

```
┌─────────────────────────────────────────┐
│              CLI (main.rs)              │
│  - Argument parsing with clap           │
│  - Orchestration of workflow           │
└────────────────┬──────────────────────┘
                 │
┌────────────────▼──────────────────────┐
│           Library (lib.rs)              │
│  - Public API re-exports               │
│  - OutputFormat enum                   │
│  - Args struct                         │
└────────────────┬──────────────────────┘
                 │
    ┌────────────┴────────────┐
    │                         │
┌───▼────────────┐  ┌────────▼─────────┐
│   scraper.rs    │  │   url_path.rs    │
│                │  │                  │
│ - HTTP client  │  │ - Domain         │
│ - Readability  │  │ - UrlPath        │
│ - HTML→MD     │  │ - OutputPath     │
│ - Saving      │  │                  │
└───────────────┘  └──────────────────┘
```

## Core Modules

### scraper.rs

The main scraping engine:

1. **HTTP Client** - Uses reqwest with:
   - Custom User-Agent
   - Gzip/Brotli compression
   - 30s timeout

2. **Content Extraction** - Two-layer approach:
   - **Primary**: legible (Readability algorithm)
   - **Fallback**: htmd for basic HTML stripping

3. **Markdown Conversion** - Uses html-to-markdown-rs:
   - Preserves heading hierarchy (h1-h6)
   - Code blocks with language detection
   - Lists (ordered/unordered)
   - Emphasis (bold, italic)
   - Links and images

4. **Output Generation**:
   - YAML frontmatter with metadata
   - Domain-based folder structure
   - URL-based file naming

### url_path.rs

Type-safe URL handling (type-no-stringly pattern):

- **Domain** - Validated domain extraction
- **UrlPath** - URL path sanitization for filesystem
- **OutputPath** - Complete output path generation

## Data Flow

```
URL Input
    │
    ▼
┌─────────────┐
│ Validation  │  url::Url parsing
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ HTTP Fetch  │  reqwest client
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ Readability │  legible crate
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ Markdown    │  html-to-markdown-rs
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ Frontmatter │  serde_yaml
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ File Save   │  std::fs
└─────────────┘
```

## Design Decisions

### Why Readability?

The Readability algorithm (used by Firefox Reader Mode, Pocket, Instapaper) is specifically designed to extract the main content from a web page while filtering out:
- Navigation menus
- Advertisements
- Sidebars
- Footer content
- Scripts and styles

This makes it ideal for RAG pipelines where clean, relevant content is essential.

### Type-Safe URL Handling

Instead of using raw `String` for paths, we use newtypes:
- Prevents invalid filenames
- Validates at construction time
- Makes APIs self-documenting

### Why html-to-markdown-rs?

Compared to alternatives:
- Preserves heading hierarchy
- Supports code blocks with language hints
- Actively maintained (v2.28.0 in 2026)
- Rich configuration options

## Dependencies

### Core
- **reqwest** - HTTP client
- **legible** - Readability algorithm
- **html-to-markdown-rs** - HTML→Markdown

### CLI
- **clap** - Argument parsing

### Output
- **serde_yaml** - YAML frontmatter
- **chrono** - Date formatting
- **syntect** - Syntax highlighting

## Testing Strategy

- **Unit tests** - Individual functions
- **Integration tests** - Full workflow
- **TempDir** - Isolated file operations
- **walkdir** - Verify nested output structure

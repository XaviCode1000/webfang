# Architecture — rust-scraper

**Last Updated:** April 2026  
**Version:** 1.4.0  
**Clean Architecture:** 4 layers with strict dependency rule

---

## Overview

The rust-scraper follows **Clean Architecture** with strict separation of concerns. Dependencies point inward: **Domain ← Application ← Infrastructure/Adapters**.

```
┌──────────────────────────────────────────────────────────────┐
│                         CLI (main.rs)                        │
│  - Clap argument parsing                                     │
│  - TUI selector (ratatui)                                    │
│  - Logging initialization (tracing)                          │
│  - 1,200+ LOC                                                │
└─────────────────────┬────────────────────────────────────────┘
                      │
┌─────────────────────▼─────────────────────────────────────────┐
│                      Library (lib.rs)                         │
│  - Public API re-exports                                      │
│  - ScraperConfig, Args, OutputFormat                          │
│  - Feature flags (ai, images, documents)                │
│  - 28,780 LOC                                                 │
└─────────────────────┬─────────────────────────────────────────┘
                      │
      ┌───────────────┴──────────────────┐
      │                                  │
┌─────▼──────────┐              ┌────────▼─────────┐
│   DOMAIN       │              │  APPLICATION     │
│   (pure)       │              │  (use cases)     │
│   1,678 LOC    │              │  1,747 LOC       │
│                │              │                  │
│ - entities     │              │ - http_client    │
│ - value_objs   │              │ - scraper_svc    │
│ - exporter     │              │ - crawler_svc    │
│ - semantic_*   │              │ - url_filter     │
│ - crawler_*    │              │                  │
└────────────────┘              └──────────────────┘
                                       │
                    ┌──────────────────┼──────────────────┐
                    │                  │                  │
             ┌──────▼──────┐  ┌───────▼───────┐  ┌───────▼──────┐
             │INFRASTRUCTURE│  │   ADAPTERS    │  │   EXTRACTOR  │
             │  7,507 LOC   │  │   1,417 LOC   │  │   (lib)      │
             │              │  │               │  │              │
             │ - ai/        │  │ - detector/   │  │ - mod.rs     │
             │ - crawler/   │  │ - downloader/ │  │              │
             │ - export/    │  │ - extractor/  │  │              │
             │ - converter/ │  │ - tui/        │  │              │
             │ - output/    │  │               │  │              │
             │ - scraper/   │  │               │  │              │
             │ - http/      │  │               │  │              │
             └──────────────┘  └───────────────┘  └──────────────┘
```

### Dependency Rule

```
Domain never imports Application, Infrastructure, or Adapters
Application imports Domain only
Infrastructure imports Domain + Application
Adapters import Domain + Infrastructure
```

**Verification:**
```bash
cd /home/gazadev/Dev/my_apps/rust_scraper
rg "^use (reqwest|tokio|scraper|tract)" src/domain/  # Returns nothing ✓
```

---

## Domain Layer (`src/domain/`)

**Total:** 1,678 lines of code  
**Purity:** Zero external framework dependencies (no reqwest, no tokio, no serde runtime)

### Module Structure

```
src/domain/
├── mod.rs                    (22 LOC)   — Module exports
├── entities.rs               (311 LOC)  — Core business entities
├── value_objects.rs          (148 LOC)  — Type-safe primitives
├── exporter.rs               (279 LOC)  — Exporter trait + error types
├── semantic_cleaner.rs       (174 LOC)  — AI cleaning trait (feature-gated)
├── crawler_entities.rs       (746 LOC)  — Web crawler domain types
└── js_renderer.rs            (92 LOC)   — JsRenderer trait + JsRenderError (SPA stub)
```

### Core Entities (`entities.rs`)

| Type | Purpose | LOC |
|------|---------|-----|
| `DownloadedAsset` | Downloaded image/document with metadata | ~30 |
| `ScrapedContent` | Main output: title, content, URL, metadata, assets | ~50 |
| `ExportFormat` | JSONL, Auto for RAG pipeline | ~40 |
| `ExportState` | Pending, Exported, Failed with metadata | ~30 |
| `DocumentChunk` | AI semantic chunk with embedding | ~50 |

**Example:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapedContent {
    pub title: String,
    pub content: String,
    pub url: ValidUrl,  // Type-safe, guaranteed valid
    pub excerpt: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub html: Option<String>,
    pub assets: Vec<DownloadedAsset>,
}
```

### Value Objects (`value_objects.rs`)

| Type | Purpose | LOC |
|------|---------|-----|
| `ValidUrl` | Newtype around `url::Url` — guarantees validity at type level | ~80 |

**Why newtype?**
- **Type Safety:** Can't accidentally pass invalid URL
- **Self-Documenting:** API signature guarantees validity
- **Compile-Time Validation:** Errors caught early

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidUrl(url::Url);

impl ValidUrl {
    pub fn parse(s: &str) -> crate::Result<Self> {
        Ok(Self(url::Url::parse(s).map_err(|e| {
            crate::ScraperError::invalid_url(e.to_string())
        })?))
    }
    
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
```

### Exporter Trait (`exporter.rs`)

**Trait definition:**
```rust
pub trait Exporter: Send + Sync + 'static {
    fn export(&self, content: &ScrapedContent) -> ExportResult;
    fn export_batch(&self, contents: &[ScrapedContent]) -> Result<(), ExporterError>;
    fn config(&self) -> &ExporterConfig;
}
```

**Implementations:**
- `JsonlExporter` — JSON Lines format for RAG pipelines

### SemanticCleaner Trait (`semantic_cleaner.rs`)

**Feature-gated:** `#[cfg(feature = "ai")]`

```rust
pub trait SemanticCleaner: private::Sealed + Send + Sync {
    async fn clean(&self, content: &str) -> Result<String, SemanticError>;
    async fn chunk(&self, content: &str) -> Result<Vec<DocumentChunk>, SemanticError>;
}
```

**Implementation:** `SemanticCleanerImpl` in `infrastructure/ai/semantic_cleaner_impl.rs` (787 LOC)

### Crawler Entities (`crawler_entities.rs`)

| Type | Purpose | LOC |
|------|---------|-----|
| `CrawlerConfig` | Configuration for web crawler | ~100 |
| `CrawlerConfigBuilder` | Builder pattern for config | ~150 |
| `CrawlResult` | Successful crawl output | ~50 |
| `CrawlError` | Crawl-specific errors | ~80 |
| `DiscoveredUrl` | URL discovered during crawl | ~30 |
| `ContentType` | HTML, XML, JSON, etc. | ~40 |

### JavaScript Renderer Trait (`js_renderer.rs`)

**Forward-compatible stub for SPA support (Phase 1 of Issue #16)**

```rust
pub trait JsRenderer: Send + Sync {
    fn render(
        &self,
        url: &url::Url,
    ) -> impl std::future::Future<Output = Result<String, JsRenderError>> + Send;
}
```

| Type | Purpose | LOC |
|------|---------|-----|
| `JsRenderer` | Trait for JS rendering of web pages | ~20 |
| `JsRenderError` | Error enum for JS rendering failures | ~30 |

**Error variants:**
- `Browser(String)` — Browser launch/communication failure
- `Timeout { url, timeout_ms }` — Page load timeout
- `Navigation(String)` — Navigation to URL failed
- `Extraction(String)` — Content extraction after rendering failed

**Architecture note:** This trait lives in the Domain layer because it defines a business capability (rendering JS-dependent pages), not a specific implementation. The actual renderer (e.g., headless Chrome via `headless_chrome` or `fantoccini`) will be provided by the Infrastructure layer in Phase 2 (v1.4).

**Native async fn in trait** — Uses Rust 1.88+ native async traits, no `async-trait` crate needed.

### JavaScript Renderer Trait (`js_renderer.rs`)

**Forward-compatible stub for SPA support (Phase 1 of Issue #16)**

```rust
pub trait JsRenderer: Send + Sync {
    fn render(
        &self,
        url: &url::Url,
    ) -> impl std::future::Future<Output = Result<String, JsRenderError>> + Send;
}
```

| Type | Purpose | LOC |
|------|---------|-----|
| `JsRenderer` | Trait for JS rendering of web pages | ~20 |
| `JsRenderError` | Error enum for JS rendering failures | ~30 |

**Error variants:**
- `Browser(String)` — Browser launch/communication failure
- `Timeout { url, timeout_ms }` — Page load timeout
- `Navigation(String)` — Navigation to URL failed
- `Extraction(String)` — Content extraction after rendering failed

**Architecture note:** This trait lives in the Domain layer because it defines a business capability (rendering JS-dependent pages), not a specific implementation. The actual renderer (e.g., headless Chrome via `headless_chrome` or `fantoccini`) will be provided by the Infrastructure layer in Phase 2 (v1.4).

**Native async fn in trait** — Uses Rust 1.88+ native async traits, no `async-trait` crate needed.

---

## Application Layer (`src/application/`)

**Total:** 1,747 lines of code  
**Role:** Use cases and orchestration

### Module Structure

```
src/application/
├── mod.rs                  (18 LOC)   — Module exports
├── http_client.rs          (75 LOC)   — HTTP client factory
├── scraper_service.rs      (390 LOC)  — Scraping orchestration + SPA detection
├── crawler_service.rs      (942 LOC)  — Web crawler service
└── url_filter.rs           (468 LOC)  — URL filtering logic
```

### HTTP Client (`http_client.rs`)

**Features:**
- User-Agent rotation (14 modern browsers, weighted selection)
- Exponential backoff retry (3 retries, 100ms→200ms→400ms)
- Gzip/Brotli compression
- 30s timeout
- TLS via rustls with system certificates
- **WAF Detection (2026):** Chrome145 TLS fingerprint + Client Hints for Layer 2 evasion

#### WAF Detection System

The scraper includes a multi-layer WAF detection system for production-grade bot evasion:

```rust
// Layer 1: TLS Fingerprint (Layer 2 - Evasion)
Client::builder()
    .emulation(Emulation::Chrome145)  // 2026 standard
    .default_headers(headers)         // Client Hints
    ...

// Layer 2: Body Signature Detection (O(N) with Aho-Corasick)
if let Some(provider) = detect_waf_challenge(&body) {
    warn!("WAF challenge detected: {}", provider);
    // Rotate UA and retry once
}

// Layer 3: WafInspector (advanced)
use crate::infrastructure::http::waf_engine::WafInspector;
WafInspector::verify_integrity(&headers, &body)?;
```

**Client Hints (2026 Standard):**
| Header | Value |
|--------|-------|
| `Sec-CH-UA` | `"Google Chrome";v="145"` |
| `Sec-CH-UA-Mobile` | `?0` |
| `Sec-CH-UA-Platform` | `"Linux"` |
| `Sec-Fetch-Dest` | `document` |
| `Sec-Fetch-Mode` | `navigate` |
| `Sec-Fetch-Site` | `none` |
| `Sec-Fetch-User` | `?1` |
| `Upgrade-Insecure-Requests` | `1` |

**WafInspector (`src/infrastructure/http/waf_engine.rs`):**
- 50+ WAF signatures with O(N) Aho-Corasick matching
- Control header detection: `x-datadome-response`, `cf-mitigated`, `x-akamai-edge-auth`
- Entropy analysis for "Silent Challenge" detection

```rust
pub fn create_http_client() -> Result<reqwest_middleware::ClientWithMiddleware> {
    let client = reqwest::Client::builder()
        .user_agent(random_user_agent())
        .timeout(Duration::from_secs(30))
        .compression(true)
        .use_rustls_tls()
        .build()?;
    
    let retry_policy = ExponentialBackoff::builder()
        .base(100)
        .max_delay(Duration::from_secs(5))
        .build_with_max_retries(3);
    
    Ok(ClientBuilder::new(client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build())
}
```

### Scraper Service (`scraper_service.rs`)

**Public functions:**
- `scrape_with_readability(url: &str)` — Clean content extraction
- `scrape_with_config(url: &str, config: &ScraperConfig)` — Scraping with options
- `scrape_multiple_with_limit(urls: Vec<&str>, limit: usize)` — Bounded concurrency
- `detect_spa_content(url: &str, content: &str)` — SPA detection heuristic (v1.3.0)

**SPA Detection (v1.3.0):**
```rust
pub fn detect_spa_content(url: &str, content: &str) -> Option<SpaDetectionResult>
```

A page is flagged as potentially SPA-dependent when:
- Extracted content is below `MIN_CONTENT_CHARS` (50 chars)
- Future versions will also check for empty titles, SPA mount points (`<div id="root">`), and absence of semantic HTML

**Hardware-aware concurrency:**
```rust
// HDD-optimized: 3 concurrent requests max
const MAX_CONCURRENT_SCRAPES: usize = 3;
```

### Crawler Service (`crawler_service.rs`)

**Largest service:** 942 LOC

**Public functions:**
- `crawl_site(config: &CrawlerConfig)` — Full site crawl
- `crawl_with_sitemap(sitemap_url: &str)` — Crawl via sitemap.xml
- `discover_urls_for_tui(base_url: &str)` — TUI URL discovery
- `scrape_urls_for_tui(urls: Vec<ValidUrl>)` — TUI scraping

**Features:**
- Rate limiting with `governor` crate
- Concurrent data structures with `dashmap`
- URL queue management
- Link extraction and filtering
- Sitemap parsing

### URL Filter (`url_filter.rs`)

**468 LOC of URL filtering logic:**

**Functions:**
- `is_allowed(url: &Url, patterns: &[String])` — Check allowlist
- `is_excluded(url: &Url, patterns: &[String])` — Check blocklist
- `is_internal_link(base: &Url, target: &Url)` — Same-domain check
- `extract_domain(url: &Url)` — Extract domain string
- `matches_pattern(url: &Url, pattern: &str)` — Glob pattern matching

---

## Infrastructure Layer (`src/infrastructure/`)

**Total:** 7,507 lines of code  
**Role:** External implementations (HTTP, FS, converters, AI)

### Module Structure

```
src/infrastructure/
├── mod.rs                      (22 LOC)   — Module exports
├── http/
│   └── mod.rs                  (6 LOC)    — HTTP re-exports
├── scraper/
│   ├── mod.rs                  (11 LOC)
│   ├── readability.rs          (111 LOC)  — legible wrapper
│   ├── fallback.rs             (70 LOC)   — htmd fallback
│   └── asset_download.rs       (168 LOC)  — Asset downloading
├── converter/
│   ├── mod.rs                  (4 LOC)
│   ├── html_to_markdown.rs     (68 LOC)   — HTML→Markdown
│   └── syntax_highlight.rs     (152 LOC)  — Code highlighting
├── output/
│   ├── mod.rs                  (4 LOC)
│   ├── file_saver.rs           (192 LOC)  — File I/O
│   └── frontmatter.rs          (117 LOC)  — YAML frontmatter
├── crawler/
│   ├── mod.rs                  (17 LOC)
│   ├── http_client.rs          (122 LOC)  — Crawler HTTP
│   ├── link_extractor.rs       (301 LOC)  — Link extraction
│   ├── url_queue.rs            (223 LOC)  — URL queue management
│   └── sitemap_parser.rs       (538 LOC)  — Sitemap.xml parsing
├── export/
│   ├── mod.rs                  (17 LOC)
│   ├── jsonl_exporter.rs       (207 LOC)  — JSONL export
│   └── state_store.rs          (433 LOC)  — Export state tracking
└── ai/ (feature-gated)
    ├── mod.rs                  (141 LOC)
    ├── chunk_id.rs             (107 LOC)  — Chunk ID generation
    ├── chunker.rs              (473 LOC)  — Semantic chunking
    ├── embedding_ops.rs        (354 LOC)  — Embedding operations
    ├── inference_engine.rs     (447 LOC)  — ONNX inference
    ├── model_cache.rs          (648 LOC)  — Model caching
    ├── model_downloader.rs     (266 LOC)  — Model downloads
    ├── relevance_scorer.rs     (473 LOC)  — Relevance scoring
    ├── semantic_cleaner_impl.rs (787 LOC) — SemanticCleaner impl
    ├── sentence.rs             (176 LOC)  — Sentence segmentation
    ├── threshold_config.rs     (364 LOC)  — Threshold configuration
    └── tokenizer.rs            (393 LOC)  — Tokenization
```

### Key Modules

#### Scraper Module (365 LOC)

**Readability (`readability.rs`):**
```rust
pub fn extract_content(html: &str, url: &Url) -> Result<ScrapedContent, ScraperError> {
    let doc = legible::parse(html, url)
        .ok_or_else(|| ScraperError::Readability("Failed to parse".into()))?;
    
    Ok(ScrapedContent {
        title: doc.title,
        content: doc.content,
        url: ValidUrl::new(url.clone()),
        excerpt: doc.excerpt,
        author: doc.author,
        date: doc.date,
        html: None,
        assets: vec![],
    })
}
```

**Fallback (`fallback.rs`):**
- Uses `htmd` crate when Readability fails
- Simpler extraction, less accurate

**Asset Download (`asset_download.rs`):**
- SHA256 content hashing for unique filenames
- File size validation (50MB max)
- 30s timeout per download

#### Converter Module (224 LOC)

**HTML to Markdown (`html_to_markdown.rs`):**
- Uses `html-to-markdown-rs` crate
- Preserves headings, code blocks, lists

**Syntax Highlighting (`syntax_highlight.rs`):**
- Uses `syntect` crate
- Supports 100+ languages
- Theme customization

#### Output Module (313 LOC)

**File Saver (`file_saver.rs`):**
- Domain-based folder structure
- Atomic writes with temp files
- Conflict resolution

**Frontmatter (`frontmatter.rs`):**
- YAML frontmatter generation
- Metadata: title, date, author, excerpt, URL

#### Crawler Module (1,201 LOC)

**HTTP Client (`http_client.rs`):**
- Crawler-specific HTTP client
- Rate limiting integration

**Link Extractor (`link_extractor.rs`):**
- Extracts all links from HTML
- Filters by pattern
- Handles relative URLs

**URL Queue (`url_queue.rs`):**
- Concurrent queue with `dashmap`
- Priority ordering
- Duplicate detection

**Sitemap Parser (`sitemap_parser.rs`):**
- Parses sitemap.xml and sitemap index
- Handles gzip compression
- Respects robots.txt

#### Export Module (753 LOC)

**JSONL Exporter (`jsonl_exporter.rs`):**
- One JSON object per line
- Optimal for RAG pipelines

**State Store (`state_store.rs`):**
- Tracks export state
- Resume capability
- Progress reporting

#### AI Module (3,828 LOC) — Feature-gated

**Semantic Cleaner Implementation (`semantic_cleaner_impl.rs`):**
- ONNX model inference
- Sentence-transformers (all-MiniLM-L6-v2)
- Cosine similarity scoring

**Model Cache (`model_cache.rs`):**
- Memory-mapped file loading (zero-copy)
- LRU eviction
- Download from HuggingFace Hub

**Chunker (`chunker.rs`):**
- Semantic-aware chunking
- Overlap handling
- Token limit enforcement (512 tokens)

**Embedding Operations (`embedding_ops.rs`):**
- Cosine similarity calculation
- SIMD optimization with `wide` crate
- Batch processing

---

## Adapters Layer (`src/adapters/`)

**Total:** 1,417 lines of code  
**Role:** External integrations (feature-gated)

### Module Structure

```
src/adapters/
├── mod.rs                      (16 LOC)   — Module exports
├── detector/
│   ├── mod.rs                  (7 LOC)
│   └── mime.rs                 (272 LOC)  — MIME type detection
├── downloader/
│   └── mod.rs                  (440 LOC)  — Asset downloader
├── extractor/
│   └── mod.rs                  (8 LOC)    — URL extractor
└── tui/
    ├── mod.rs                  (50 LOC)   — TUI module
    ├── terminal.rs             (74 LOC)   — Terminal setup
    └── url_selector.rs         (550 LOC)  — Interactive URL selection
```

### Detector Module (279 LOC)

**MIME Type Detection (`mime.rs`):**
- `detect_from_url(url: &str)` — Classify by extension
- `detect_from_path(path: &Path)` — Path-based detection
- `AssetType` enum: Image, Document, Unknown
- `get_extension(url: &str)` — Extract extension

### Downloader Module (440 LOC)

**Asset Downloading:**
- Bounded concurrency (3 concurrent)
- SHA256 content hashing
- File size validation
- Timeout handling (30s)
- Progress reporting

### Extractor Module (8 LOC)

**URL Extraction:**
- Re-exports from `infrastructure/crawler/link_extractor.rs`

### TUI Module (674 LOC)

**Terminal UI with Ratatui:**

**Terminal Setup (`terminal.rs`):**
- Crossterm backend
- Signal handling for cleanup
- Alternate screen mode

**URL Selector (`url_selector.rs`):**
- Interactive URL selection
- Multi-select with checkboxes
- Search/filter functionality
- Real-time preview

---

## Extractor Library (`src/extractor/`)

**Standalone module** — URL extraction utilities

```
src/extractor/
└── mod.rs
```

---

## Error Handling

### Error Types (thiserror)

**Primary Error: `ScraperError`** (`src/error.rs`, 340 LOC)

```rust
#[derive(Error, Debug)]
pub enum ScraperError {
    #[error("URL inválida: {0}")]
    InvalidUrl(String),

    #[error("HTTP error {status} al acceder a {url}")]
    Http { status: reqwest::StatusCode, url: String },

    #[error("Error de legibilidad: {0}")]
    Readability(String),

    #[error("Error de I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("Error de red: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Error de middleware: {0}")]
    Middleware(String),

    #[error("Error de serialización: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Error de YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Error de parseo de URL: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("Error de extracción: {0}")]
    Extraction(String),

    #[error("Error de descarga: {0}")]
    Download(String),

    #[error("Error de configuración: {0}")]
    Config(String),

    #[error("Validación de URL falló: {0}")]
    Validation(String),

    #[error("Error de conversión: {0}")]
    Conversion(String),

    #[error("Error de exportación: {0}")]
    Export(String),

    #[error("Error de exportación en batch: {0}")]
    ExportBatch(String),

    #[cfg(feature = "ai")]
    #[error("Error de limpieza semántica: {0}")]
    Semantic(#[from] SemanticError),
}
```

**Secondary Errors:**

| Error Type | Location | LOC | Purpose |
|------------|----------|-----|---------|
| `SemanticError` | `src/error.rs` | ~100 | AI/ML operations |
| `CrawlError` | `src/domain/crawler_entities.rs` | ~80 | Crawl-specific |
| `ExporterError` | `src/domain/exporter.rs` | ~50 | Export operations |
| `TuiError` | `src/adapters/tui/mod.rs` | ~40 | TUI operations |
| `SitemapError` | `src/infrastructure/crawler/sitemap_parser.rs` | ~50 | Sitemap parsing |
| `DomainError` | `src/url_path.rs` | ~50 | Domain validation |
| `UrlPathError` | `src/url_path.rs` | ~50 | URL path operations |
| `OutputPathError` | `src/url_path.rs` | ~50 | Output path operations |

### Error Handling Patterns

**Following rust-skills rules:**

1. **err-thiserror-lib:** Library uses `thiserror` for type-safe errors
2. **err-question-mark:** `?` operator for clean propagation
3. **err-context-chain:** `.context()` for error chain context
4. **err-no-unwrap-prod:** No `.unwrap()` in production code
5. **err-lowercase-msg:** Error messages in lowercase, no trailing punctuation
6. **err-from-impl:** `#[from]` for automatic error conversion
7. **err-source-chain:** `#[source]` to chain underlying errors

**Example:**
```rust
pub fn scrape(url: &str) -> Result<ScrapedContent, ScraperError> {
    let valid_url = ValidUrl::parse(url)?;  // ? propagates UrlParse error
    let client = create_http_client()?;
    let response = client.get(valid_url.as_str()).send().await?;
    
    if !response.status().is_success() {
        return Err(ScraperError::Http {
            status: response.status(),
            url: url.to_string(),
        });
    }
    
    let html = response.text().await?;
    extract_content(&html, valid_url.as_url())
}
```

---

## Data Flow

### Content Scraping Workflow

```
URL Input (String)
    │
    ▼
┌─────────────────┐
│  Application    │  ValidUrl::parse() → Result<ValidUrl, ScraperError>
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Application    │  create_http_client() + retry middleware
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  reqwest HTTP fetch (rustls-tls-native-roots)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  legible::parse() (Readability algorithm)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  html_to_markdown::convert()
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  syntax_highlight::highlight()
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  frontmatter::generate() (YAML metadata)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  file_saver::save_results() (atomic write)
└─────────────────┘

Output: Markdown file with YAML frontmatter
```

### Web Crawler Workflow

```
CrawlerConfig
    │
    ▼
┌─────────────────┐
│  Application    │  crawl_site(config)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  url_queue::UrlQueue (concurrent dashmap)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  link_extractor::extract_links()
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Application    │  url_filter::is_internal_link()
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  scraper::extract_content()
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  export::JsonlExporter (streaming)
└─────────────────┘

Output: JSONL file with one document per line
```

### Asset Download Workflow

```
HTML Content
    │
    ▼
┌─────────────────┐
│   Adapters      │  extractor::extract_images() / extract_documents()
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│   Adapters      │  detector::detect_from_url() → AssetType
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  asset_download::download_all() (bounded concurrency)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  SHA256 hash + file save
└─────────────────┘

Output: Vec<DownloadedAsset> with local paths
```

### AI Semantic Cleaning Workflow (feature: ai)

```
Raw Content (String)
    │
    ▼
┌─────────────────┐
│   Domain        │  SemanticCleaner trait (sealed)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  SemanticCleanerImpl::clean()
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  tokenizer::tokenize() (sentence-transformers)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  model_cache::load() (memory-mapped, zero-copy)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  inference_engine::embed() (ONNX runtime)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  relevance_scorer::score() (cosine similarity)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Infrastructure  │  chunker::chunk() (semantic boundaries)
└─────────────────┘

Output: Vec<DocumentChunk> with embeddings
```

---

## Testing Strategy

### Test Counts (Verified: April 2026)

```bash
$ cargo nextest run 2>&1 | tail -5
test result: ok. 366 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Total:** 366 tests passing (includes 20 new WAF tests + 6 SPA detection tests)

### Test Distribution by Layer

| Layer | Test Count | Test Types |
|-------|------------|------------|
| **Domain** | ~56 | Entity creation, value object validation, serialization, SPA detection |
| **Application** | ~66 | HTTP client creation, service orchestration, URL filtering, SPA detection |
| **Infrastructure** | ~80 | Converter tests, file saver tests, crawler tests |
| **Adapters** | ~27 | Extractor tests, detector tests, TUI tests |

### Testing Patterns

**Following rust-skills rules:**

1. **test-cfg-test-module:** `#[cfg(test)] mod tests { }`
2. **test-tokio-async:** `#[tokio::test]` for async tests
3. **test-arrange-act-assert:** Three-phase test structure
4. **test-descriptive-names:** `test_scrape_with_config_invalid_url()`
5. **test-use-super:** `use super::*;` in test modules

**Example:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scrape_with_config_invalid_url() {
        // Arrange
        let invalid_url = "not-a-valid-url";
        
        // Act
        let result = scrape_with_config(invalid_url, &default_config()).await;
        
        // Assert
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ScraperError::InvalidUrl(_)));
    }
}
```

### Test Commands

```bash
# Run all library tests (2 threads for HDD optimization)
cargo test --lib --test-threads=2

# Run specific test
cargo test test_scrape_with_config_invalid_url

# Run with output
cargo test --lib -- --nocapture

# Run AI feature tests (requires ONNX models)
cargo test --features ai --lib
```

---

## Key Design Decisions

### 1. Why Clean Architecture?

**Following engineering-practices SOLID principles:**

1. **Separation of Concerns** — Domain logic isolated from frameworks
2. **Testability** — Mock infrastructure, test domain/application in isolation
3. **Maintainability** — Changes to HTTP client don't affect domain entities
4. **Reusability** — Domain entities usable in different contexts (CLI, web API, library)

**Verification:**
```bash
cd /home/gazadev/Dev/my_apps/rust_scraper
rg "^use (reqwest|tokio|scraper|tract)" src/domain/  # Returns nothing ✓
```

### 2. Why `ValidUrl` Newtype?

**Following type-newtype-ids and type-newtype-validated:**

Instead of `String` or raw `url::Url`:
- **Type Safety** — Can't accidentally pass invalid URL
- **Self-Documenting** — API signature guarantees validity
- **Compile-Time Validation** — Errors caught early

### 3. Why Bounded Concurrency?

**Following optimizing-low-resource-hardware:**

Hardware-aware design for target system (Intel i5-4590, 8GB RAM, HDD):
- **Prevents FD Exhaustion** — 100 URLs ≠ 100 open files
- **Avoids HDD Thrashing** — Sequential writes on mechanical drives
- **Reduces Bot Detection** — Doesn't look like DDoS

**Implementation:**
```rust
const MAX_CONCURRENT_SCRAPES: usize = 3;  // HDD-optimized
```

### 4. Why Retry with Exponential Backoff?

**Following err-context-chain and production resilience:**

- **Handles Transient Failures** — 5xx errors, timeouts, connection resets
- **Respectful** — Backoff prevents hammering servers
- **User-Friendly** — Scraping succeeds despite network hiccups

### 5. Why User-Agent Rotation?

**Following anti-patterns avoidance:**

Anti-bot evasion:
- **14 Modern Browsers** — Chrome (40%), Firefox (20%), Safari (20%), Edge (20%)
- **Weighted Selection** — Mimics real traffic distribution
- **Per-Request Rotation** — No patterns for detection

### 6. Why `once_cell::Lazy` for CSS Selectors?

**Following perf-iter-lazy and mem-reuse-collections:**

- **Compile Once** — `Selector::parse()` is expensive
- **No unwrap() in Prod** — `expect()` with clear error message
- **Thread-Safe** — Static initialization

### 7. Why Feature-Gated AI Module?

**Following api-serde-optional and YAGNI:**

- **Lightweight Core** — Default build has no ML dependencies
- **Optional Complexity** — Users opt-in to AI features
- **Compile Time** — Faster builds without AI

**Enable with:**
```bash
cargo build --features ai
```

### 8. Why Memory-Mapped Model Loading?

**Following mem-zero-copy and optimizing-low-resource-hardware:**

- **Zero-Copy** — No RAM duplication (8GB constraint)
- **HDD Optimization** — `ionice -c 3` for bulk I/O
- **Fast Startup** — Models load on-demand

---

## Dependencies by Layer

### Domain
```toml
serde = { version = "1", features = ["derive"] }
url = { version = "2", features = ["serde"] }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
```

### Application
```toml
reqwest-middleware = "0.4"
reqwest-retry = "0.7"
retry-policies = "0.4"
futures = "0.3"
```

### Infrastructure
```toml
reqwest = { version = "0.12", features = ["rustls-tls-native-roots", "gzip", "brotli", "stream", "json"] }
legible = "0.4"
htmd = "0.5"
html-to-markdown-rs = "2.3"
syntect = "5"
serde_yaml = "0.9"
sha2 = "0.10"
governor = "0.6"
dashmap = "6"
ratatui = "0.29"
crossterm = "0.28"
quick-xml = "0.37"
```

### Adapters
```toml
scraper = "0.22"
mimetype-detector = { version = "0.3", optional = true }
rand = "0.8"
```

### AI (feature-gated)
```toml
tract-onnx = { version = "0.21", optional = true }
tokenizers = { version = "0.21", optional = true }
hf-hub = { version = "0.5", features = ["tokio"], optional = true }
memmap2 = { version = "0.9", optional = true }
ndarray = { version = "0.17", optional = true }
unicode-segmentation = { version = "1.12", optional = true }
smallvec = { version = "1.13", optional = true }
wide = { version = "0.7", optional = true }
```

---

## Performance Optimizations

### Hardware-Aware Settings

**Following optimizing-low-resource-hardware:**

| Constraint | Optimization | Implementation |
|------------|--------------|----------------|
| **4C/4T CPU** | Max 3 threads | `num_cpus::get() - 1` |
| **8GB RAM** | Memory-mapped files | `memmap2` for models |
| **HDD** | Sequential I/O | `ionice -c 3` for bulk ops |
| **HDD** | Bounded concurrency | 3 concurrent requests |

### Cargo.toml Release Profile

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

**Following opt-lto-release and opt-codegen-units:**

- **LTO fat** — Cross-module optimization
- **codegen-units = 1** — Single compilation unit for max optimization
- **panic = abort** — Smaller binaries, no unwind
- **strip = true** — Remove debug symbols

### Runtime Optimizations

1. **Async I/O** — Tokio runtime for non-blocking operations
2. **Connection Pooling** — Reqwest reuses connections
3. **Compression** — Gzip/Brotli support reduces bandwidth
4. **Bounded Concurrency** — Prevents resource exhaustion
5. **Retry Backoff** — Reduces server load on failures
6. **Lazy Statics** — CSS selectors compiled once
7. **SHA256 Hashing** — Fast unique filenames
8. **Zero-Copy** — Memory-mapped model loading
9. **SIMD** — Cosine similarity with `wide` crate
10. **SIMD** — Cosine similarity with `wide` crate

---

## Module Dependency Graph

```
main.rs
  │
  ▼
lib.rs ───────────────────────┐
  │                           │
  ▼                           │
domain ◄──────────────────────┘
  │
  ▼
application
  │
  ├──────────────► infrastructure
  │                      │
  │                      ▼
  │                  ai (feature-gated)
  │
  └──────────────► adapters
```

**Verification:**
```bash
# Domain has no external dependencies
rg "^use (reqwest|tokio|scraper)" src/domain/  # Returns nothing ✓

# Application only imports domain
rg "^use rust_scraper::domain" src/application/  # Returns matches ✓

# Infrastructure imports both
rg "^use rust_scraper::(domain|application)" src/infrastructure/  # Returns matches ✓
```

---

## rust-skills Applied (179 Rules)

### CRITICAL Priority

**Ownership & Borrowing (own-*):**
- ✅ own-borrow-over-clone — `&[T]` over `&Vec<T>`, `&str` over `&String`
- ✅ own-slice-over-vec — Function parameters accept slices
- ✅ own-arc-shared — `Arc<T>` for thread-safe sharing in crawler
- ✅ own-mutex-interior — `Mutex<T>` for interior mutability where needed

**Error Handling (err-*):**
- ✅ err-thiserror-lib — `ScraperError` with `thiserror`
- ✅ err-question-mark — `?` operator throughout
- ✅ err-no-unwrap-prod — No `.unwrap()` in production code
- ✅ err-context-chain — `.context()` for error messages
- ✅ err-from-impl — `#[from]` for automatic conversion
- ✅ err-lowercase-msg — Error messages in lowercase
- ✅ err-custom-type — `JsRenderError` custom error enum for JS rendering

**Memory Optimization (mem-*):**
- ✅ mem-with-capacity — `Vec::with_capacity()` where size known
- ✅ mem-smallvec — `SmallVec` in AI module (feature-gated)
- ✅ mem-zero-copy — Memory-mapped model loading
- ✅ mem-smallvec — SmallVec for usually-small collections

### HIGH Priority

**API Design (api-*):**
- ✅ api-builder-pattern — `CrawlerConfigBuilder`
- ✅ api-newtype-safety — `ValidUrl`, `UserId` patterns
- ✅ api-from-not-into — `From` implementations, not `Into`
- ✅ api-must-use — `#[must_use]` on builder types
- ✅ api-non-exhaustive — `#[non_exhaustive]` on error types

**Async/Await (async-*):**
- ✅ async-no-lock-await — No `Mutex`/`RwLock` across `.await`
- ✅ async-spawn-blocking — `spawn_blocking` for CPU-intensive work
- ✅ async-tokio-fs — `tokio::fs` in async code
- ✅ async-bounded-channel — Bounded channels for backpressure
- ✅ async-clone-before-await — Clone data before await points
- ✅ async-native-ait — Native async fn in trait (Rust 1.88+), no `async-trait` crate
- ✅ async-native-ait — Native async fn in trait (Rust 1.88+), no `async-trait` crate

**Compiler Optimization (opt-*):**
- ✅ opt-lto-release — LTO enabled in release profile
- ✅ opt-codegen-units — `codegen-units = 1`
- ✅ opt-inline-small — `#[inline]` for small hot functions
- ✅ opt-simd-portable — SIMD for cosine similarity

### MEDIUM Priority

**Naming Conventions (name-*):**
- ✅ name-types-camel — `UpperCamelCase` for types
- ✅ name-funcs-snake — `snake_case` for functions
- ✅ name-consts-screaming — `SCREAMING_SNAKE_CASE` for constants
- ✅ name-acronym-word — `Uuid` not `UUID`

**Type Safety (type-*):**
- ✅ type-newtype-ids — `ValidUrl` newtype
- ✅ type-enum-states — Enums for mutually exclusive states
- ✅ type-option-nullable — `Option<T>` for nullable values
- ✅ type-result-fallible — `Result<T, E>` for fallible operations

**Testing (test-*):**
- ✅ test-cfg-test-module — `#[cfg(test)] mod tests { }`
- ✅ test-tokio-async — `#[tokio::test]` for async tests
- ✅ test-arrange-act-assert — Three-phase test structure
- ✅ test-descriptive-names — Descriptive test names

**Documentation (doc-*):**
- ✅ doc-all-public — `///` for all public items
- ✅ doc-examples-section — `# Examples` with runnable code
- ✅ doc-errors-section — `# Errors` for fallible functions
- ✅ doc-intra-links — `[ScraperError]` intra-doc links

**Performance Patterns (perf-*):**
- ✅ perf-iter-over-index — Iterators over manual indexing
- ✅ perf-entry-api — `entry()` API for map operations
- ✅ perf-drain-reuse — `drain()` to reuse allocations
- ✅ perf-profile-first — Profile before optimizing

### LOW Priority

**Project Structure (proj-*):**
- ✅ proj-lib-main-split — `main.rs` minimal, logic in `lib.rs`
- ✅ proj-mod-by-feature — Modules by feature, not type
- ✅ proj-pub-crate-internal — `pub(crate)` for internal APIs
- ✅ proj-pub-use-reexport — `pub use` for clean public API

**Clippy & Linting (lint-*):**
- ✅ lint-deny-correctness — `#![deny(clippy::correctness)]`
- ✅ lint-warn-perf — `#![warn(clippy::perf)]`
- ✅ lint-warn-suspicious — `#![warn(clippy::suspicious)]`
- ✅ lint-rustfmt-check — `cargo fmt --check` in CI

### Anti-patterns Avoided (anti-*)

- ✅ anti-unwrap-abuse — No `.unwrap()` in production
- ✅ anti-lock-across-await — No locks held across `.await`
- ✅ anti-clone-excessive — Borrow over clone
- ✅ anti-format-hot-path — No `format!()` in hot paths
- ✅ anti-vec-for-slice — `&[T]` over `&Vec<T>`
- ✅ anti-string-for-str — `&str` over `&String`
- ✅ anti-collect-intermediate — No intermediate `collect()`
- ✅ anti-premature-optimize — Profile before optimizing

---

## Related Documentation

- [`README.md`](../README.md) — User guide and examples
- [`CHANGELOG.md`](../CHANGELOG.md) — Version history
- [`docs/`](../docs/) — Additional documentation
- [`rust-skills/`](../rust-skills/) — 179 Rust rules applied

---

## Verification Commands

**Verify architecture:**
```bash
cd /home/gazadev/Dev/my_apps/rust_scraper

# Check domain has no external dependencies
rg "^use (reqwest|tokio|scraper|tract)" src/domain/

# Count lines per layer
wc -l src/domain/*.rs src/application/*.rs src/infrastructure/*.rs src/adapters/*.rs

# Run tests
cargo test --lib --test-threads=2

# Check Clippy
cargo clippy --all-targets --all-features -- -D correctness
```

**Last verified:** April 1, 2026  
**Tests passing:** 271/271  
**Clippy:** Clean

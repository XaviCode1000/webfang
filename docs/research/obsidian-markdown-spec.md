# Research Report: Obsidian-Compatible Markdown Export

## 1. Obsidian Markdown Spec — What "Obsidian-Compatible" Actually Means

### 1.1 Core Format: YAML Frontmatter (Properties)

Obsidian uses YAML frontmatter at the top of every `.md` file, called **Properties**. This is the single most important distinguishing feature.

```yaml
---
title: "Article Title"
tags:
  - web-clip
  - technology
aliases:
  - "Alternative Title"
created: 2026-04-03T17:00:00
modified: 2026-04-03T17:00:00
source: "https://example.com/article"
author: "John Doe"
description: "Page excerpt or meta description"
image: "https://example.com/og-image.jpg"
language: en
---
```

**Property types Obsidian supports natively:**
| Type | YAML Format | Example |
|------|------------|---------|
| Text | `key: value` | `source: "https://..."` |
| List | `key: [a, b]` or multi-line | `tags: [web, tech]` |
| Number | `key: 42` | `word_count: 1234` |
| Checkbox | `key: true` | `archived: false` |
| Date | `key: 2026-04-03` | `published: 2026-01-15` |
| Date & time | `key: 2026-04-03T17:00:00` | `created: 2026-04-03T17:00:00` |
| Tags | `key: [#tag1, #tag2]` | `tags: [#web-clip, #tech]` |

**Standard frontmatter fields for web clippings:**
- `title` — Page title (string)
- `tags` — Array of tags for categorization
- `aliases` — Alternative titles for linking
- `created` — When the clip was made (date+time)
- `source` / `url` — Original URL (string)
- `author` — Article author (string)
- `description` — Excerpt/meta description (string)
- `image` — Social share / OG image URL (string)
- `published` — Original publication date (date)
- `domain` — Source domain (string)

### 1.2 Wikilinks: `[[Like This]]`

Obsidian supports **wikilinks** as an alternative to standard markdown links:

```markdown
[[Note Title]]                    # Link to note
[[Note Title|Display Text]]       # Link with alias
[[Note Title#Heading]]            # Link to specific heading
[[Note Title#^block-id]]          # Link to specific block
![[image.png]]                    # Embed/transclude image
![[document.pdf]]                 # Embed PDF (renders inline)
```

For a web scraper, wikilinks are useful for:
- Cross-referencing clips from the same domain
- Linking to author notes: `[[John Doe]]`
- Tagging via links: `[[technology]]`

### 1.3 Callouts (Admonitions)

Obsidian supports a special blockquote syntax for callouts:

```markdown
> [!note]
> This is a note callout.

> [!warning]
> This is a warning.

> [!tip]
> This is a tip.

> [!info]
> This is informational.

> [!quote]
> This is a quote callout.

> [!important]
> This is important.

> [!caution]
> This is a caution.

> [!bug]
> This is a bug report.

> [!example]
> This is an example.

> [!question]
> This is a question.

> [!failure] / > [!fail]
> This is a failure.

> [!success]
> This is a success.

> [!abstract] / > [!summary] / > [!tldr]
> This is an abstract.
```

**Foldable callouts:**
```markdown
> [!note]- This is a foldable callout
> Content is hidden until expanded.
```

**For a web scraper:** WAF/CAPTCHA detection results, rate-limit warnings, or content quality notes could be expressed as callouts.

### 1.4 Obsidian Flavored Markdown Extensions

Beyond standard CommonMark + GFM, Obsidian supports:
- **Wikilinks** (`[[...]]`)
- **Callouts** (`> [!type]`)
- **Embeds** (`![[...]]`)
- **Tags** (`#tag-name`, `#nested/tag`)
- **MathJax/LaTeX** (`$inline$`, `$$display$$`)
- **Footnotes** (`[^1]`)
- **Highlight** (`==highlighted text==`)
- **Strikethrough** (`~~deleted~~`)
- **Comments** (`%% hidden comment %%`)
- **Code block with language** (```` ```rust ````)
- **Tables** (GFM)
- **Task lists** (`- [ ]`, `- [x]`)

### 1.5 Attachment Handling

- Images are typically stored in an **attachments folder** (configurable in Obsidian settings)
- Convention: `_attachments/`, `assets/`, or `{{title}}/` per-note folders
- Obsidian Web Clipper roadmap includes "Save images locally" (added in Obsidian 1.8.0)
- Wikilink embed syntax: `![[filename.png]]`
- Standard markdown also works: `![alt](path/to/image.png)`

### 1.6 Folder Structure Conventions

Common Obsidian vault structures for web clippings:

```
Vault/
├── webclips/
│   ├── 2026-04-03-article-title.md
│   ├── 2026-04-03-another-article.md
│   └── _attachments/
│       ├── article-title-hero.png
│       └── another-article-diagram.jpg
├── sources/
│   ├── example-com/
│   │   └── article-title.md
│   └── another-site/
│       └── page.md
└── _templates/
    └── web-clip.md
```

Naming conventions:
- `YYYY-MM-DD-title.md` — Date-prefixed for chronological sorting
- `title-slugified.md` — Simple slugified titles
- `domain/title.md` — Organized by source domain

---

## 2. Competitor Matrix

### Browser Extensions

| Feature | MarkDownload | Obsidian Web Clipper (Official) | SingleFile |
|---------|-------------|--------------------------------|------------|
| **Stars** | 3.7K+ | 3.4K+ | 10K+ |
| **Output** | `.md` file download | `.md` file in vault | `.html` (single file) |
| **Frontmatter** | ✅ Custom templates (front/back matter) | ✅ YAML Properties | ❌ N/A (HTML) |
| **Wikilinks** | ❌ | ❌ | ❌ |
| **Callouts** | ❌ | ❌ | ❌ |
| **Image handling** | Download locally or keep URLs | Download locally (Obsidian 1.8+) | Embedded as base64 |
| **Content extraction** | Readability.js + Turndown | Defuddle (custom) | Full page HTML |
| **Templates** | ✅ Front/back matter with variables | ✅ Rich templates with variables | ❌ |
| **Math/LaTeX** | ✅ MathJax → LaTeX | ✅ | ✅ (in HTML) |
| **Tables** | ✅ GFM plugin | ✅ | ✅ (in HTML) |
| **Obsidian direct** | ✅ Via Advanced URI + clipboard | ✅ Native integration | ❌ |
| **Batch mode** | ✅ Download all tabs | ❌ | ✅ |
| **Highlight mode** | ❌ | ✅ Highlight before clipping | ❌ |
| **License** | Apache 2.0 | MIT | MPL 2.0 |

### Read-it-Later Services

| Feature | Readwise Reader | Omnivore |
|---------|----------------|----------|
| **Obsidian export** | ✅ Via official integration | ✅ Via community plugin (obsidian-omnivore) |
| **Frontmatter** | ✅ Customizable | ✅ Customizable via template |
| **Highlights** | ✅ With block references | ✅ With highlights section |
| **Full content** | ✅ | ✅ Via `{{{content}}}` variable |
| **Tags/Labels** | ✅ As Obsidian tags | ✅ As wikilinks in frontmatter |
| **Sync** | ✅ Scheduled sync | ✅ API-based sync |
| **Template vars** | Rich templating | Handlebars-style `{{{var}}}` |
| **Status** | Paid ($7.99/mo, 50% students) | **Shut down** (acquired by ElevenLabs) |

### API-Based Scrapers

| Feature | Firecrawl | Jina Reader | Crawlee/Apify |
|---------|-----------|-------------|---------------|
| **Markdown output** | ✅ Primary format | ✅ Primary format | ❌ Raw HTML/JSON |
| **Format options** | Markdown, HTML, JSON, Screenshot | Markdown, Text, HTML | JSON, CSV, HTML |
| **Content extraction** | Yes (clean markdown) | Yes (LLM-powered) | No (raw scrape) |
| **Frontmatter** | ❌ | ❌ | ❌ |
| **Metadata** | ✅ URL, title, description | ✅ URL, title | ✅ Full page data |
| **Image handling** | Keep URLs | Keep URLs | Downloadable |
| **Pricing** | Paid (free tier) | Free tier + paid | Paid (Apify platform) |
| **Use case** | LLM/RAG pipelines | LLM/RAG pipelines | General scraping |

### Notion → Obsidian Exporters

| Tool | Approach | Frontmatter | Wikilinks | Images |
|------|----------|-------------|-----------|--------|
| notion2obsidian | Notion API → MD | ✅ | ✅ (converts @mentions) | ✅ Downloads locally |
| notion-to-obsidian | CLI tool | ✅ | ✅ | ✅ |
| Obsidian Notion Importer | Built-in | ❌ (basic) | ❌ | ❌ (broken links) |

### Key Insight: Defuddle

**Defuddle** (by Kepano, Obsidian co-founder) is the content extraction engine behind the official Obsidian Web Clipper. It was recently open-sourced (March 2026).

- **What it does:** Extracts main content from any URL and returns clean Markdown with YAML frontmatter
- **API:** `curl defuddle.md/stephango.com` → Returns Markdown with frontmatter
- **npm:** `defuddle` package (21.4K weekly downloads, v0.15.0)
- **License:** MIT
- **Relevance:** This is the gold standard for Obsidian-compatible markdown extraction. It handles tables, code blocks, footnotes, and math equations.

---

## 3. Feature Recommendations (Ranked by Priority)

### P0 — Must Have (Launch)

1. **YAML Frontmatter Generation**
   - Fields: `title`, `source`/`url`, `created`, `author`, `description`, `tags`, `domain`
   - Maps directly from existing `ScrapedContent` fields
   - **Why:** This is the #1 thing that makes markdown "Obsidian-compatible"

2. **One File Per URL**
   - Each scraped page → one `.md` file
   - Filename: slugified from title or URL path
   - **Why:** Matches Obsidian's note-per-file model; all competitors do this

3. **Clean Markdown Content**
   - Convert HTML → Markdown (tables, code blocks, lists, links)
   - Strip nav, ads, footers, scripts
   - **Why:** Core value proposition; Readability.js + Turndown is the proven stack

4. **Relative Link Resolution**
   - Convert absolute internal links to relative paths
   - Keep external links as standard markdown `[text](url)`
   - **Why:** Broken links are the #1 complaint with web clippers

### P1 — Should Have (First Iteration)

5. **Image Download with Local Paths**
   - Download images to `_attachments/` or `assets/` folder
   - Update markdown image refs to local paths
   - **Why:** Offline readability; Obsidian Web Clipper just added this in 1.8.0

6. **Date-Prefixed Filenames**
   - `YYYY-MM-DD-title-slug.md` format
   - Configurable naming strategy
   - **Why:** Chronological sorting in Obsidian file explorer

7. **Tags from Content**
   - Extract tags from page meta keywords
   - Map to Obsidian `tags` property
   - **Why:** Enables Obsidian's tag-based navigation

8. **Domain-Based Folder Structure**
   - `output/domain.com/page-title.md`
   - **Why:** Organizes clips by source; matches common vault patterns

### P2 — Nice to Have (Future)

9. **Wikilink Generation**
   - Convert internal links to `[[wikilink]]` format
   - Cross-reference clips from same domain
   - **Why:** Enables Obsidian's graph view and internal linking

10. **Callout Integration**
    - WAF detection → `> [!warning]` callout
    - Content quality notes → `> [!info]` callout
    - **Why:** Leverages Obsidian-specific features

11. **Incremental Updates / Deduplication**
    - Use existing `StateStore` to track processed URLs
    - Skip or update existing files
    - **Why:** Already have StateStore infrastructure; just needs file-level tracking

12. **Math/LaTeX Preservation**
    - Detect MathJax/KaTeX and preserve as `$...$` or `$$...$$`
    - **Why:** Academic content scraping

---

## 4. Technical Design Suggestions

### 4.1 Architecture

Following the existing Clean Architecture pattern:

```
src/
├── domain/
│   ├── entities.rs          # Add OutputFormat::Markdown variant
│   ├── exporter.rs          # Exporter trait (already exists)
│   └── markdown.rs          # NEW: MarkdownDocument entity
├── infrastructure/
│   └── export/
│       ├── markdown_exporter.rs   # NEW: MarkdownExporter impl
│       ├── html_to_md.rs          # NEW: HTML → Markdown converter
│       └── ...
└── export_factory.rs        # Add Markdown case to create_exporter()
```

### 4.2 Proposed `OutputFormat` Enum (separate from `ExportFormat`)

The existing `ExportFormat` is designed for RAG pipelines (JSONL, Vector). A new `OutputFormat` enum should handle file-per-URL output modes:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    /// One JSONL file (RAG pipeline)
    Jsonl,
    /// One markdown file per URL (Obsidian-compatible)
    Markdown,
}
```

### 4.3 MarkdownExporter Design

```rust
pub struct MarkdownExporter {
    config: MarkdownExporterConfig,
}

pub struct MarkdownExporterConfig {
    pub output_dir: PathBuf,
    pub attachments_dir: String,        // e.g., "_attachments"
    pub filename_strategy: FilenameStrategy,
    pub download_images: bool,
    pub use_wikilinks: bool,
}

pub enum FilenameStrategy {
    SlugifiedTitle,          // "article-title.md"
    DatePrefixed,            // "2026-04-03-article-title.md"
    DomainPath,              // "domain.com/path/to/page.md"
}
```

### 4.4 HTML → Markdown Conversion

**Recommended stack:**
- **Content extraction:** `readability` crate (Rust port of Mozilla Readability) OR `defuddle` (if a Rust port becomes available)
- **HTML → Markdown:** `html2md` or `mdka` crate, OR integrate `turndown` via WASM
- **Alternative:** Use the existing scraper HTML parsing + manual markdown generation

**Given the Rust ecosystem, the most practical approach:**
1. Use `readability` crate for content extraction
2. Use `html2md` crate for HTML → Markdown conversion
3. Post-process: fix links, handle images, add frontmatter

### 4.5 Proposed Markdown Output Structure

```markdown
---
title: "Article Title"
source: "https://example.com/article"
created: 2026-04-03T17:00:00+00:00
author: "John Doe"
description: "Page excerpt or meta description"
tags:
  - web-clip
  - technology
domain: "example.com"
---

# Article Title

Main content converted to clean markdown...

## Section Heading

More content with proper formatting.

![Image description](_attachments/article-title-hero.png)

> [!warning] WAF Detected
> This page had Cloudflare protection. Content may be incomplete.

---
*Clipped from [example.com](https://example.com/article) on 2026-04-03*
```

### 4.6 Image Handling Strategy

```
output/
├── domain.com/
│   ├── article-title.md
│   └── _attachments/
│       ├── hero-image.png
│       └── diagram.jpg
```

- Download images asynchronously with bounded concurrency
- Generate deterministic filenames from URL hash or title slug
- Update all `![alt](url)` references to local paths
- Fall back to keeping remote URLs if download fails

### 4.7 Integration with Existing Code

The existing `ScrapedContent` struct already has all needed fields:
- `title`, `content`, `url`, `excerpt`, `author`, `date`, `assets`

The `MarkdownExporter` would:
1. Take `ScrapedContent` as input
2. Generate YAML frontmatter from metadata
3. Convert `content` (HTML) → Markdown
4. Download images → `_attachments/`
5. Write `.md` file to output directory
6. Update `StateStore` for incremental support

---

## 5. Proposed GitHub Issue

```markdown
## Feature: Obsidian-Compatible Markdown Export

### Problem

The scraper currently exports to JSONL and Vector formats, which are designed for RAG/embedding pipelines. There is no way to export scraped content as clean, Obsidian-compatible Markdown files — a format increasingly demanded by knowledge workers, researchers, and PKM (Personal Knowledge Management) users.

### Context

Research into the competitive landscape reveals:

- **Obsidian Web Clipper** (official, 3.4K stars) uses Defuddle for content extraction → clean Markdown with YAML frontmatter
- **MarkDownload** (3.7K stars) uses Readability.js + Turndown, supports custom frontmatter templates
- **Firecrawl** and **Jina Reader** provide markdown output but without Obsidian-specific features (no frontmatter, no wikilinks)
- **Omnivore** (now defunct) had a robust Obsidian plugin with Handlebars-style templates

The gap: No Rust-based web scraper offers Obsidian-compatible markdown export with proper frontmatter, image handling, and vault-friendly file structure.

### Proposed Solution

Add a `Markdown` output format that produces one `.md` file per URL, with:

#### P0 (Launch)
- [ ] YAML frontmatter with: `title`, `source`, `created`, `author`, `description`, `tags`, `domain`
- [ ] One file per URL, named from slugified title
- [ ] Clean markdown content (HTML → Markdown conversion)
- [ ] Relative link resolution for internal links

#### P1 (First Iteration)
- [ ] Image download to `_attachments/` folder with local path refs
- [ ] Date-prefixed filename option (`YYYY-MM-DD-title.md`)
- [ ] Tags extraction from page meta keywords
- [ ] Domain-based folder structure (`output/domain.com/page.md`)

#### P2 (Future)
- [ ] Wikilink generation (`[[like this]]`)
- [ ] Callout integration for WAF/quality notes
- [ ] Incremental updates via existing StateStore
- [ ] Math/LaTeX preservation

### Technical Approach

1. Add `OutputFormat::Markdown` variant (separate from RAG-focused `ExportFormat`)
2. Create `MarkdownExporter` implementing the existing `Exporter` trait
3. Use `readability` crate for content extraction + `html2md` for conversion
4. Generate YAML frontmatter from `ScrapedContent` metadata
5. Download images asynchronously with bounded concurrency
6. Write files following Obsidian vault conventions

### Architecture

```
src/
├── domain/
│   ├── markdown.rs          # NEW: MarkdownDocument entity
│   └── entities.rs          # Add OutputFormat enum
├── infrastructure/
│   └── export/
│       ├── markdown_exporter.rs   # NEW: Exporter impl
│       └── html_to_md.rs          # NEW: HTML → Markdown
└── export_factory.rs        # Add Markdown case
```

### Acceptance Criteria

- [ ] `--output-format markdown` CLI flag produces `.md` files
- [ ] Each file has valid YAML frontmatter with standard fields
- [ ] Content is clean markdown (no nav, ads, scripts)
- [ ] Images are downloaded to local folder with correct refs
- [ ] Files are organized in vault-friendly structure
- [ ] Existing JSONL/Vector export paths remain unchanged
- [ ] All tests pass with `cargo nextest run --test-threads 2`
- [ ] Clippy clean: `cargo clippy -- -D warnings`

### Dependencies

- `readability` (crate) — Content extraction
- `html2md` or `mdka` (crate) — HTML to Markdown conversion
- `slug` (crate) — Filename slugification
- Existing: `chrono`, `serde_yaml` (for frontmatter)
```

---

## Appendix: Key Libraries & Tools Reference

| Tool | Language | Purpose | License |
|------|----------|---------|---------|
| **Defuddle** | TypeScript | Content extraction → Markdown (Obsidian official) | MIT |
| **Readability.js** | JavaScript | Content extraction (Mozilla) | Apache 2.0 |
| **Turndown** | JavaScript | HTML → Markdown | MIT |
| **readability** | Rust | Content extraction (Rust port) | MIT |
| **html2md** | Rust | HTML → Markdown | MIT |
| **mdka** | Rust | HTML → Markdown (alternative) | MIT |
| **slug** | Rust | URL/filename slugification | MPL 2.0 |
| **serde_yaml** | Rust | YAML serialization | MIT/Apache |

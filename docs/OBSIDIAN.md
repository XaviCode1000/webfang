# Obsidian Integration — rust-scraper

**Version:** 1.1.0  
**Last Updated:** 2026-04-04

---

## Overview

rust-scraper provides native Obsidian integration for frictionless URL-to-vault workflows. Scrape any webpage and save it directly to your Obsidian vault with wiki-links, rich metadata, and relative asset paths.

## Features

### 1. Vault Auto-Detect

Automatically finds your Obsidian vault using 4-tier resolution:

1. **CLI flag:** `--vault ~/Obsidian/MyVault` (highest priority)
2. **Environment variable:** `OBSIDIAN_VAULT=~/Obsidian/MyVault`
3. **Config file:** `~/.config/rust-scraper/config.toml` → `[obsidian]` section
4. **Auto-scan:** Searches upward from current directory for `.obsidian/app.json`

**Validation:** A valid vault must contain `.obsidian/app.json` — not just the `.obsidian/` directory.

### 2. Quick-Save Mode

The fastest way to clip a URL to your vault:

```bash
cargo run --release -- --url https://example.com/article --obsidian --quick-save
```

**Behavior:**
- Bypasses TUI and confirmation prompts
- Saves to `{vault}/_inbox/YYYY-MM-DD-slug.md`
- Creates `_inbox/` directory automatically
- Opens note in Obsidian if running (Linux via `xdg-open`)
- Falls back to `./output/` if no vault detected

### 3. Wiki-Links Conversion

Converts standard Markdown links to Obsidian wiki-links for same-domain URLs:

| Input | Output |
|-------|--------|
| `[API Docs](https://docs.rs/api/auth)` | `[[auth\|API Docs]]` |
| `[Google](https://google.com)` | `[Google](https://google.com)` (unchanged — external) |
| `` `[code](url)` `` | Unchanged — inside code block |

**Rules:**
- Only converts links where the domain matches the scraped page's domain
- Strips query parameters and fragments from slugs
- Root URLs (`/`) fallback to `index`
- Skips links inside code blocks (inline and fenced)

### 4. Relative Asset Paths

Rewrites absolute asset paths to be relative to the `.md` file:

| Before | After |
|--------|-------|
| `![](/home/user/output/images/photo.png)` | `![](../_attachments/photo.png)` |

Uses `pathdiff` crate for cross-platform compatibility. Always outputs `/` separators (Obsidian requirement).

### 5. Rich Metadata for Dataview

Extended YAML frontmatter with fields optimized for Dataview queries:

```yaml
---
title: "Article Title"
url: "https://example.com/article"
date: "2026-04-04"
author: "John Doe"
excerpt: "Page excerpt if available"
tags:
  - web-clip
  - rust
readingTime: 5
language: en
wordCount: 1234
contentType: article
status: unread
---
```

**New fields:**
| Field | Type | Description |
|-------|------|-------------|
| `readingTime` | Integer | Estimated minutes (word_count / 200 WPM, minimum 1) |
| `language` | String | ISO 639-1 code (en, es, fr, de, pt, zh, ja) — only if `whatlang` confidence ≥ 0.5 |
| `wordCount` | Integer | Total word count of content |
| `contentType` | String | `article`, `product`, `recipe`, `paper`, `documentation` |
| `status` | String | Default: `unread` |

**Example Dataview queries:**

```dataview
TABLE readingTime, language, status FROM "webclips"
WHERE status = "unread"
SORT created DESC
```

```dataview
TABLE length(tags) as tagCount FROM "webclips"
WHERE language = "en" AND readingTime < 10
```

## CLI Reference

### Quick Start

```bash
# Simplest: quick-save to auto-detected vault
cargo run --release -- --url https://example.com --obsidian --quick-save

# With explicit vault
cargo run --release -- --url https://example.com --vault ~/Obsidian/Brain --quick-save

# Full control
cargo run --release -- --url https://example.com \
  --vault ~/Obsidian/Brain \
  --obsidian-wiki-links \
  --obsidian-tags "rust,web,scraping" \
  --obsidian-relative-assets \
  --quick-save
```

### Environment Variables

```bash
# Set vault path persistently
export OBSIDIAN_VAULT=~/Obsidian/MyKnowledge

# Then just quick-save
cargo run --release -- --url https://example.com --obsidian --quick-save
```

### Config File

```toml
# ~/.config/rust-scraper/config.toml
[obsidian]
vault_path = "~/Obsidian/MyVault"
wiki_links = true
relative_assets = true
tags = ["web-clip"]
```

## Architecture

```
src/infrastructure/obsidian/
├── mod.rs              # Module root
├── vault_detector.rs   # 4-tier vault path resolution
├── metadata.rs         # Language, word count, reading time, content type
└── uri.rs              # Obsidian URI builder + xdg-open
```

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `pathdiff` | 0.2 | Cross-platform relative paths |
| `whatlang` | 0.18 | Language detection (pure Rust, ~81KB) |
| `urlencoding` | 2.1 | URL encoding for Obsidian URI |
| `slug` | 0.1 | URL slug generation for filenames |

## Troubleshooting

### Vault not detected
- Ensure `.obsidian/app.json` exists in your vault
- Try explicit `--vault` flag first to verify the path works
- Check permissions on the vault directory

### Quick-save falls back to ./output/
- No vault was detected — check vault detection order
- Set `OBSIDIAN_VAULT` environment variable as a persistent solution

### Language shows as empty in frontmatter
- `whatlang` requires sufficient text for reliable detection
- Short content (< 100 chars) may not have enough data
- Language field is omitted if confidence < 0.5 (prevents false positives)

### Obsidian doesn't open after save
- URI opening is best-effort and non-blocking
- Requires `xdg-open` on Linux
- Obsidian must be configured to handle `obsidian://` URIs
- Check logs for warning messages

## See Also

- [Obsidian URI Documentation](https://help.obsidian.md/Advanced+topics/Using+obsidian+URI)
- [Dataview Plugin](https://github.com/blacksmithgu/obsidian-dataview)
- [Obsidian Clipper Issues](https://github.com/obsidianmd/obsidian-clipper/issues/112)

# TUI Unified Design — Collapsible Config + URL Selector

## Problem

Two separate flags (`--interactive` and `--config-tui`) that should be one flow.
Config form covers only 12/45 CLI flags (27%).

## Solution: Single `--tui` flag with two-phase flow

```text
Phase 1: Config Form (collapsible sections)
  ├─ ▶ Target (collapsed by default)
  │    └─ url, selector
  ├─ ▶ Output (collapsed by default)
  │    └─ output, format, export_format
  ├─ ▼ Discovery (expanded — most used)
  │    └─ use_sitemap, sitemap_url, max_pages, max_depth
  ├─ ▶ Crawler (collapsed)
  │    └─ timeout_secs, max_retries, delay_ms, concurrency
  ├─ ▶ Network (collapsed)
  │    └─ user_agent, accept_language, h2_profile, js_strategy
  ├─ ▶ Download (collapsed)
  │    └─ download_images, download_documents, max_file_size
  ├─ ▶ Obsidian (collapsed)
  │    └─ obsidian_wiki_links, obsidian_tags, vault, quick_save, etc.
  ├─ ▶ Advanced (collapsed)
  │    └─ elastic, pipeline, batch, checkpoint, autoscale
  └─ [Start Scraping] button

Phase 2: URL Selector (after config submitted)
  └─ Select which URLs to scrape from discovered list
```

## Collapsible Section Implementation

Since `ratatui-accordion` is reserved, implement with ratatui's `List` widget:

```rust,ignore
struct ConfigSection {
    title: String,
    expanded: bool,
    fields: Vec<FormField>,
}

struct CollapsibleConfig {
    sections: Vec<ConfigSection>,
    cursor: usize,  // which section is focused
}

impl CollapsibleConfig {
    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down => self.cursor = (self.cursor + 1).min(self.sections.len() - 1),
            KeyCode::Enter | KeyCode::Right => self.sections[self.cursor].expanded = true,
            KeyCode::Left => self.sections[self.cursor].expanded = false,
            KeyCode::Char(' ') => self.sections[self.cursor].expanded ^= true,
            _ => {}
        }
    }
    
    fn render(&self, frame: &mut Frame, area: Rect) {
        // Each section renders as:
        // ▶ Section Title          (collapsed)
        // ▼ Section Title          (expanded)
        //   ├─ field1: value
        //   ├─ field2: value
        //   └─ field3: value
    }
}
```

## Field Mapping (45 fields → 8 sections)

| Section | Fields | Default State |
|:--------|:-------|:-------------|
| Target | url, selector | Expanded |
| Output | output, format, export_format | Collapsed |
| Discovery | use_sitemap, sitemap_url, max_pages, max_depth, sitemap_depth | Expanded |
| Crawler | timeout_secs, max_retries, delay_ms, concurrency, include/exclude patterns | Collapsed |
| Network | user_agent, accept_language, h2_profile, js_strategy, force_js_render | Collapsed |
| Download | download_images, download_documents, max_file_size, download_timeout | Collapsed |
| Obsidian | obsidian_wiki_links, obsidian_tags, obsidian_relative_assets, obsidian_rich_metadata, vault, quick_save | Collapsed |
| Advanced | elastic, cpu_cores, ram_budget, db_path, pipeline, pipeline_output, batch, batch_file, batch_concurrency, checkpoint_interval, no_checkpoint, ignore_robots, autoscale, no_session_health, verbose, quiet, dry_run, trace_file | Collapsed |

## Keyboard Navigation

| Key | Action |
|:----|:-------|
| ↑/↓ | Navigate between sections |
| Enter/→ | Expand section |
| ← | Collapse section |
| Space | Toggle expand/collapse |
| Tab | Move to first field in expanded section |
| Shift+Tab | Move to previous field |
| Esc | Back to section list |
| Ctrl+S | Submit form |

## Migration Plan

1. Create `src/adapters/tui/collapsible_config.rs` — new collapsible form
2. Update `src/adapters/tui/config_form.rs` — use collapsible sections
3. Update `src/main.rs` — unify `--interactive` + `--config-tui` → `--tui`
4. Update `src/cli/args.rs` — replace two flags with one
5. Update `src/cli/preflight.rs` — handle all 45 fields in merge
6. Add tests for collapsible navigation and field mapping
7. Deprecate old flags (keep for backward compatibility)

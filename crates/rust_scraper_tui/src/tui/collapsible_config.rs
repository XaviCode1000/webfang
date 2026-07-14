//! Collapsible Configuration Form
//!
//! A config form with expandable/collapsible sections.
//! Each section contains a group of related CLI flags.
//!
//! # Keyboard Navigation
//!
//! | Key | Action |
//! |-----|--------|
//! | ↑/↓ | Navigate between sections |
//! | Enter/→ | Expand section |
//! | ← | Collapse section |
//! | Space | Toggle expand/collapse |
//! | Tab | Move to fields in expanded section |
//! | Esc | Back to section list |
//! | Ctrl+S | Submit form |

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph};
use ratatui::Frame;
use ratatui_form::{Form, FormResult};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use super::action::Action;
use super::component::Component;
use super::theme::Theme;

// ============================================================================
// Section definition
// ============================================================================

/// A collapsible section containing related form fields.
pub struct ConfigSection {
    /// Section title (displayed in header)
    pub title: String,
    /// Whether section is currently expanded
    pub expanded: bool,
    /// The form for this section's fields
    pub form: Form,
    /// Number of fields in this section
    pub field_count: usize,
}

impl ConfigSection {
    /// Create a new section with a form.
    pub fn new(title: impl Into<String>, form: Form, expanded: bool) -> Self {
        let field_count = 0; // Will be set after form creation
        Self {
            title: title.into(),
            expanded,
            form,
            field_count,
        }
    }

    /// Create a section with field count.
    pub fn with_fields(
        title: impl Into<String>,
        form: Form,
        expanded: bool,
        field_count: usize,
    ) -> Self {
        Self {
            title: title.into(),
            expanded,
            form,
            field_count,
        }
    }
}

// ============================================================================
// Collapsible config state
// ============================================================================

/// Mode for the collapsible config form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigMode {
    /// Navigating between sections
    SectionList,
    /// Editing fields in an expanded section
    FieldEdit,
}

/// A configuration form with collapsible sections.
pub struct CollapsibleConfig {
    /// All configuration sections
    sections: Vec<ConfigSection>,
    /// Currently focused section index
    cursor: usize,
    /// Current interaction mode
    mode: ConfigMode,
    /// Whether form was submitted
    pub submitted: bool,
    /// Whether form was cancelled
    pub cancelled: bool,
    /// Action channel sender
    action_tx: Option<UnboundedSender<Action>>,
}

impl Default for CollapsibleConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CollapsibleConfig {
    /// Create a new collapsible config with all sections.
    pub fn new() -> Self {
        Self {
            sections: Self::build_sections(),
            cursor: 0,
            mode: ConfigMode::SectionList,
            submitted: false,
            cancelled: false,
            action_tx: None,
        }
    }

    /// Build all configuration sections with their forms.
    fn build_sections() -> Vec<ConfigSection> {
        vec![
            // Section 1: Target (expanded by default)
            ConfigSection::with_fields(
                "Target",
                Self::build_target_form(),
                true,
                2, // url, selector
            ),
            // Section 2: Output
            ConfigSection::with_fields(
                "Output",
                Self::build_output_form(),
                false,
                3, // output, format, export_format
            ),
            // Section 3: Discovery (expanded by default)
            ConfigSection::with_fields(
                "Discovery",
                Self::build_discovery_form(),
                true,
                5, // use_sitemap, sitemap_url, max_pages, max_depth, sitemap_depth
            ),
            // Section 4: Crawler
            ConfigSection::with_fields(
                "Crawler",
                Self::build_crawler_form(),
                false,
                6, // timeout, retries, delay, concurrency, include, exclude
            ),
            // Section 5: Network
            ConfigSection::with_fields(
                "Network",
                Self::build_network_form(),
                false,
                5, // user_agent, accept_language, h2_profile, js_strategy, force_js
            ),
            // Section 6: Download
            ConfigSection::with_fields(
                "Download",
                Self::build_download_form(),
                false,
                4, // images, documents, max_file_size, download_timeout
            ),
            // Section 7: Obsidian
            ConfigSection::with_fields(
                "Obsidian",
                Self::build_obsidian_form(),
                false,
                6, // wiki_links, tags, relative_assets, rich_metadata, vault, quick_save
            ),
            // Section 8: Advanced
            ConfigSection::with_fields(
                "Advanced",
                Self::build_advanced_form(),
                false,
                8, // elastic, pipeline, batch, checkpoint, autoscale, verbose, quiet, dry_run
            ),
        ]
    }

    // ------------------------------------------------------------------
    // Form builders for each section
    // ------------------------------------------------------------------

    fn build_target_form() -> Form {
        Form::builder()
            .text("url", "URL")
            .placeholder("https://example.com")
            .done()
            .text("selector", "CSS Selector")
            .initial_value("body")
            .done()
            .build()
    }

    fn build_output_form() -> Form {
        Form::builder()
            .text("output", "Output Directory")
            .initial_value("output")
            .done()
            .select("format", "Output Format")
            .option("markdown", "Markdown")
            .option("json", "JSON")
            .option("text", "Plain Text")
            .initial_value("markdown")
            .done()
            .select("export_format", "Export Format")
            .option("jsonl", "JSONL")
            .option("vector", "Vector")
            .option("auto", "Auto")
            .initial_value("jsonl")
            .done()
            .build()
    }

    fn build_discovery_form() -> Form {
        Form::builder()
            .checkbox("use_sitemap", "Use Sitemap")
            .checked(false)
            .done()
            .text("sitemap_url", "Sitemap URL")
            .placeholder("https://example.com/sitemap.xml")
            .done()
            .text("max_pages", "Max Pages")
            .initial_value("10")
            .done()
            .text("max_depth", "Max Depth")
            .initial_value("2")
            .done()
            .text("sitemap_depth", "Sitemap Recursion Depth")
            .initial_value("3")
            .done()
            .build()
    }

    fn build_crawler_form() -> Form {
        Form::builder()
            .text("timeout_secs", "Request Timeout (secs)")
            .initial_value("30")
            .done()
            .text("max_retries", "Max Retries")
            .initial_value("3")
            .done()
            .text("delay_ms", "Delay Between Requests (ms)")
            .initial_value("1000")
            .done()
            .text("concurrency", "Concurrency")
            .initial_value("auto")
            .done()
            .text("include_pattern", "Include Pattern (glob)")
            .placeholder("*/products/*")
            .done()
            .text("exclude_pattern", "Exclude Pattern (glob)")
            .placeholder("*/admin/*")
            .done()
            .build()
    }

    fn build_network_form() -> Form {
        Form::builder()
            .text("user_agent", "User-Agent")
            .placeholder("Chrome145 (default)")
            .done()
            .text("accept_language", "Accept-Language")
            .initial_value("en-US,en;q=0.9")
            .done()
            .text("h2_profile", "TLS Profile")
            .initial_value("Chrome145")
            .done()
            .select("js_strategy", "JS Strategy")
            .option("static", "Static (fastest)")
            .option("hybrid", "Hybrid (3-layer)")
            .option("full", "Full (Chromiumoxide)")
            .initial_value("static")
            .done()
            .checkbox("force_js_render", "Force JS Rendering")
            .checked(false)
            .done()
            .build()
    }

    fn build_download_form() -> Form {
        Form::builder()
            .checkbox("download_images", "Download Images")
            .checked(false)
            .done()
            .checkbox("download_documents", "Download Documents")
            .checked(false)
            .done()
            .text("max_file_size", "Max File Size (bytes)")
            .initial_value("52428800")
            .done()
            .text("download_timeout", "Download Timeout (secs)")
            .initial_value("30")
            .done()
            .build()
    }

    fn build_obsidian_form() -> Form {
        Form::builder()
            .checkbox("obsidian_wiki_links", "Wiki Links")
            .checked(false)
            .done()
            .text("obsidian_tags", "Tags (comma-separated)")
            .placeholder("scraping,ai")
            .done()
            .checkbox("obsidian_relative_assets", "Relative Assets")
            .checked(false)
            .done()
            .checkbox("obsidian_rich_metadata", "Rich Metadata")
            .checked(false)
            .done()
            .text("vault", "Vault Path")
            .placeholder("~/Documents/MyVault")
            .done()
            .checkbox("quick_save", "Quick Save to _inbox")
            .checked(false)
            .done()
            .build()
    }

    fn build_advanced_form() -> Form {
        Form::builder()
            .checkbox("elastic", "Elastic Ingestion")
            .checked(false)
            .done()
            .checkbox("pipeline", "Enable Pipeline")
            .checked(false)
            .done()
            .checkbox("batch", "Batch Mode")
            .checked(false)
            .done()
            .text("checkpoint_interval", "Checkpoint Interval")
            .initial_value("100")
            .done()
            .checkbox("autoscale", "Autoscale Concurrency")
            .checked(false)
            .done()
            .text("verbose", "Verbosity (0-3)")
            .initial_value("0")
            .done()
            .checkbox("quiet", "Quiet Mode")
            .checked(false)
            .done()
            .checkbox("dry_run", "Dry Run")
            .checked(false)
            .done()
            .build()
    }

    // ------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------

    /// Move cursor up in section list.
    fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Move cursor down in section list.
    fn move_down(&mut self) {
        self.cursor = (self.cursor + 1).min(self.sections.len() - 1);
    }

    /// Expand the current section.
    fn expand_current(&mut self) {
        self.sections[self.cursor].expanded = true;
    }

    /// Collapse the current section.
    fn collapse_current(&mut self) {
        self.sections[self.cursor].expanded = false;
    }

    /// Toggle expand/collapse of current section.
    fn toggle_current(&mut self) {
        self.sections[self.cursor].expanded ^= true;
    }

    /// Get the currently focused form (if in field edit mode).
    fn focused_form(&mut self) -> Option<&mut Form> {
        if self.mode == ConfigMode::FieldEdit {
            Some(&mut self.sections[self.cursor].form)
        } else {
            None
        }
    }

    // ------------------------------------------------------------------
    // Form data export
    // ------------------------------------------------------------------

    /// Merge all section forms into a single JSON value.
    pub fn to_json(&self) -> Value {
        let mut merged = serde_json::Map::new();
        for section in &self.sections {
            let section_data = section.form.to_json();
            if let Value::Object(map) = section_data {
                for (k, v) in map {
                    merged.insert(k, v);
                }
            }
        }
        Value::Object(merged)
    }

    /// Get the URL field value (from Target section).
    pub fn url(&self) -> Option<String> {
        self.sections[0]
            .form
            .to_json()
            .get("url")
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    /// Get the index of the currently focused section.
    pub fn focused_section_index(&self) -> usize {
        self.cursor
    }

    /// Check if a section is expanded.
    pub fn is_section_expanded(&self, index: usize) -> bool {
        self.sections.get(index).is_some_and(|s| s.expanded)
    }
}

// ============================================================================
// Component implementation
// ============================================================================

impl Component for CollapsibleConfig {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        // Global shortcuts
        if matches!(key.code, KeyCode::Char('?')) {
            return Ok(Some(Action::ToggleHelp));
        }

        if matches!(key.code, KeyCode::Char('q' | 'Q')) {
            self.cancelled = true;
            return Ok(Some(Action::ConfigCancelled));
        }

        // Ctrl+S to submit
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('s')) {
            self.submitted = true;
            return Ok(Some(Action::ConfigDone(Some(self.to_json()))));
        }

        match self.mode {
            ConfigMode::SectionList => self.handle_section_keys(key),
            ConfigMode::FieldEdit => self.handle_field_keys(key),
        }
    }

    fn update(&mut self, _action: Action) -> Result<Option<Action>> {
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> Result<()> {
        // Layout: header + sections + footer
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(1),    // Sections
                Constraint::Length(2), // Footer
            ])
            .split(area);

        // Render header
        self.render_header(frame, chunks[0]);

        // Render sections
        self.render_sections(frame, chunks[1]);

        // Render footer
        self.render_footer(frame, chunks[2]);

        Ok(())
    }
}

// ============================================================================
// Rendering
// ============================================================================

impl CollapsibleConfig {
    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let title = match self.mode {
            ConfigMode::SectionList => "Configuration — Select a section",
            ConfigMode::FieldEdit => {
                let section = &self.sections[self.cursor];
                &format!(
                    "Configuration — {} ({} fields)",
                    section.title, section.field_count
                )
            },
        };

        let header = Paragraph::new(Line::from(vec![
            Span::styled(" ⚙️ ", Theme::accent()),
            Span::styled(title, Theme::text()),
        ]))
        .block(
            Block::bordered()
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::new().fg(Theme::surface())),
        );

        frame.render_widget(header, area);
    }

    fn render_sections(&mut self, frame: &mut Frame, area: Rect) {
        match self.mode {
            ConfigMode::SectionList => self.render_section_list(frame, area),
            ConfigMode::FieldEdit => self.render_field_editor(frame, area),
        }
    }

    fn render_section_list(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .sections
            .iter()
            .enumerate()
            .map(|(i, section)| {
                let icon = if section.expanded { "▼" } else { "▶" };
                let is_focused = i == self.cursor;

                let style = if is_focused {
                    Style::default()
                        .fg(Theme::accent())
                        .add_modifier(Modifier::BOLD)
                } else if section.expanded {
                    Style::default().fg(Theme::text())
                } else {
                    Style::default().fg(Theme::text_subtle())
                };

                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {icon} "), style),
                    Span::styled(&section.title, style),
                    Span::styled(
                        format!("  ({} fields)", section.field_count),
                        Style::default().fg(Theme::text_muted()),
                    ),
                ]))
            })
            .collect();

        let list = List::new(items).block(
            Block::bordered()
                .title("Sections")
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::new().fg(Theme::surface())),
        );

        frame.render_widget(list, area);
    }

    fn render_field_editor(&mut self, frame: &mut Frame, area: Rect) {
        let section = &mut self.sections[self.cursor];

        // Render section title
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" ▼ {} ", section.title),
                Style::default()
                    .fg(Theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("← back", Style::default().fg(Theme::text_muted())),
        ]));

        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);

        frame.render_widget(title, chunks[0]);

        // Render form fields
        section.form.render(chunks[1], frame.buffer_mut());
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let hints = match self.mode {
            ConfigMode::SectionList => {
                "↑↓ Navigate │ Enter/→ Expand │ ← Collapse │ Space Toggle │ Ctrl+S Submit │ q Quit"
            },
            ConfigMode::FieldEdit => {
                "Tab/Shift+Tab Fields │ Enter Submit │ Esc Back │ Ctrl+S Submit All │ q Quit"
            },
        };

        let footer = Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(Theme::text_muted()),
        )))
        .block(
            Block::bordered()
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::new().fg(Theme::surface())),
        );

        frame.render_widget(footer, area);
    }
}

// ============================================================================
// Key handling
// ============================================================================

impl CollapsibleConfig {
    fn handle_section_keys(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            KeyCode::Up => {
                self.move_up();
                Ok(None)
            },
            KeyCode::Down => {
                self.move_down();
                Ok(None)
            },
            KeyCode::Enter | KeyCode::Right => {
                self.expand_current();
                self.mode = ConfigMode::FieldEdit;
                Ok(None)
            },
            KeyCode::Left => {
                self.collapse_current();
                Ok(None)
            },
            KeyCode::Char(' ') => {
                self.toggle_current();
                if self.sections[self.cursor].expanded {
                    self.mode = ConfigMode::FieldEdit;
                }
                Ok(None)
            },
            _ => Ok(None),
        }
    }

    fn handle_field_keys(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            KeyCode::Esc | KeyCode::Left => {
                self.mode = ConfigMode::SectionList;
                Ok(None)
            },
            _ => {
                // Delegate to form
                if let Some(form) = self.focused_form() {
                    form.handle_input(key);
                    match form.result() {
                        FormResult::Submitted => {
                            // Section form submitted, go back to section list
                            self.mode = ConfigMode::SectionList;
                            Ok(None)
                        },
                        FormResult::Cancelled => {
                            self.mode = ConfigMode::SectionList;
                            Ok(None)
                        },
                        FormResult::Active => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            },
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsible_config_creates_with_all_sections() {
        let config = CollapsibleConfig::new();
        assert_eq!(config.sections.len(), 8);
        assert_eq!(config.cursor, 0);
        assert_eq!(config.mode, ConfigMode::SectionList);
    }

    #[test]
    fn sections_have_correct_field_counts() {
        let config = CollapsibleConfig::new();
        let expected = [2, 3, 5, 6, 5, 4, 6, 8]; // Total: 39 fields
        for (i, &expected_count) in expected.iter().enumerate() {
            assert_eq!(
                config.sections[i].field_count, expected_count,
                "Section {} has wrong field count",
                config.sections[i].title
            );
        }
    }

    #[test]
    fn first_and_third_sections_are_expanded_by_default() {
        let config = CollapsibleConfig::new();
        assert!(config.sections[0].expanded, "Target should be expanded");
        assert!(!config.sections[1].expanded, "Output should be collapsed");
        assert!(config.sections[2].expanded, "Discovery should be expanded");
    }

    #[test]
    fn move_up_stays_at_zero() {
        let mut config = CollapsibleConfig::new();
        config.move_up();
        config.move_up();
        assert_eq!(config.cursor, 0);
    }

    #[test]
    fn move_down_stays_at_max() {
        let mut config = CollapsibleConfig::new();
        for _ in 0..20 {
            config.move_down();
        }
        assert_eq!(config.cursor, 7); // 8 sections, 0-indexed
    }

    #[test]
    fn toggle_expand_collapse() {
        let mut config = CollapsibleConfig::new();
        // Output section (index 1) starts collapsed
        assert!(!config.sections[1].expanded);
        config.cursor = 1; // Move to Output
        config.toggle_current();
        assert!(config.sections[1].expanded);
        config.toggle_current();
        assert!(!config.sections[1].expanded);
    }

    #[test]
    fn to_json_merges_all_sections() {
        let config = CollapsibleConfig::new();
        let json = config.to_json();
        assert!(json.is_object());
        // Should have fields from all sections
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("url"));
        assert!(obj.contains_key("output"));
        assert!(obj.contains_key("max_pages"));
    }

    #[test]
    fn section_header_colors() {
        assert_eq!(Theme::section_header(true), Theme::accent());
        assert_eq!(Theme::section_header(false), Theme::text_subtle());
    }

    #[test]
    fn section_content_colors() {
        assert_eq!(Theme::section_content(true), Theme::text());
        assert_eq!(Theme::section_content(false), Theme::text_muted());
    }
}

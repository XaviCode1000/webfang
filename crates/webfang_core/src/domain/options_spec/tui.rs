//! TUI flag group (ADR-002 slice 5a): mirrors `cli::args::TuiArgs` field-by-field.
//! Only one option today (`--tui`); kept as its own group so future TUI
//! surface additions slot into the OptionsSpec SSOT instead of going back to
//! derive.
use super::{OptionSpec, ValueKind};

/// `--tui`
pub const TUI: OptionSpec = OptionSpec {
    id: "tui",
    value_name: "TUI",
    long: "tui",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_TUI"),
    // No explicit `default_value` attr: clap's implicit SetTrue default is
    // not introspectable via `get_default_values()` (same as the crawler's
    // `use_sitemap` / `resume`).
    default: None,
    help: "Unified TUI mode: config form (collapsible sections) → URL selector → scraping",
    heading: Some("Behavior"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// All TUI-group options, in `TuiArgs` field-declaration order.
pub const GROUP: &[OptionSpec] = &[TUI];

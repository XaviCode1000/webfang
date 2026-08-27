//! Obsidian flag group (ADR-002 slice 5a): mirrors `cli::args::ObsidianArgs`
//! field-by-field. The `obsidian_tags` entry is the first OptionsSpec entry
//! with a `value_delimiter` (`','`); all others stay single-value.
use super::{DefaultValue, OptionSpec, ValueKind};

/// `--obsidian-wiki-links`
pub const OBSIDIAN_WIKI_LINKS: OptionSpec = OptionSpec {
    id: "obsidian_wiki_links",
    value_name: "OBSIDIAN_WIKI_LINKS",
    long: "obsidian-wiki-links",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_OBSIDIAN_WIKI_LINKS"),
    default: Some(DefaultValue::Bool(false)),
    help: "Convert same-domain links to Obsidian [[wiki-link]] syntax",
    heading: Some("Obsidian"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--obsidian-tags <OBSIDIAN_TAGS>` — comma-delimited list (`','`), single
/// spec entry that needs a value delimiter. Mirrors clap's `value_delimiter
/// = ','` rendering exactly.
pub const OBSIDIAN_TAGS: OptionSpec = OptionSpec {
    id: "obsidian_tags",
    value_name: "OBSIDIAN_TAGS",
    long: "obsidian-tags",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_OBSIDIAN_TAGS"),
    default: None,
    help: "Tags to include in YAML frontmatter (comma-separated)",
    heading: Some("Obsidian"),
    kind: ValueKind::TextList,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: Some(','),
};

/// `--obsidian-relative-assets`
pub const OBSIDIAN_RELATIVE_ASSETS: OptionSpec = OptionSpec {
    id: "obsidian_relative_assets",
    value_name: "OBSIDIAN_RELATIVE_ASSETS",
    long: "obsidian-relative-assets",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_OBSIDIAN_RELATIVE_ASSETS"),
    default: Some(DefaultValue::Bool(false)),
    help: "Rewrite downloaded asset paths as relative to the .md file",
    heading: Some("Obsidian"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--vault <VAULT>` — env `WEBFANG_OBSIDIAN_VAULT`. Byte-exact
/// transcription of clap's rendering of the multi-line doc comment: outer
/// paragraphs are joined with single spaces and only the final period is
/// stripped. The `#762` note travels with the help text so the MCP wire
/// description stays anchored to the same source of truth.
pub const VAULT: OptionSpec = OptionSpec {
    id: "vault",
    value_name: "VAULT",
    long: "vault",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_OBSIDIAN_VAULT"),
    default: None,
    help: "Path to Obsidian vault (auto-detects if not provided). When provided explicitly, the vault becomes the output base: Markdown, downloaded assets and the RAG export are written inside it — no need to duplicate the path in `-o` (which then must stay at its default). Auto-detected or config-file vaults do NOT redirect output (#762)",
    heading: Some("Obsidian"),
    kind: ValueKind::Path,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--quick-save`
pub const QUICK_SAVE: OptionSpec = OptionSpec {
    id: "quick_save",
    value_name: "QUICK_SAVE",
    long: "quick-save",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_OBSIDIAN_QUICK_SAVE"),
    default: Some(DefaultValue::Bool(false)),
    help: "Quick-save mode: save directly to vault _inbox folder",
    heading: Some("Obsidian"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--obsidian-rich-metadata`
pub const OBSIDIAN_RICH_METADATA: OptionSpec = OptionSpec {
    id: "obsidian_rich_metadata",
    value_name: "OBSIDIAN_RICH_METADATA",
    long: "obsidian-rich-metadata",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_OBSIDIAN_RICH_METADATA"),
    default: Some(DefaultValue::Bool(false)),
    help: "Add rich metadata to frontmatter",
    heading: Some("Obsidian"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// All Obsidian-group options, in `ObsidianArgs` field-declaration order.
pub const GROUP: &[OptionSpec] = &[
    OBSIDIAN_WIKI_LINKS,
    OBSIDIAN_TAGS,
    OBSIDIAN_RELATIVE_ASSETS,
    VAULT,
    QUICK_SAVE,
    OBSIDIAN_RICH_METADATA,
];

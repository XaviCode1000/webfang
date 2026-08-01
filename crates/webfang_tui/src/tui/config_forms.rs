//! Config form schema builders.
//!
//! Pure builders that define the TUI configuration form schema — field names,
//! defaults, and help text — for each collapsible section. Extracted from the
//! [`CollapsibleConfig`](super::collapsible_config::CollapsibleConfig) view so
//! the schema lives apart from rendering and navigation logic.

use ratatui_form::Form;

/// Build the "Target" section form (url, selector).
pub(super) fn build_target_form() -> Form {
    Form::builder()
        .text("url", "URL")
        .placeholder("https://example.com")
        .done()
        .text("selector", "CSS Selector")
        .initial_value("body")
        .done()
        .build()
}

/// Build the "Output" section form (output, format, export_format).
pub(super) fn build_output_form() -> Form {
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

/// Build the "Discovery" section form (use_sitemap, sitemap_url, max_pages,
/// max_depth, sitemap_depth).
pub(super) fn build_discovery_form() -> Form {
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

/// Build the "Crawler" section form (timeout, retries, delay, concurrency,
/// include, exclude).
pub(super) fn build_crawler_form() -> Form {
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

/// Build the "Network" section form (user_agent, accept_language, h2_profile,
/// js_strategy, force_js).
pub(super) fn build_network_form() -> Form {
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
        .build()
}

/// Build the "Download" section form (images, documents, max_file_size,
/// download_timeout).
pub(super) fn build_download_form() -> Form {
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

/// Build the "Obsidian" section form (wiki_links, tags, relative_assets,
/// rich_metadata, vault, quick_save).
pub(super) fn build_obsidian_form() -> Form {
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

/// Build the "Advanced" section form (elastic, pipeline, batch, checkpoint,
/// autoscale, verbose, quiet, dry_run).
pub(super) fn build_advanced_form() -> Form {
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

//! Export flow logic extracted from orchestrator.

use std::path::PathBuf;
use tracing::{info, warn};

use crate::{Args, ObsidianOptions};
use crate::domain::ScrapedContent;
use crate::infrastructure::export::state_store::StateStore;

use crate::CliExit;
use crate::cli::preflight;
use crate::export_flow::{self, ExportConfig};

/// Run the export flow (AI or standard).
pub async fn run_export_flow(
    results: &[ScrapedContent],
    args: &Args,
    vault_path: &Option<PathBuf>,
    state_store: Option<&StateStore>,
) -> Result<Vec<String>, CliExit> {
    let ok = preflight::icon("✅", "OK");
    info!("Exporting results (format: {:?})...", args.export_format);

    let output_dir = determine_output_dir(args, vault_path);
    let obsidian_options = build_obsidian_options(args, vault_path);

    let export_config = build_export_config(
        results,
        args,
        output_dir,
        vault_path,
        obsidian_options,
        state_store,
    );

    let processed_urls: Vec<String> = match export_flow::run_export(export_config).await {
        Ok(urls) => urls,
        Err(exit) => return Err(exit),
    };

    info!(
        "{} Export completed: {} URLs processed",
        ok,
        processed_urls.len()
    );

    Ok(processed_urls)
}

/// Build ExportConfig from args, handling feature-gated AI fields.
pub fn build_export_config<'a>(
    results: &'a [ScrapedContent],
    args: &'a Args,
    output_dir: PathBuf,
    vault_path: &'a Option<PathBuf>,
    obsidian_options: ObsidianOptions,
    state_store: Option<&'a StateStore>,
) -> ExportConfig<'a> {
    ExportConfig {
        results,
        output_dir,
        format: args.format,
        export_format: args.export_format,
        clean_ai: args.clean_ai,
        quick_save: args.quick_save,
        vault_path: vault_path.as_ref(),
        obsidian_options,
        state_store,
        resume: args.resume,
        #[cfg(feature = "ai")]
        ai_threshold: args.threshold,
        #[cfg(feature = "ai")]
        ai_max_tokens: args.max_tokens,
        #[cfg(feature = "ai")]
        ai_offline: args.offline,
        #[cfg(not(feature = "ai"))]
        ai_threshold: 0.3,
        #[cfg(not(feature = "ai"))]
        ai_max_tokens: 512,
        #[cfg(not(feature = "ai"))]
        ai_offline: false,
    }
}

/// Determine output directory (vault _inbox for quick-save mode).
pub fn determine_output_dir(args: &Args, vault_path: &Option<PathBuf>) -> PathBuf {
    if args.quick_save {
        if let Some(ref vault) = vault_path {
            let inbox_path = vault.join("_inbox");
            if let Err(e) = std::fs::create_dir_all(&inbox_path) {
                warn!("Failed to create vault _inbox directory: {}", e);
                args.output.clone()
            } else {
                info!("Quick-save: using vault inbox {}", inbox_path.display());
                inbox_path
            }
        } else {
            warn!("Quick-save mode but no vault detected, using output directory");
            args.output.clone()
        }
    } else {
        args.output.clone()
    }
}

/// Build ObsidianOptions from CLI args.
pub fn build_obsidian_options(args: &Args, vault_path: &Option<PathBuf>) -> ObsidianOptions {
    ObsidianOptions {
        wiki_links: args.obsidian_wiki_links,
        tags: args.obsidian_tags.clone().unwrap_or_default(),
        relative_assets: args.obsidian_relative_assets,
        rich_metadata: args.obsidian_rich_metadata,
        quick_save: args.quick_save,
        vault_path: vault_path.clone(),
    }
}

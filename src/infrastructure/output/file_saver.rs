//! File saving with domain-based folder structure
//!
//! Saves scraped content to files with:
//! - Domain-based folder organization
//! - URL-based file naming
//! - YAML frontmatter with metadata
//! - Obsidian-compatible output (wiki-links, relative assets, tags)
//! - Rich metadata (word count, reading time, language)

use crate::domain::ScrapedContent;
use crate::error::Result;
use crate::infrastructure::converter::{html_to_markdown, obsidian, syntax_highlight};
use crate::infrastructure::obsidian::ObsidianRichMetadata;
use crate::infrastructure::output::frontmatter;
use crate::url_path::OutputPath;
use crate::OutputFormat;
use std::path::Path;
use tracing::warn;

/// Configuration for Obsidian-compatible output.
#[derive(Debug, Clone, Default)]
pub struct ObsidianOptions {
    /// Convert same-domain links to [[wiki-link]] syntax
    pub wiki_links: bool,
    /// Rewrite asset paths as relative to the .md file
    pub relative_assets: bool,
    /// Tags to include in YAML frontmatter
    pub tags: Vec<String>,
    /// Enable rich metadata (word count, reading time, language)
    pub rich_metadata: bool,
    /// Quick-save mode: save to vault _inbox folder
    pub quick_save: bool,
    /// Vault path for Obsidian integration
    pub vault_path: Option<std::path::PathBuf>,
}

/// Save scraped results to output directory
///
/// # Arguments
/// * `results` - Scraped content to save
/// * `output_dir` - Base output directory
/// * `format` - Output format (Markdown, Text, JSON)
/// * `obsidian` - Obsidian options (tags, wiki-links, relative assets)
///
/// # Returns
/// * `Ok(())` - Successfully saved
/// * `Err(ScraperError::Io)` - File system error
pub fn save_results(
    results: &[ScrapedContent],
    output_dir: &Path,
    format: &OutputFormat,
    obsidian: &ObsidianOptions,
) -> Result<()> {
    use std::fs;

    fs::create_dir_all(output_dir)?;

    match format {
        OutputFormat::Markdown => save_as_markdown(results, output_dir, obsidian),
        OutputFormat::Text => save_as_text(results, output_dir),
        OutputFormat::Json => save_as_json(results, output_dir),
    }
}

/// Save results as Markdown files with YAML frontmatter
fn save_as_markdown(
    results: &[ScrapedContent],
    output_dir: &Path,
    obsidian: &ObsidianOptions,
) -> Result<()> {
    use std::fs;

    for item in results {
        let output_path = match OutputPath::from_url(item.url.as_str()) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse URL {}: {}, using fallback", item.url, e);
                let fallback_path = output_dir.join("index.md");
                fs::create_dir_all(output_dir)?;
                let content = format!("# {}\n\n{}", item.title, item.content);
                fs::write(&fallback_path, content)?;
                continue;
            },
        };

        let full_path_str = output_path.to_full_path();
        let relative_path = full_path_str.trim_start_matches("./output/");
        let full_path = output_dir.join(relative_path);
        let md_file_dir = full_path.parent().unwrap_or(output_dir);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let markdown_content = item
            .html
            .as_ref()
            .map(|html| html_to_markdown::convert_to_markdown(html))
            .unwrap_or_else(|| item.content.clone());

        let mut processed = syntax_highlight::highlight_code_blocks(&markdown_content);

        // Apply wiki-link conversion if enabled
        if obsidian.wiki_links {
            let base_domain = item.url.host_str().unwrap_or("");
            processed = obsidian::convert_wiki_links(&processed, base_domain);
        }

        // Apply relative asset paths if enabled
        if obsidian.relative_assets && !item.assets.is_empty() {
            processed = obsidian::resolve_asset_paths(&processed, md_file_dir, &item.assets);
        }

        // Generate rich metadata if enabled
        let rich_meta = if obsidian.rich_metadata {
            Some(ObsidianRichMetadata::from_content(item))
        } else {
            None
        };

        let fm = frontmatter::generate_with_metadata(
            &item.title,
            item.url.as_str(),
            item.date.as_deref(),
            item.author.as_deref(),
            item.excerpt.as_deref(),
            &obsidian.tags,
            rich_meta.as_ref(),
        );

        let final_content = format!("---\n{}\n---\n\n{}", fm.trim_end(), processed);
        fs::write(&full_path, final_content)?;
        tracing::info!("💾 Saved: {}", full_path.display());
    }

    Ok(())
}

/// Save results as plain text files
fn save_as_text(results: &[ScrapedContent], output_dir: &Path) -> Result<()> {
    use std::fs;

    for item in results {
        let output_path = match OutputPath::from_url(item.url.as_str()) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse URL {}: {}, using fallback", item.url, e);
                let fallback_path = output_dir.join("index.txt");
                fs::write(&fallback_path, &item.content)?;
                continue;
            },
        };

        let full_path = output_dir.join(
            output_path
                .to_full_path()
                .trim_start_matches("./")
                .replace(".md", ".txt"),
        );
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&full_path, &item.content)?;
        tracing::info!("💾 Saved: {}", full_path.display());
    }

    Ok(())
}

/// Save results as JSON
fn save_as_json(results: &[ScrapedContent], output_dir: &Path) -> Result<()> {
    use std::fs;

    let json_path = output_dir.join("results.json");
    let json = serde_json::to_string_pretty(results)?;
    fs::write(&json_path, json)?;
    tracing::info!("💾 Saved: {}", json_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ValidUrl;
    use tempfile::TempDir;

    #[test]
    fn test_save_as_markdown_single_item() {
        let temp_dir = TempDir::new().unwrap();
        let output_dir = temp_dir.path();

        let results = vec![ScrapedContent {
            title: "Test Article".to_string(),
            content: "Test content".to_string(),
            url: ValidUrl::parse("https://example.com/article").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
        }];

        let result = save_as_markdown(&results, output_dir, &ObsidianOptions::default());
        assert!(result.is_ok());

        // Verify file was created
        use walkdir::WalkDir;
        let files: Vec<_> = WalkDir::new(output_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .collect();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_save_as_json() {
        let temp_dir = TempDir::new().unwrap();
        let output_dir = temp_dir.path();

        let results = vec![ScrapedContent {
            title: "Test".to_string(),
            content: "Content".to_string(),
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
        }];

        let result = save_as_json(&results, output_dir);
        assert!(result.is_ok());

        let json_path = output_dir.join("results.json");
        assert!(json_path.exists());
    }
}

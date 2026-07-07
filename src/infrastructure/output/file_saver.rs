//! File saving with domain-based folder structure
//!
//! Saves scraped content to files with:
//! - Domain-based folder organization
//! - URL-based file naming
//! - YAML frontmatter with metadata
//! - Obsidian-compatible output (wiki-links, relative assets, tags)
//! - Rich metadata (word count, reading time, language)

use crate::adapters::url_path::OutputPath;
use crate::domain::ScrapedContent;
use crate::error::Result;
use crate::infrastructure::converter::{html_to_markdown, obsidian, syntax_highlight};
use crate::infrastructure::obsidian::ObsidianRichMetadata;
use crate::infrastructure::output::frontmatter;
use crate::OutputFormat;
use std::path::Path;
use tracing::warn;

/// Convert HTML to Markdown using the best available converter.
///
/// Pipeline strategy:
/// 1. Try `htmd` first (turndown-like, handles modern HTML well)
/// 2. Fall back to `html_to_markdown` if htmd produces empty output or errors
/// 3. If both fail, return the input as-is (shouldn't happen with valid HTML)
fn convert_html_to_markdown(html: &str) -> String {
    // Try htmd (turndown-inspired converter)
    if let Ok(md) = htmd::convert(html) {
        if !md.trim().is_empty() {
            return md;
        }
    }

    // Fallback to html_to_markdown
    let fallback = html_to_markdown::convert_to_markdown(html);
    if !fallback.trim().is_empty() {
        return fallback;
    }

    // Last resort: return the HTML as-is
    warn!("Both Markdown converters produced empty output, returning raw HTML");
    html.to_string()
}

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
        OutputFormat::Text => save_as_text(results, output_dir, obsidian),
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
        let output_path: OutputPath = match OutputPath::from_url(item.url.as_str()) {
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

        // Convert HTML to Markdown.
        // The HTML stored in item.html should be clean HTML (already processed
        // by Readability/legible to remove nav, sidebar, footer, ads).
        // We use htmd (turndown-like) as primary converter, with html_to_markdown
        // as fallback.
        let markdown_content = if let Some(html) = &item.html {
            convert_html_to_markdown(html)
        } else if !item.content.is_empty() {
            // Fallback: use readability text directly (no structure)
            item.content.clone()
        } else {
            String::new()
        };

        let mut processed = syntax_highlight::highlight_code_blocks(&markdown_content);

        // Apply wiki-link conversion if enabled
        if obsidian.wiki_links {
            let base_domain = item.url.host_str().unwrap_or("");
            processed = obsidian::convert_wiki_links(&processed, base_domain);
        }

        // Apply relative asset paths if enabled
        if obsidian.relative_assets {
            if !item.assets.is_empty() {
                processed = obsidian::resolve_asset_paths(&processed, md_file_dir, &item.assets);
            } else {
                processed = rewrite_image_urls_to_relative(&processed, md_file_dir);
            }
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

/// Rewrite absolute image URLs in markdown to relative paths.
///
/// Scans for `![alt](https://...)` patterns and rewrites to `![alt](./relative/path)`.
/// Used as a fallback when `--download-images` is not enabled and `item.assets` is empty.
fn rewrite_image_urls_to_relative(content: &str, _md_file_dir: &Path) -> String {
    use regex::Regex;

    // Match ![alt text](url) where url is absolute
    static RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\((https?://[^)]+)\)").unwrap());

    RE.replace_all(content, |caps: &regex::Captures| {
        let alt = &caps[1];
        let url = &caps[2];

        // Parse the URL to extract the path
        if let Ok(parsed) = url::Url::parse(url) {
            let path = parsed.path();
            // Create a relative path from md_file_dir to the image
            // For simplicity, use the last 2 segments as the relative path
            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            let rel_path = if segments.len() >= 2 {
                format!("./{}/{}", segments[segments.len() - 2], segments[segments.len() - 1])
            } else if segments.len() == 1 {
                format!("./{}", segments[0])
            } else {
                url.to_string()
            };
            format!("![{alt}]({rel_path})")
        } else {
            caps[0].to_string()
        }
    })
    .to_string()
}

/// Save results as plain text files with structured format
fn save_as_text(
    results: &[ScrapedContent],
    output_dir: &Path,
    _obsidian: &ObsidianOptions,
) -> Result<()> {
    use std::fs;

    for item in results {
        let output_path: OutputPath = match OutputPath::from_url(item.url.as_str()) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse URL {}: {}, using fallback", item.url, e);
                let fallback_path = output_dir.join("index.txt");
                // Structured text format with delimiters
                let author = item.author.as_deref().unwrap_or("Unknown");
                let date = item.date.as_deref().unwrap_or("N/A");
                let content = format!(
                    "========================================\n\
                     TITLE: {}\n\
                     URL: {}\n\
                     AUTHOR: {}\n\
                     DATE: {}\n\
                     ----------------------------------------\n\
                     CONTENT:\n\
                     {}\n\
                     ========================================",
                    item.title, item.url, author, date, item.content
                );
                fs::write(&fallback_path, &content)?;
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

        // Structured text format with delimiters
        let author = item.author.as_deref().unwrap_or("Unknown");
        let date = item.date.as_deref().unwrap_or("N/A");
        let content = format!(
            "========================================\n\
             TITLE: {}\n\
             URL: {}\n\
             AUTHOR: {}\n\
             DATE: {}\n\
             ----------------------------------------\n\
             CONTENT:\n\
             {}\n\
             ========================================",
            item.title, item.url, author, date, item.content
        );
        fs::write(&full_path, &content)?;
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
            correlation_id: None,
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
            correlation_id: None,
        }];

        let result = save_as_json(&results, output_dir);
        assert!(result.is_ok());

        let json_path = output_dir.join("results.json");
        assert!(json_path.exists());
    }

    // === rewrite_image_urls_to_relative tests ===

    #[test]
    fn test_rewrite_single_image_url_to_relative() {
        let content = "![alt](https://example.com/img/photo.png)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "![alt](./img/photo.png)");
    }

    #[test]
    fn test_rewrite_deeply_nested_image_url() {
        let content = "![alt](https://example.com/a/b/c.png)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "![alt](./b/c.png)");
    }

    #[test]
    fn test_rewrite_non_url_images_unchanged() {
        let content = "![alt](./local/image.png)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "![alt](./local/image.png)");
    }

    #[test]
    fn test_rewrite_empty_content_returns_empty() {
        let content = "";
        let dir = Path::new("/output");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "");
    }

    #[test]
    fn test_rewrite_multiple_images() {
        let content = "First ![a](https://example.com/x.png) then ![b](https://example.com/y.jpg)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(
            result,
            "First ![a](./x.png) then ![b](./y.jpg)"
        );
    }

    #[test]
    fn test_rewrite_single_segment_path() {
        let content = "![alt](https://example.com/photo.png)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "![alt](./photo.png)");
    }
}

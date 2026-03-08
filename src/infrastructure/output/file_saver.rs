//! File saving with domain-based folder structure
//!
//! Saves scraped content to files with:
//! - Domain-based folder organization
//! - URL-based file naming
//! - YAML frontmatter with metadata

use crate::domain::ScrapedContent;
use crate::error::Result;
use crate::infrastructure::converter::{html_to_markdown, syntax_highlight};
use crate::infrastructure::output::frontmatter;
use crate::url_path::OutputPath;
use crate::OutputFormat;
use std::path::Path;
use tracing::warn;

/// Save scraped results to output directory
///
/// # Arguments
/// * `results` - Scraped content to save
/// * `output_dir` - Base output directory
/// * `format` - Output format (Markdown, Text, JSON)
///
/// # Returns
/// * `Ok(())` - Successfully saved
/// * `Err(ScraperError::Io)` - File system error
pub fn save_results(
    results: &[ScrapedContent],
    output_dir: &Path,
    format: &OutputFormat,
) -> Result<()> {
    use std::fs;

    fs::create_dir_all(output_dir)?;

    match format {
        OutputFormat::Markdown => save_as_markdown(results, output_dir),
        OutputFormat::Text => save_as_text(results, output_dir),
        OutputFormat::Json => save_as_json(results, output_dir),
    }
}

/// Save results as Markdown files with YAML frontmatter
fn save_as_markdown(results: &[ScrapedContent], output_dir: &Path) -> Result<()> {
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
            }
        };

        let full_path_str = output_path.to_full_path();
        let relative_path = full_path_str.trim_start_matches("./output/");
        let full_path = output_dir.join(relative_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let markdown_content = item
            .html
            .as_ref()
            .map(|html| html_to_markdown::convert_to_markdown(html))
            .unwrap_or_else(|| item.content.clone());

        let highlighted = syntax_highlight::highlight_code_blocks(&markdown_content);

        let fm = frontmatter::generate(
            &item.title,
            item.url.as_str(),
            item.date.as_deref(),
            item.author.as_deref(),
            item.excerpt.as_deref(),
        );

        let final_content = format!("---\n{}---\n\n{}", fm.trim(), highlighted);
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
            }
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

        let result = save_as_markdown(&results, output_dir);
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

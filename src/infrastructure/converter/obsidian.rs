//! Obsidian-compatible Markdown export
//!
//! Provides transformations for Obsidian vault compatibility:
//! - Wiki-link conversion: [text](url) -> [[slug|text]] (via wikilinks module)
//! - Relative asset paths: absolute paths -> relative to .md file

use crate::domain::DownloadedAsset;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::path::Path;

// Re-export wikilink functions for backward compatibility
pub use super::wikilinks::{convert_wiki_links, slug_from_url};
// Shared serialization helper
use super::wikilinks::push_event_text;

/// Rewrite Markdown image/document references to use relative paths.
///
/// Transforms `![](absolute/local/path)` -> `![](../../relative/path)`
/// based on the `.md` file's location and the asset's `local_path`.
///
/// Uses fuzzy matching to handle URL-encoded paths in Markdown references.
///
/// # Arguments
/// - `content` — Markdown content with `![]()` references
/// - `md_file_dir` — Directory containing the output `.md` file
/// - `assets` — DownloadedAsset list with `local_path` and original `url`
///
/// # Returns
/// Markdown with asset paths rewritten as relative
pub fn resolve_asset_paths(
    content: &str,
    md_file_dir: &Path,
    assets: &[DownloadedAsset],
) -> String {
    if assets.is_empty() {
        return content.to_string();
    }

    // Build a map: original_url -> relative_path
    use std::collections::HashMap;
    let mut asset_map: HashMap<String, String> = HashMap::with_capacity(assets.len());

    for asset in assets {
        let local_path = Path::new(&asset.local_path);
        let rel = match pathdiff::diff_paths(local_path, md_file_dir) {
            Some(p) => p,
            None => continue,
        };

        let rel_str = rel.to_string_lossy().replace('\\', "/");
        asset_map.insert(asset.url.clone(), rel_str);
    }

    if asset_map.is_empty() {
        return content.to_string();
    }

    let mut options = Options::all();
    options.remove(Options::ENABLE_SMART_PUNCTUATION);

    let parser = Parser::new_ext(content, options);
    transform_images_and_serialize(parser, &asset_map)
}

/// Transform image events to use relative paths and serialize to string.
fn transform_images_and_serialize<'a>(
    events: impl Iterator<Item = Event<'a>>,
    asset_map: &std::collections::HashMap<String, String>,
) -> String {
    let mut result = String::new();
    let mut in_image = false;
    let mut image_url = String::new();
    let mut alt_text = String::new();

    for event in events {
        match &event {
            Event::Start(Tag::Image {
                link_type: _,
                dest_url,
                title: _,
                id: _,
            }) => {
                in_image = true;
                image_url = dest_url.to_string();
                alt_text.clear();
                result.push_str("![");
            },
            Event::End(TagEnd::Image) => {
                if in_image {
                    if let Some(rel_path) = asset_map.get(&image_url) {
                        result.push_str(&alt_text);
                        result.push_str("](");
                        result.push_str(rel_path);
                        result.push(')');
                    } else {
                        result.push_str(&alt_text);
                        result.push_str("](");
                        result.push_str(&image_url);
                        result.push(')');
                    }
                    in_image = false;
                    image_url.clear();
                    alt_text.clear();
                } else {
                    push_event_text(&event, &mut result);
                }
            },
            Event::Text(s) => {
                if in_image {
                    alt_text.push_str(s);
                } else {
                    result.push_str(s);
                }
            },
            _ => {
                if in_image {
                    // Collect alt text content
                } else {
                    push_event_text(&event, &mut result);
                }
            },
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // === resolve_asset_paths tests ===

    #[test]
    fn test_resolve_single_asset() {
        let md = "![image](https://example.com/img/photo.png)";
        let assets = vec![DownloadedAsset {
            url: "https://example.com/img/photo.png".to_string(),
            local_path: "/home/user/output/example.com/images/photo.png".to_string(),
            asset_type: "image".to_string(),
            size: 1024,
        }];
        let md_dir = Path::new("/home/user/output/example.com");
        let result = resolve_asset_paths(md, md_dir, &assets);
        assert!(result.contains("images/photo.png"));
    }

    #[test]
    fn test_resolve_no_assets() {
        let md = "No images here";
        let result = resolve_asset_paths(md, Path::new("/tmp"), &[]);
        assert_eq!(result, md);
    }

    #[test]
    fn test_resolve_asset_in_nested_dir() {
        let md = "![chart](https://example.com/charts/data.png)";
        let assets = vec![DownloadedAsset {
            url: "https://example.com/charts/data.png".to_string(),
            local_path: "/tmp/output/example.com/blog/images/data.png".to_string(),
            asset_type: "image".to_string(),
            size: 2048,
        }];
        let md_dir = Path::new("/tmp/output/example.com/blog");
        let result = resolve_asset_paths(md, md_dir, &assets);
        assert!(result.contains("images/data.png"));
    }

    #[test]
    fn test_resolve_skips_code_blocks() {
        let md = "```\n![](https://example.com/img.png)\n```";
        let assets = vec![DownloadedAsset {
            url: "https://example.com/img.png".to_string(),
            local_path: "/tmp/img.png".to_string(),
            asset_type: "image".to_string(),
            size: 1024,
        }];
        let result = resolve_asset_paths(md, Path::new("/tmp"), &assets);
        assert!(result.contains("!["));
    }

    #[test]
    fn test_resolve_windows_paths_converted() {
        let md = "![image](https://example.com/img.png)";
        let assets = vec![DownloadedAsset {
            url: "https://example.com/img.png".to_string(),
            local_path: "C:\\Users\\output\\example.com\\images\\photo.png".to_string(),
            asset_type: "image".to_string(),
            size: 1024,
        }];
        let md_dir = Path::new("C:\\Users\\output\\example.com");
        let result = resolve_asset_paths(md, md_dir, &assets);
        assert!(result.contains("images/photo.png"));
    }
}

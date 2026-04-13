//! Wiki-link conversion for Obsidian vault compatibility
//!
//! Transforms Markdown links to Obsidian wiki-link syntax for same-domain URLs:
//! - `[text](https://same-domain.com/page)` → `[[page-slug|text]]`
//! - Extracts URL-safe slugs from paths

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Extract a URL-safe slug from a URL path.
///
/// Strips query strings, fragments, trailing slashes, and file extensions.
/// Takes the last path segment and converts to lowercase kebab-case.
///
/// # Examples
/// - "/blog/my-post" -> "my-post"
/// - "/docs/api/v2.html?page=1" -> "api-v2"
/// - "/" -> "index"
/// - "/My%20Post%20Title" -> "my-post-title" (URL-decoded)
pub fn slug_from_url(url_path: &str) -> String {
    // Strip query string
    let path = url_path.split('?').next().unwrap_or(url_path);
    // Strip fragment
    let path = path.split('#').next().unwrap_or(path);
    // Strip trailing slash
    let path = path.trim_end_matches('/');
    // Strip file extensions
    let path = path
        .trim_end_matches(".html")
        .trim_end_matches(".htm")
        .trim_end_matches(".php")
        .trim_end_matches(".asp")
        .trim_end_matches(".aspx")
        .trim_end_matches(".jsp");

    // Take last segment
    let segment = path.rsplit('/').next().unwrap_or(path);

    if segment.is_empty() {
        return "index".to_string();
    }

    // Manually decode common percent-encoded characters
    let decoded = segment
        .replace("%20", " ")
        .replace("%2F", "/")
        .replace("%2f", "/")
        .replace("%3A", ":")
        .replace("%3a", ":")
        .replace("%2D", "-")
        .replace("%2d", "-")
        .replace("%2E", ".")
        .replace("%2e", ".")
        .replace("_", " ");

    // Convert to lowercase and replace non-alphanumeric with hyphens
    let mut slug = String::with_capacity(decoded.len());
    let mut last_was_hyphen = false;

    for ch in decoded.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            slug.push('-');
            last_was_hyphen = true;
        }
    }

    slug.trim_matches('-').to_string()
}

/// Determines if a URL should be converted to a wiki-link.
/// Returns Some(slug) if conversion is possible, None otherwise.
fn should_convert_wikilink(url_str: &str, base_domain: &str) -> Option<String> {
    // Skip anchor links
    if url_str.starts_with('#') {
        return None;
    }

    // Skip relative paths
    if url_str.starts_with('/') && !url_str.contains("://") {
        return None;
    }

    // Try to parse the URL
    let parsed = match url::Url::parse(url_str) {
        Ok(p) => p,
        Err(_) => return None,
    };

    let host = match parsed.host_str() {
        Some(h) => h,
        None => return None,
    };

    // Only convert same-domain links
    if host != base_domain {
        return None;
    }

    let path = parsed.path();
    let slug = slug_from_url(path);
    Some(slug)
}

/// Convert Markdown links to Obsidian wiki-links for same-domain URLs.
///
/// Transforms `[link text](https://same-domain.com/page)` -> `[[page-slug|link text]]`
/// External links (different domain) are left unchanged.
///
/// # Arguments
/// - `content` — Markdown content to process
/// - `base_domain` — The domain of the scraped page (e.g. "example.com")
///
/// # Returns
/// Markdown with same-domain links converted to wiki-link syntax
pub fn convert_wiki_links(content: &str, base_domain: &str) -> String {
    let mut options = Options::all();
    options.remove(Options::ENABLE_SMART_PUNCTUATION);

    let parser = Parser::new_ext(content, options);
    transform_and_serialize(parser, base_domain)
}

/// Transform link events to wiki-links and serialize to string.
fn transform_and_serialize<'a>(
    events: impl Iterator<Item = Event<'a>>,
    base_domain: &str,
) -> String {
    let mut result = String::new();
    let mut in_link = false;
    let mut link_url = String::new();
    let mut link_text_parts: Vec<Event<'a>> = Vec::new();
    let mut depth = 0;

    for event in events {
        match &event {
            Event::Start(Tag::Link {
                dest_url,
                title: _,
                id: _,
                link_type: _,
            }) => {
                if depth == 0 {
                    in_link = true;
                    link_url = dest_url.to_string();
                    link_text_parts.clear();
                }
                depth += 1;
                if !in_link {
                    push_event_text(&event, &mut result);
                }
            },
            Event::End(TagEnd::Link) => {
                if depth == 1 && in_link {
                    in_link = false;
                    depth = 0;

                    if let Some(slug) = should_convert_wikilink(&link_url, base_domain) {
                        let link_text = extract_text_from_events(&link_text_parts);
                        let normalized_text = link_text.to_lowercase().trim().replace(' ', "-");

                        if slug == normalized_text {
                            result.push_str("[[");
                            result.push_str(&slug);
                            result.push_str("]]");
                        } else {
                            result.push_str("[[");
                            result.push_str(&slug);
                            result.push('|');
                            result.push_str(&link_text);
                            result.push_str("]]");
                        }
                    } else {
                        let link_text = extract_text_from_events(&link_text_parts);
                        result.push('[');
                        result.push_str(&link_text);
                        result.push_str("](");
                        result.push_str(&link_url);
                        result.push(')');
                    }
                    link_text_parts.clear();
                } else {
                    depth -= 1;
                    if !in_link {
                        push_event_text(&event, &mut result);
                    }
                }
            },
            Event::Start(Tag::Image { .. }) => {
                if in_link {
                    link_text_parts.push(event);
                } else {
                    push_event_text(&event, &mut result);
                }
            },
            Event::Start(_) => {
                if in_link {
                    depth += 1;
                    link_text_parts.push(event);
                } else {
                    push_event_text(&event, &mut result);
                }
            },
            Event::End(TagEnd::Image) => {
                if in_link {
                    link_text_parts.push(event);
                } else {
                    push_event_text(&event, &mut result);
                }
            },
            Event::End(_) => {
                if in_link && depth > 1 {
                    depth -= 1;
                    link_text_parts.push(event);
                } else if in_link {
                    link_text_parts.push(event);
                } else {
                    push_event_text(&event, &mut result);
                }
            },
            _ => {
                if in_link {
                    link_text_parts.push(event);
                } else {
                    push_event_text(&event, &mut result);
                }
            },
        }
    }

    if in_link {
        for e in link_text_parts.drain(..) {
            push_event_text(&e, &mut result);
        }
    }

    result.trim_end().to_string()
}

/// Push the text representation of an event to the result string.
/// Shared with obsidian.rs for asset path transformation.
pub(crate) fn push_event_text(event: &Event, result: &mut String) {
    match event {
        Event::Text(s) => result.push_str(s),
        Event::Code(s) => {
            result.push('`');
            result.push_str(s);
            result.push('`');
        },
        Event::Html(s) => result.push_str(s),
        Event::FootnoteReference(s) => {
            result.push_str("[^");
            result.push_str(s);
            result.push(']');
        },
        Event::TaskListMarker(checked) => {
            result.push_str(if *checked { "- [x] " } else { "- [ ] " });
        },
        Event::SoftBreak => result.push('\n'),
        Event::HardBreak => result.push_str("  \n"),
        Event::Rule => result.push_str("---\n"),
        Event::InlineMath(s) => {
            result.push('$');
            result.push_str(s);
            result.push('$');
        },
        Event::DisplayMath(s) => {
            result.push_str("$$");
            result.push_str(s);
            result.push_str("$$");
        },
        Event::Start(Tag::Link { .. }) => {},
        Event::End(TagEnd::Link) => {},
        Event::Start(Tag::Image { .. }) => {
            result.push_str("![");
        },
        Event::End(TagEnd::Image) => {},
        Event::Start(Tag::Paragraph) => {},
        Event::End(TagEnd::Paragraph) => result.push_str("\n\n"),
        Event::Start(Tag::CodeBlock(_)) => result.push_str("```\n"),
        Event::End(TagEnd::CodeBlock) => result.push_str("\n```\n"),
        Event::Start(Tag::BlockQuote(_)) => result.push_str("> "),
        Event::End(TagEnd::BlockQuote(_)) => result.push('\n'),
        Event::Start(Tag::List(_)) => {},
        Event::End(TagEnd::List(_)) => {},
        Event::Start(Tag::Item) => {},
        Event::End(TagEnd::Item) => {},
        Event::Start(Tag::Table(_)) => {},
        Event::End(TagEnd::Table) => result.push('\n'),
        Event::Start(Tag::TableRow) => {},
        Event::End(TagEnd::TableRow) => result.push('\n'),
        Event::Start(Tag::TableCell) => {},
        Event::End(TagEnd::TableCell) => result.push('|'),
        Event::Start(Tag::FootnoteDefinition(s)) => {
            result.push_str("[^");
            result.push_str(s);
            result.push_str("]: ");
        },
        Event::End(TagEnd::FootnoteDefinition) => result.push_str("\n\n"),
        Event::Start(Tag::Emphasis) => result.push('*'),
        Event::End(TagEnd::Emphasis) => result.push('*'),
        Event::Start(Tag::Strong) => result.push_str("**"),
        Event::End(TagEnd::Strong) => result.push_str("**"),
        Event::Start(Tag::Strikethrough) => result.push_str("~~"),
        Event::End(TagEnd::Strikethrough) => result.push_str("~~"),
        Event::Start(Tag::Heading { .. }) => {},
        Event::End(TagEnd::Heading(_)) => result.push('\n'),
        Event::Start(Tag::MetadataBlock(_)) => {},
        Event::End(TagEnd::MetadataBlock(_)) => result.push_str("---\n"),
        _ => {},
    }
}

/// Extract plain text from a sequence of events.
fn extract_text_from_events(events: &[Event]) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            Event::Text(s) => text.push_str(s),
            Event::Code(s) => text.push_str(s),
            _ => {},
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_same_domain_link() {
        let md = "[Read more](https://example.com/about)";
        let result = convert_wiki_links(md, "example.com");
        assert_eq!(result, "[[about|Read more]]");
    }

    #[test]
    fn test_skip_external_domain_link() {
        let md = "[Google](https://google.com)";
        let result = convert_wiki_links(md, "example.com");
        assert_eq!(result, "[Google](https://google.com)");
    }

    #[test]
    fn test_skip_links_in_code_block() {
        let md = "```\n[not a link](https://example.com/foo)\n```";
        let result = convert_wiki_links(md, "example.com");
        assert!(result.contains("[not a link]"));
    }

    #[test]
    fn test_skip_inline_code_link() {
        let md = "Use `[link](https://example.com)` for docs";
        let result = convert_wiki_links(md, "example.com");
        assert!(result.contains("[link](https://example.com)"));
    }

    #[test]
    fn test_multiple_links_mixed() {
        let md = "[internal](https://example.com/a) and [external](https://other.com/b)";
        let result = convert_wiki_links(md, "example.com");
        assert!(result.contains("[[a|internal]]"));
        assert!(result.contains("[external](https://other.com/b)"));
    }

    #[test]
    fn test_identical_links_all_converted() {
        let md = "[link](https://example.com/x) and [link](https://example.com/x)";
        let result = convert_wiki_links(md, "example.com");
        assert_eq!(result.matches("[[x|link]]").count(), 2);
    }

    #[test]
    fn test_anchor_links_unchanged() {
        let md = "[Section](#section)";
        let result = convert_wiki_links(md, "example.com");
        assert_eq!(result, md);
    }

    #[test]
    fn test_relative_links_unchanged() {
        let md = "[About](/about)";
        let result = convert_wiki_links(md, "example.com");
        assert_eq!(result, md);
    }

    #[test]
    fn test_slug_simple_path() {
        assert_eq!(slug_from_url("/blog/my-post"), "my-post");
    }

    #[test]
    fn test_slug_with_query_and_fragment() {
        assert_eq!(slug_from_url("/page?id=1#section"), "page");
    }

    #[test]
    fn test_slug_root_path() {
        assert_eq!(slug_from_url("/"), "index");
    }

    #[test]
    fn test_slug_with_extension() {
        assert_eq!(slug_from_url("/docs/api.html"), "api");
    }

    #[test]
    fn test_slug_url_encoded() {
        assert_eq!(slug_from_url("/My%20Post%20Title"), "my-post-title");
    }

    #[test]
    fn test_slug_nested_with_date() {
        assert_eq!(slug_from_url("/2026/04/03/hello-world/"), "hello-world");
    }

    #[test]
    fn test_slug_trailing_slash() {
        assert_eq!(slug_from_url("/blog/"), "blog");
    }

    #[test]
    fn test_slug_multiple_extensions() {
        assert_eq!(slug_from_url("/page.asp?id=1"), "page");
    }
}

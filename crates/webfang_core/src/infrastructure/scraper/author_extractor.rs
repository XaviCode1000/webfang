//! Multi-strategy author extraction
//!
//! Extracts the author of a page by cascading through independent
//! strategies, ordered from most to least reliable:
//!
//! 1. JSON-LD structured data (`application/ld+json`)
//! 2. `<meta>` tags (`name="author"`, `property="article:author"`)
//! 3. Microdata (`[itemprop="author"]`, `[rel="author"]`)
//! 4. Common CSS classes (`.author`, `.byline`, ...)
//! 5. Legible byline heuristic (fallback, supplied by the caller)
//!
//! The legible byline alone misses most real-world authors because it only
//! looks for `<address>` tags and narrow byline patterns. This cascade fills
//! the gap without touching readability's content-body extraction.
//!
//! # Rules Applied
//!
//! - **own-borrow-over-clone**: Accept `&str` not `&String`
//! - **err-no-unwrap-prod**: Static selectors use `expect("BUG: ...")` only for
//!   compile-time-known-valid selectors (matches `extractor/mod.rs`)
//! - **opt-inline**: CSS selectors compiled once via `LazyLock`

use std::sync::LazyLock;

use scraper::{Html, Selector};
use serde_json::Value;

/// Maximum length of a plausible author name.
///
/// CSS-class candidates longer than this are almost certainly containers
/// (a whole paragraph or a footer) rather than a name, so they are skipped.
const MAX_AUTHOR_LEN: usize = 100;

/// Leading byline prefixes stripped from CSS-class candidates.
///
/// Ordered longest-first so "Written by" wins over "by".
const BYLINE_PREFIXES: &[&str] = &["written by", "por", "by"];

static SEL_JSONLD: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("script[type=\"application/ld+json\"]")
        .expect("BUG: invalid CSS selector application/ld+json")
});
static SEL_META_AUTHOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("meta[name=\"author\"], meta[property=\"article:author\"]")
        .expect("BUG: invalid CSS selector meta author")
});
static SEL_ITEMPROP_AUTHOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("[itemprop=\"author\"]").expect("BUG: invalid CSS selector itemprop=author")
});
static SEL_ITEMPROP_NAME: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("[itemprop=\"name\"]").expect("BUG: invalid CSS selector itemprop=name")
});
static SEL_REL_AUTHOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("[rel=\"author\"]").expect("BUG: invalid CSS selector rel=author")
});
static SEL_CSS_CLASS: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".author, .byline, .post-author, .article-author")
        .expect("BUG: invalid CSS selector author classes")
});

/// Extracts author information from HTML using a specific strategy.
pub trait AuthorExtractor: Send + Sync {
    /// Human-readable strategy name for logging.
    fn name(&self) -> &str;

    /// Try to extract an author from the HTML. Returns `None` if this
    /// strategy finds nothing.
    fn extract(&self, html: &str) -> Option<String>;
}

/// Strategy 1: JSON-LD structured data.
///
/// Handles `{"author": {"name": "..."}}`, `{"author": "..."}`,
/// `{"author": [{"name": "..."}]}` and `@graph` wrappers.
#[derive(Default)]
pub struct JsonLdAuthorExtractor;

impl AuthorExtractor for JsonLdAuthorExtractor {
    fn name(&self) -> &str {
        "json-ld"
    }

    fn extract(&self, html: &str) -> Option<String> {
        let document = Html::parse_document(html);
        document.select(&SEL_JSONLD).find_map(|element| {
            let json: String = element.text().collect();
            author_from_jsonld(&json)
        })
    }
}

/// Strategy 2: `<meta>` tags.
#[derive(Default)]
pub struct MetaTagAuthorExtractor;

impl AuthorExtractor for MetaTagAuthorExtractor {
    fn name(&self) -> &str {
        "meta-tag"
    }

    fn extract(&self, html: &str) -> Option<String> {
        let document = Html::parse_document(html);
        document
            .select(&SEL_META_AUTHOR)
            .find_map(|element| element.value().attr("content").and_then(non_empty))
    }
}

/// Strategy 3: microdata (`itemprop` / `rel`).
#[derive(Default)]
pub struct ItempropAuthorExtractor;

impl AuthorExtractor for ItempropAuthorExtractor {
    fn name(&self) -> &str {
        "itemprop"
    }

    fn extract(&self, html: &str) -> Option<String> {
        let document = Html::parse_document(html);

        for element in document.select(&SEL_ITEMPROP_AUTHOR) {
            let nested_name = element
                .select(&SEL_ITEMPROP_NAME)
                .next()
                .and_then(|name_el| text_content(&name_el));
            if let Some(name) = nested_name {
                return Some(name);
            }
            if let Some(content) = element.value().attr("content").and_then(non_empty) {
                return Some(content);
            }
            if let Some(text) = text_content(&element) {
                return Some(text);
            }
        }

        document
            .select(&SEL_REL_AUTHOR)
            .find_map(|element| text_content(&element))
    }
}

/// Strategy 4: common CSS classes, with byline-prefix stripping.
#[derive(Default)]
pub struct CssClassAuthorExtractor;

impl AuthorExtractor for CssClassAuthorExtractor {
    fn name(&self) -> &str {
        "css-class"
    }

    fn extract(&self, html: &str) -> Option<String> {
        let document = Html::parse_document(html);
        document.select(&SEL_CSS_CLASS).find_map(|element| {
            let raw = text_content(&element)?;
            let candidate = strip_byline_prefix(&raw);
            let candidate = candidate.trim();
            if candidate.is_empty() || candidate.len() > MAX_AUTHOR_LEN {
                None
            } else {
                Some(candidate.to_string())
            }
        })
    }
}

/// Try each extraction strategy in order, returning the first successful
/// result. Falls back to the legible byline when no strategy matches.
///
/// # Arguments
///
/// * `html` - Full raw HTML of the page (must include `<head>` for meta/JSON-LD)
/// * `legible_byline` - Byline produced by readability, used as last resort
///
/// # Returns
///
/// * `Some(author)` - The first non-empty author found
/// * `None` - No strategy matched and no byline was supplied
pub fn extract_author(html: &str, legible_byline: Option<&str>) -> Option<String> {
    let extractors: Vec<Box<dyn AuthorExtractor>> = vec![
        Box::new(JsonLdAuthorExtractor),
        Box::new(MetaTagAuthorExtractor),
        Box::new(ItempropAuthorExtractor),
        Box::new(CssClassAuthorExtractor),
    ];

    for extractor in &extractors {
        if let Some(author) = extractor.extract(html) {
            let author = author.trim().to_string();
            if !author.is_empty() {
                tracing::debug!(strategy = extractor.name(), %author, "author extracted");
                return Some(author);
            }
        }
    }

    legible_byline
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
}

/// Resolve an author name from a raw JSON-LD block.
fn author_from_jsonld(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    find_author(&value)
}

/// Recursively search a JSON-LD value for an `author` field.
fn find_author(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(name) = map.get("author").and_then(author_name) {
                return Some(name);
            }
            map.get("@graph").and_then(find_author)
        },
        Value::Array(items) => items.iter().find_map(find_author),
        _ => None,
    }
}

/// Coerce an `author` JSON value (string, object, or array) into a name.
fn author_name(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => non_empty(s),
        Value::Object(map) => map.get("name").and_then(Value::as_str).and_then(non_empty),
        Value::Array(items) => items.iter().find_map(author_name),
        _ => None,
    }
}

/// Trim and return the string, or `None` if it is empty/whitespace.
fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Collapse an element's text nodes into a single trimmed string.
fn text_content(element: &scraper::ElementRef<'_>) -> Option<String> {
    let collapsed: String = element.text().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    non_empty(&collapsed)
}

/// Strip a leading byline prefix ("By ", "Written by ", "Por ") if present.
fn strip_byline_prefix(s: &str) -> String {
    let trimmed = s.trim();
    let lower = trimmed.to_lowercase();
    for prefix in BYLINE_PREFIXES {
        let Some(rest) = lower.strip_prefix(prefix) else {
            continue;
        };
        if rest.starts_with(' ') || rest.starts_with('\u{a0}') {
            return trimmed[prefix.len()..].trim().to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    fn extract_with(extractor: &dyn AuthorExtractor, html: &str) -> Option<String> {
        extractor.extract(html)
    }

    // ---- JSON-LD strategy ----

    #[test]
    fn jsonld_object_form() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"@type":"Article","author":{"name":"John Doe"}}</script>
        </head><body></body></html>"#;
        assert_eq!(
            extract_with(&JsonLdAuthorExtractor, html).as_deref(),
            Some("John Doe")
        );
    }

    #[test]
    fn jsonld_string_form() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"author":"Jane Roe"}</script>
        </head><body></body></html>"#;
        assert_eq!(
            extract_with(&JsonLdAuthorExtractor, html).as_deref(),
            Some("Jane Roe")
        );
    }

    #[test]
    fn jsonld_array_form() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"author":[{"name":"Alice"},{"name":"Bob"}]}</script>
        </head><body></body></html>"#;
        assert_eq!(
            extract_with(&JsonLdAuthorExtractor, html).as_deref(),
            Some("Alice")
        );
    }

    #[test]
    fn jsonld_graph_wrapper() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"@graph":[{"@type":"Article","author":{"name":"Graph Author"}}]}</script>
        </head><body></body></html>"#;
        assert_eq!(
            extract_with(&JsonLdAuthorExtractor, html).as_deref(),
            Some("Graph Author")
        );
    }

    #[test]
    fn jsonld_missing_author() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"@type":"Article","headline":"No author"}</script>
        </head><body></body></html>"#;
        assert_eq!(extract_with(&JsonLdAuthorExtractor, html), None);
    }

    #[test]
    fn jsonld_malformed_json() {
        let html = r#"<html><head>
            <script type="application/ld+json">{not valid json</script>
        </head><body></body></html>"#;
        assert_eq!(extract_with(&JsonLdAuthorExtractor, html), None);
    }

    // ---- Meta tag strategy ----

    #[test]
    fn meta_name_author() {
        let html =
            r#"<html><head><meta name="author" content="Meta Author"></head><body></body></html>"#;
        assert_eq!(
            extract_with(&MetaTagAuthorExtractor, html).as_deref(),
            Some("Meta Author")
        );
    }

    #[test]
    fn meta_property_article_author() {
        let html = r#"<html><head><meta property="article:author" content="OG Author"></head><body></body></html>"#;
        assert_eq!(
            extract_with(&MetaTagAuthorExtractor, html).as_deref(),
            Some("OG Author")
        );
    }

    #[test]
    fn meta_missing() {
        let html =
            r#"<html><head><meta name="description" content="x"></head><body></body></html>"#;
        assert_eq!(extract_with(&MetaTagAuthorExtractor, html), None);
    }

    // ---- Itemprop strategy ----

    #[test]
    fn itemprop_author_text() {
        let html = r#"<html><body><span itemprop="author">Microdata Author</span></body></html>"#;
        assert_eq!(
            extract_with(&ItempropAuthorExtractor, html).as_deref(),
            Some("Microdata Author")
        );
    }

    #[test]
    fn itemprop_author_nested_name() {
        let html = r#"<html><body>
            <div itemprop="author" itemscope><span itemprop="name">Nested Name</span></div>
        </body></html>"#;
        assert_eq!(
            extract_with(&ItempropAuthorExtractor, html).as_deref(),
            Some("Nested Name")
        );
    }

    #[test]
    fn rel_author_link() {
        let html = r#"<html><body><a rel="author" href="/about">Rel Author</a></body></html>"#;
        assert_eq!(
            extract_with(&ItempropAuthorExtractor, html).as_deref(),
            Some("Rel Author")
        );
    }

    // ---- CSS class strategy ----

    #[test]
    fn css_author_class() {
        let html = r#"<html><body><div class="author">Plain Author</div></body></html>"#;
        assert_eq!(
            extract_with(&CssClassAuthorExtractor, html).as_deref(),
            Some("Plain Author")
        );
    }

    #[test]
    fn css_byline_prefix_stripped() {
        let html = r#"<html><body><span class="byline">By John Doe</span></body></html>"#;
        assert_eq!(
            extract_with(&CssClassAuthorExtractor, html).as_deref(),
            Some("John Doe")
        );
    }

    #[test]
    fn css_written_by_prefix_stripped() {
        let html =
            r#"<html><body><span class="post-author">Written by Jane Roe</span></body></html>"#;
        assert_eq!(
            extract_with(&CssClassAuthorExtractor, html).as_deref(),
            Some("Jane Roe")
        );
    }

    #[test]
    fn css_rejects_overlong_container() {
        let long = "x".repeat(MAX_AUTHOR_LEN + 1);
        let html = format!(r#"<html><body><div class="author">{long}</div></body></html>"#);
        assert_eq!(extract_with(&CssClassAuthorExtractor, &html), None);
    }

    // ---- Cascade ----

    #[test]
    fn cascade_first_match_wins() {
        // Both JSON-LD and meta present: JSON-LD must win (higher priority).
        let html = r#"<html><head>
            <meta name="author" content="Meta Loser">
            <script type="application/ld+json">{"author":"Json Winner"}</script>
        </head><body></body></html>"#;
        assert_eq!(extract_author(html, None).as_deref(), Some("Json Winner"));
    }

    #[test]
    fn cascade_falls_back_to_byline() {
        let html = r#"<html><head><title>No author anywhere</title></head><body><p>text</p></body></html>"#;
        assert_eq!(
            extract_author(html, Some("Legible Byline")).as_deref(),
            Some("Legible Byline")
        );
    }

    #[test]
    fn cascade_all_empty_returns_none() {
        let html = r#"<html><head></head><body><p>nothing</p></body></html>"#;
        assert_eq!(extract_author(html, None), None);
        assert_eq!(extract_author(html, Some("   ")), None);
    }

    #[test]
    fn cascade_trims_whitespace() {
        let html = r#"<html><head><meta name="author" content="  Spaced Author  "></head><body></body></html>"#;
        assert_eq!(extract_author(html, None).as_deref(), Some("Spaced Author"));
    }

    // ---- Integration with readability ----

    #[test]
    fn integration_jsonld_beats_legible_byline() {
        // A realistic page: JSON-LD author present, legible byline also present.
        // The cascade must prefer the structured JSON-LD author.
        let html = r#"<html><head>
            <title>Article Title</title>
            <script type="application/ld+json">
            {"@context":"https://schema.org","@type":"NewsArticle","author":{"name":"Structured Reporter"}}
            </script>
        </head><body>
            <article>
                <h1>Article Title</h1>
                <p>First paragraph with enough content for readability to work properly here.</p>
                <p>Second paragraph adds more body text so the algorithm keeps the article.</p>
            </article>
        </body></html>"#;

        let article =
            crate::infrastructure::scraper::readability::parse(html, Some("https://example.com"))
                .expect("readability should parse a valid article");
        let author = extract_author(html, article.byline.as_deref());
        assert_eq!(author.as_deref(), Some("Structured Reporter"));
    }
}

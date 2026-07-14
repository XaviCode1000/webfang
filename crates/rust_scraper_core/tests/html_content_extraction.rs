//! HTML content extraction integration tests using shared fixtures.

mod common;

use common::*;

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn extract_title_from_sample_html() {
    let html = sample_html();
    let doc = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("title").unwrap();
    let title = doc
        .select(&sel)
        .next()
        .map(|el| el.text().collect::<String>());
    assert_eq!(title.as_deref(), Some("Test Article - Example Domain"));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn extract_article_content() {
    let html = sample_html();
    let doc = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse(".content p").unwrap();
    let paragraphs: Vec<String> = doc
        .select(&sel)
        .map(|el| el.text().collect::<String>())
        .collect();
    assert!(paragraphs.len() >= 2);
    assert!(paragraphs[0].contains("main content"));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn extract_links_from_html() {
    let html = sample_html();
    let doc = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("a[href]").unwrap();
    let links: Vec<String> = doc
        .select(&sel)
        .filter_map(|el| el.value().attr("href").map(String::from))
        .collect();
    assert!(links.len() >= 2);
    assert!(links.iter().any(|l| l.contains("/page/2")));
    assert!(links.iter().any(|l| l.contains("external.com")));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn extract_meta_description() {
    let html = sample_html();
    let doc = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("meta[name='description']").unwrap();
    let desc = doc
        .select(&sel)
        .next()
        .and_then(|el| el.value().attr("content"));
    assert_eq!(desc, Some("A test article for parsing"));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn noisy_html_main_content_extraction() {
    let html = sample_noisy_html();
    let doc = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("main h1").unwrap();
    let heading = doc
        .select(&sel)
        .next()
        .map(|el| el.text().collect::<String>());
    assert_eq!(heading.as_deref(), Some("Actual Content"));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn nested_html_deep_content() {
    let html = sample_nested_html();
    let doc = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse(".level-3 p").unwrap();
    let text = doc
        .select(&sel)
        .next()
        .map(|el| el.text().collect::<String>());
    assert_eq!(text.as_deref(), Some("Deep content"));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn minimal_html_parseable() {
    let html = sample_minimal_html();
    let doc = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("p").unwrap();
    let text = doc
        .select(&sel)
        .next()
        .map(|el| el.text().collect::<String>());
    assert_eq!(text.as_deref(), Some("Hello world"));
}

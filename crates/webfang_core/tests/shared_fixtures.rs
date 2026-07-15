//! Integration tests verifying shared fixtures are consumable.
//!
//! These tests confirm the `tests/common/` module works correctly
//! and that fixtures produce expected results with core crate parsers.

mod common;

use common::*;

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_sample_html_parseable() {
    let html = sample_html();
    let document = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("title").unwrap();
    let title = document.select(&sel);
    let texts: Vec<_> = title.map(|el| el.text().collect::<String>()).collect();
    assert!(!texts.is_empty(), "should find title element");
    assert!(texts[0].contains("Test Article"));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_sample_html_link_extraction() {
    let html = sample_html();
    let document = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("a[href]").unwrap();
    let links: Vec<String> = document
        .select(&sel)
        .filter_map(|el| el.value().attr("href").map(String::from))
        .collect();
    assert!(links.len() >= 2, "should find at least 2 links");
    assert!(links.iter().any(|l| l.contains("/page/2")));
}

#[test]
fn test_sample_sitemap_parseable() {
    let xml = sample_sitemap();
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut url_count = 0;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(ref e)) if e.name().as_ref() == b"url" => {
                url_count += 1;
            },
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
    }
    assert_eq!(url_count, 3, "should find 3 URLs in sitemap");
}

#[test]
fn test_sample_sitemap_index_parseable() {
    let xml = sample_sitemap_index();
    assert!(xml.contains("<sitemap>"));
    assert!(xml.contains("sitemap-posts.xml"));
}

#[test]
fn test_sample_urls_format() {
    let urls = sample_urls(3);
    for url in &urls {
        assert!(url.starts_with("https://example.com/"));
    }
}

#[test]
fn test_temp_dir_helper_lifecycle() {
    let tmp = TempDirHelper::new();
    let file = tmp.path().join("test.txt");
    std::fs::write(&file, "hello").unwrap();
    assert!(file.exists());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello");
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_minimal_html_parseable() {
    let html = sample_minimal_html();
    let document = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("p").unwrap();
    let p = document.select(&sel);
    let texts: Vec<_> = p.map(|el| el.text().collect::<String>()).collect();
    assert!(texts.iter().any(|t| t.contains("Hello world")));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_noisy_html_has_main_content() {
    let html = sample_noisy_html();
    let document = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse("main").unwrap();
    let main = document.select(&sel);
    let texts: Vec<_> = main.map(|el| el.text().collect::<String>()).collect();
    assert!(texts.iter().any(|t| t.contains("Actual Content")));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_nested_html_depth() {
    let html = sample_nested_html();
    let document = scraper::Html::parse_document(html);
    let sel = scraper::Selector::parse(".level-3 p").unwrap();
    let deep = document.select(&sel);
    let texts: Vec<_> = deep.map(|el| el.text().collect::<String>()).collect();
    assert!(texts.iter().any(|t| t.contains("Deep content")));
}

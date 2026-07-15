//! Sitemap parsing integration tests using shared fixtures.

mod common;

use common::*;

#[test]
fn parse_standard_sitemap() {
    let xml = sample_sitemap();
    let doc = scraper::Html::parse_document(xml);
    // Verify it's parseable as XML via quick_xml
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
    assert_eq!(url_count, 3);
    let _ = doc; // ensure html parsing also works
}

#[test]
fn parse_sitemap_index() {
    let xml = sample_sitemap_index();
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut sitemap_count = 0;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(ref e)) if e.name().as_ref() == b"sitemap" => {
                sitemap_count += 1;
            },
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
    }
    assert_eq!(sitemap_count, 2);
}

#[test]
fn parse_empty_sitemap_yields_zero_urls() {
    let xml = sample_empty_sitemap();
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
    assert_eq!(url_count, 0);
}

#[test]
fn special_chars_sitemap_preserves_urls() {
    let xml = sample_sitemap_special_chars();
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut urls = Vec::new();
    let mut in_loc = false;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(ref e)) if e.name().as_ref() == b"loc" => {
                in_loc = true;
            },
            Ok(quick_xml::events::Event::Text(ref t)) if in_loc => {
                if let Ok(s) = std::str::from_utf8(t.as_ref()) {
                    urls.push(s.to_string());
                }
                in_loc = false;
            },
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {
                in_loc = false;
            },
        }
    }
    assert_eq!(urls.len(), 3);
    assert!(urls.iter().any(|u| u.contains("path%20with")));
}

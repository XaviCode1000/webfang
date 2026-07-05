#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(xml) = std::str::from_utf8(data) {
        if let Ok(base_url) = url::Url::parse("https://example.com") {
            let _ = rust_scraper::application::crawler::discovery::parse_sitemap(xml, &base_url);
        }
    }
});

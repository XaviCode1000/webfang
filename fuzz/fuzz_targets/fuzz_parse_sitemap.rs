#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz XML sitemap parser — processes untrusted XML from remote servers.
// Uses zero-allocation streaming XML parser (quick-xml).
// Panic = DoS when crawling sites with malformed sitemaps.
fuzz_target!(|data: &[u8]| {
    if let Ok(xml) = std::str::from_utf8(data) {
        if let Ok(base_url) = url::Url::parse("https://example.com/sitemap.xml") {
            // parse_sitemap is sync and returns Result — errors are expected
            let _ = webfang::application::crawler::discovery::parse_sitemap(xml, &base_url);
        }
    }
});

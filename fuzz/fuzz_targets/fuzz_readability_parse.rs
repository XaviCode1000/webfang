#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz Readability parser — extracts article content from HTML pages.
// This is the most complex parser in the pipeline. Panic = DoS.
fuzz_target!(|data: &[u8]| {
    if let Ok(html) = std::str::from_utf8(data) {
        // parse() returns Result, so errors are expected and fine
        let _ = webfang::infrastructure::scraper::readability::parse(html, Some("https://example.com/page"));
    }
});

#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz link extraction — extracts URLs from untrusted HTML.
// Uses HTML parser + URL resolution. Panic = DoS during crawl.
fuzz_target!(|data: &[u8]| {
    // Split input into HTML + base URL for more realistic fuzzing
    let mid = data.len() / 2;
    if let (Ok(html), Ok(base)) = (
        std::str::from_utf8(&data[..mid]),
        std::str::from_utf8(&data[mid..]),
    ) {
        // Only fuzz with valid base URLs to focus on HTML parsing
        if url::Url::parse(base).is_ok() {
            let _ = webfang::infrastructure::crawler::link_extractor::extract_links(html, base);
        }
    }
});

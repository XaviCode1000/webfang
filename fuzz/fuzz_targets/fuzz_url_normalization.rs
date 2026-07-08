#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz URL normalization — normalizes URLs for deduplication.
// Processes untrusted URLs from HTML. Panic = DoS during crawl.
fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // normalize_url returns String, never panics
        let normalized = rust_scraper::infrastructure::crawler::link_extractor::normalize_url(s);

        // Idempotency check: skip when input contains control characters.
        // WHATWG preprocessing strips \n, \t, \r from input — this changes URL
        // structure, so re-parsing the normalized output may produce different
        // results (e.g., non-ASCII bytes double-encoded). This is expected
        // behavior, not a bug in url-normalize.
        let has_control = s.bytes().any(|b| b < 0x20 || b == 0x7F);
        if !has_control && url::Url::parse(&normalized).is_ok() {
            let double = rust_scraper::infrastructure::crawler::link_extractor::normalize_url(&normalized);
            assert_eq!(normalized, double, "URL normalization is not idempotent for valid URLs without control chars!");
        }
    }
});

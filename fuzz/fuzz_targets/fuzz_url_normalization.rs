#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz URL normalization — normalizes URLs for deduplication.
// Processes untrusted URLs from HTML. Panic = DoS during crawl.
//
// NOTE: We do NOT test idempotency here. url-normalize applies WHATWG
// preprocessing (control char removal, backslash conversion, encoding)
// which changes URL structure — re-parsing the normalized output may
// produce different results. This is expected crate behavior, not a bug.
// Our goal: normalize_url must NEVER panic on any input.
fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = webfang::infrastructure::crawler::link_extractor::normalize_url(s);
    }
});

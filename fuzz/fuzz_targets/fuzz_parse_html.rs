#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz HTML parser — processes raw HTML from the internet through the
// scraper service's extract_with_selector pipeline.
// This exercises the HTML parsing + CSS selector matching code path,
// which is distinct from the readability parser (fuzz_readability_parse).
// Panic = DoS when crawling sites with malformed HTML.
fuzz_target!(|data: &[u8]| {
    if let Ok(html) = std::str::from_utf8(data) {
        // Use a non-body selector to exercise the HTML parsing + CSS
        // selector matching pipeline. Selector parse errors are handled
        // gracefully (returned as Err), so no panic is possible.
        let _ = webfang::application::scraper_service::extract_with_selector(html, "div", None);
    }
});

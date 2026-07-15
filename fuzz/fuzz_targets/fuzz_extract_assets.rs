#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz asset extraction — extracts image/document URLs from HTML.
// Processes untrusted HTML. Panic = DoS during download phase.
fuzz_target!(|data: &[u8]| {
    if let Ok(html) = std::str::from_utf8(data) {
        if let Ok(base_url) = url::Url::parse("https://example.com/page") {
            // extract_all_assets returns Vec<AssetUrl> — empty is fine
            let _ = webfang::extractor::extract_all_assets(html, &base_url);
        }
    }
});

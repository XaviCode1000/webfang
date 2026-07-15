#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz fallback text extraction — processes HTML when Readability fails.
// This is the safety net for malformed content. Panic = DoS.
fuzz_target!(|data: &[u8]| {
    if let Ok(html) = std::str::from_utf8(data) {
        let text = webfang::infrastructure::scraper::fallback::extract_text(html);

        // Sanity: non-empty HTML should produce some text
        if !html.is_empty() && text.is_empty() {
            eprintln!("WARN: non-empty HTML produced empty text ({} bytes)", html.len());
        }
    }
});

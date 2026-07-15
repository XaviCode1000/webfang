#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz the HTML cleaner — processes raw HTML from the internet through lol_html.
// This is the FIRST gate in the scraping pipeline. A panic here = DoS.
fuzz_target!(|data: &[u8]| {
    if let Ok(html) = std::str::from_utf8(data) {
        // clean_html must NEVER panic on any input.
        let cleaned = webfang::infrastructure::converter::html_cleaner::clean_html(html);

        // Sanity: output should be valid UTF-8 (it returns String, so guaranteed)
        // but check that it doesn't silently produce empty output for non-empty input
        // (this helps the fuzzer find inputs that bypass all cleaning rules)
        if !html.is_empty() && cleaned.is_empty() {
            // Not a panic, but interesting — log for analysis
            eprintln!("WARN: non-empty HTML produced empty output ({} bytes)", html.len());
        }
    }
});

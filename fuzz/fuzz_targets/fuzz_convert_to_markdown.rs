#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz HTML-to-Markdown conversion — two-stage pipeline (clean_html + html_to_markdown_rs).
// Processes untrusted HTML. Panic = DoS on the scraper.
fuzz_target!(|data: &[u8]| {
    if let Ok(html) = std::str::from_utf8(data) {
        let markdown = webfang::infrastructure::converter::html_to_markdown::convert_to_markdown(html);

        // Sanity: non-empty HTML should produce some output
        if !html.is_empty() && markdown.is_empty() {
            eprintln!("WARN: non-empty HTML produced empty markdown ({} bytes)", html.len());
        }
    }
});

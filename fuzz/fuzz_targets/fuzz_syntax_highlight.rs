#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz syntax highlighting — applies code block highlighting to markdown.
// Processes markdown with code blocks. Panic = DoS during export.
fuzz_target!(|data: &[u8]| {
    if let Ok(markdown) = std::str::from_utf8(data) {
        // highlight_code_blocks returns String, uses regex + syntect
        let _ = webfang::infrastructure::converter::syntax_highlight::highlight_code_blocks(markdown);
    }
});

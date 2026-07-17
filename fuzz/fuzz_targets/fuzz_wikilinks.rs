#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz Obsidian wiki-link conversion — converts markdown links to [[wikilinks]].
// Processes scraped markdown content. Panic = DoS during Obsidian export.
fuzz_target!(|data: &[u8]| {
    if let Ok(content) = std::str::from_utf8(data) {
        // convert_wiki_links returns String, never panics
        let _ = webfang::infrastructure::converter::wikilinks::convert_wiki_links(
            content, "example.com",
        );
    }
});

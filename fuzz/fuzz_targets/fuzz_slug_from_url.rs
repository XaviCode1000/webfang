#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz slug generation from URL paths — used for Obsidian file naming.
// Processes untrusted URLs. Panic = DoS during export.
fuzz_target!(|data: &[u8]| {
    if let Ok(path) = std::str::from_utf8(data) {
        // slug_from_url returns String, handles percent-encoding
        let slug = webfang::infrastructure::converter::wikilinks::slug_from_url(path);

        // Sanity: slug should not contain path separators (would create subdirs)
        assert!(!slug.contains('/'), "slug contains path separator: {}", slug);
        // Sanity: slug should not be empty for non-empty input
        if !path.is_empty() && slug.is_empty() {
            eprintln!("WARN: non-empty path produced empty slug: {:?}", path);
        }
    }
});

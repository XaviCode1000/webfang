#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mid = data.len() / 2;
    if let (Ok(html), Ok(base)) = (
        std::str::from_utf8(&data[..mid]),
        std::str::from_utf8(&data[mid..]),
    ) {
        let _ = rust_scraper::infrastructure::crawler::extract_links(html, base);
    }
});

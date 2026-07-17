#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz URL validation — RFC 3986 URL parsing on user-provided strings.
// Every URL from the internet passes through this. Panic = DoS.
fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // validate_and_parse_url returns Result — errors are expected for invalid URLs
        let _ = webfang::domain::url_validation::validate_and_parse_url(s);
    }
});

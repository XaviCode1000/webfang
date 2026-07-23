#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz Content-Disposition header parsing — processes untrusted HTTP headers.
// A panic here = DoS when receiving malformed Content-Disposition values.
fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // parse_content_disposition returns Option<String> — None means unparseable.
        // Must NEVER panic on any valid or invalid UTF-8 string.
        let _ = webfang::infrastructure::crawler::binary_utils::parse_content_disposition(s);
    }
});

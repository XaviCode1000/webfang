#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz WAF detection — scans HTTP response bodies for WAF signatures.
// Processes untrusted HTTP responses. Panic = DoS when hitting WAF-protected sites.
fuzz_target!(|data: &[u8]| {
    if let Ok(body) = std::str::from_utf8(data) {
        // detect_body returns Option<&str> — None means no WAF detected (normal)
        let _ = webfang::infrastructure::http::waf_engine::WafInspector::detect_body(body);
    }
});

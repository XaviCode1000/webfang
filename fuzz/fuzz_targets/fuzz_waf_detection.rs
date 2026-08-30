#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz WAF detection — scans HTTP response bodies for WAF signatures.
// Processes untrusted HTTP responses. Panic = DoS when hitting WAF-protected sites.
fuzz_target!(|data: &[u8]| {
    if let Ok(body) = std::str::from_utf8(data) {
        // Degraded-mode inspection (no HTTP context): the verdict is discarded,
        // we only exercise the scan path for panics. (`webfang` is the fuzz
        // crate's rename of `webfang_core`.)
        let ctx = webfang::domain::waf::InspectionContext::default();
        let _ = webfang::infrastructure::http::waf_engine::WafInspector::inspect(body, &ctx);
    }
});

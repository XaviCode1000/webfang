#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz compression detection — identifies gzip/deflate/brotli/zstd from magic bytes.
// Processes untrusted network bytes. Panic = DoS when receiving compressed responses.
fuzz_target!(|data: &[u8]| {
    if let Ok(url) = url::Url::parse("https://example.com/data") {
        // detect_compression returns Vec<CompressionType> — empty means unknown
        let _ = webfang::infrastructure::crawler::compression_handler::CompressionHandler::detect_compression(
            data, url.as_str(),
        );
    }
});

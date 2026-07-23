#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz decompression detection and decompression — processes untrusted network bytes.
// A panic here = DoS when receiving compressed responses from hostile servers.
fuzz_target!(|data: &[u8]| {
    // detect_and_decompress is async and requires a CompressionHandler instance.
    // Use a small max_decompressed_size to bound memory during fuzzing.
    let handler = webfang::infrastructure::crawler::compression_handler::CompressionHandler::with_max_size(
        1024 * 1024, // 1MB limit for fuzzing
    );

    // Run async decompression in a short-lived tokio runtime
    if let Ok(rt) = tokio::runtime::Runtime::new() {
        let _ = rt.block_on(async {
            handler
                .detect_and_decompress(data, "https://example.com/data.bin")
                .await
        });
    }
});

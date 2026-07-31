#![no_main]
use libfuzzer_sys::fuzz_target;

// Fuzz compression detection — identifies compression type of untrusted network bytes.
// A panic here = DoS when receiving compressed responses from hostile servers.
fuzz_target!(|data: &[u8]| {
    // detect_compression is a public static method that returns the detected
    // compression types. Errors are expected for non-compressed or invalid data.
    let _ = webfang::infrastructure::crawler::compression_handler::CompressionHandler::detect_compression(data, "https://example.com/data.bin");
});

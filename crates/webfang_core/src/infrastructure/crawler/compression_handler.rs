//! Compression Handler Module
//!
//! Multi-format compression detection and decompression for sitemap processing.
//! Supports gzip, deflate, brotli, and zstd formats with automatic detection.
//!
//! Detection policy (#757): **magic bytes are the sovereign truth** for
//! snifable formats (gzip, zstd). The URL extension is only a fallback hint
//! for formats that cannot be sniffed (brotli). A `.gz`/`.gzip`/`.zst` URL
//! whose payload has no magic bytes passes through untouched: either the HTTP
//! transport (`wreq`, built with `.gzip(true)`) already decoded the
//! `Content-Encoding` — and strips that header afterwards — or the extension
//! was lying. Decompressing anyway would double-decode valid XML.

use crate::domain::CompressionType;
use async_compression::tokio::bufread::{BrotliDecoder, DeflateDecoder, GzipDecoder, ZstdDecoder};
use std::io::Cursor;
use tokio::io::{AsyncReadExt, BufReader};

/// Errors that can occur during compression handling
#[derive(Debug, thiserror::Error)]
pub(crate) enum CompressionError {
    #[error("unsupported compression format: {0}")]
    #[allow(dead_code)] // pub(crate) Phase 0 triage — internal API surface
    UnsupportedFormat(String),
    #[error("decompression failed: {0}")]
    DecompressionFailed(String),
    #[error("size limit exceeded: {0} bytes")]
    SizeLimitExceeded(usize),
}

/// Result type for compression operations
pub(crate) type Result<T> = std::result::Result<T, CompressionError>;

/// Handles multi-format compression detection and decompression
pub struct CompressionHandler {
    max_decompressed_size: usize,
}

impl CompressionHandler {
    /// Create new compression handler with default settings
    pub fn new() -> Self {
        Self {
            max_decompressed_size: 100 * 1024 * 1024, // 100MB
        }
    }

    /// Create compression handler with custom max decompressed size
    pub fn with_max_size(max_decompressed_size: usize) -> Self {
        Self {
            max_decompressed_size,
        }
    }

    /// Detect compression format from content and URL
    ///
    /// Magic bytes are the sovereign truth for snifable formats (gzip, zstd):
    /// if the payload carries them, the matching decoder is returned. The URL
    /// extension is only a fallback hint for formats that cannot be sniffed
    /// (brotli).
    ///
    /// A `.gz`/`.gzip`/`.zst` URL whose payload has no magic bytes produces an
    /// **empty** result (pass-through): either the HTTP transport (`wreq`)
    /// already decoded the `Content-Encoding` — it strips that header after
    /// decoding — or the extension was lying. Decompressing anyway would
    /// double-decode valid XML (#757).
    pub fn detect_compression(content: &[u8], url: &str) -> Vec<CompressionType> {
        let mut formats = Vec::new();

        // 1. Magic bytes: sovereign truth for snifable formats.
        if content.len() >= 2 {
            // Gzip magic: 0x1f 0x8b
            if content[0] == 0x1f && content[1] == 0x8b {
                formats.push(CompressionType::Gzip);
            }
            // Zstd magic: 0x28 0xb5 0x2f 0xfd or 0x37 0xa4 0x30 0xec
            if content.len() >= 4
                && ((content[0] == 0x28
                    && content[1] == 0xb5
                    && content[2] == 0x2f
                    && content[3] == 0xfd)
                    || (content[0] == 0x37
                        && content[1] == 0xa4
                        && content[2] == 0x30
                        && content[3] == 0xec))
            {
                formats.push(CompressionType::Zstd);
            }
        }

        // 2. Extension as hint ONLY when magic bytes did not decide.
        let url_lower = url.to_lowercase();
        if formats.is_empty() && url_lower.ends_with(".br") {
            formats.push(CompressionType::Brotli);
        } else if !formats.is_empty() {
            tracing::debug!(
                url = %url,
                detected = ?formats,
                "compression detected by magic bytes"
            );
        } else if url_lower.ends_with(".gz")
            || url_lower.ends_with(".gzip")
            || url_lower.ends_with(".zst")
        {
            // #757 guard: transport already decoded or lying extension —
            // pass through untouched.
            tracing::debug!(
                url = %url,
                "compression hint ignored: no magic bytes (body already decompressed by transport)"
            );
        }

        formats
    }

    /// Detect compression format and decompress content
    pub(crate) async fn detect_and_decompress(&self, content: &[u8], url: &str) -> Result<Vec<u8>> {
        let formats = Self::detect_compression(content, url);

        if formats.is_empty() {
            // No compression detected, return as-is
            return Ok(content.to_vec());
        }

        // Try each detected format in order - fail closed on error
        for format in formats {
            match format {
                CompressionType::Gzip => {
                    return self.decompress_gzip(content).await;
                },
                CompressionType::Deflate => {
                    return self.decompress_deflate(content).await;
                },
                CompressionType::Brotli => {
                    return self.decompress_brotli(content).await;
                },
                CompressionType::Zstd => {
                    return self.decompress_zstd(content).await;
                },
                CompressionType::None => {},
            }
        }

        // No supported compression format found
        Ok(content.to_vec())
    }

    async fn decompress_gzip(&self, content: &[u8]) -> Result<Vec<u8>> {
        let reader = BufReader::new(Cursor::new(content));
        let decoder = GzipDecoder::new(reader);
        let mut decompressed = Vec::new();

        let mut limited = decoder.take(self.max_decompressed_size as u64);
        limited
            .read_to_end(&mut decompressed)
            .await
            .map_err(|e| CompressionError::DecompressionFailed(e.to_string()))?;

        if decompressed.len() >= self.max_decompressed_size {
            return Err(CompressionError::SizeLimitExceeded(
                self.max_decompressed_size,
            ));
        }

        Ok(decompressed)
    }

    async fn decompress_deflate(&self, content: &[u8]) -> Result<Vec<u8>> {
        let reader = BufReader::new(Cursor::new(content));
        let decoder = DeflateDecoder::new(reader);
        let mut decompressed = Vec::new();

        let mut limited = decoder.take(self.max_decompressed_size as u64);
        limited
            .read_to_end(&mut decompressed)
            .await
            .map_err(|e| CompressionError::DecompressionFailed(e.to_string()))?;

        if decompressed.len() >= self.max_decompressed_size {
            return Err(CompressionError::SizeLimitExceeded(
                self.max_decompressed_size,
            ));
        }

        Ok(decompressed)
    }

    async fn decompress_brotli(&self, content: &[u8]) -> Result<Vec<u8>> {
        let reader = BufReader::new(Cursor::new(content));
        let decoder = BrotliDecoder::new(reader);
        let mut decompressed = Vec::new();

        let mut limited = decoder.take(self.max_decompressed_size as u64);
        limited
            .read_to_end(&mut decompressed)
            .await
            .map_err(|e| CompressionError::DecompressionFailed(e.to_string()))?;

        if decompressed.len() >= self.max_decompressed_size {
            return Err(CompressionError::SizeLimitExceeded(
                self.max_decompressed_size,
            ));
        }

        Ok(decompressed)
    }

    async fn decompress_zstd(&self, content: &[u8]) -> Result<Vec<u8>> {
        let reader = BufReader::new(Cursor::new(content));
        let decoder = ZstdDecoder::new(reader);
        let mut decompressed = Vec::new();

        let mut limited = decoder.take(self.max_decompressed_size as u64);
        limited
            .read_to_end(&mut decompressed)
            .await
            .map_err(|e| CompressionError::DecompressionFailed(e.to_string()))?;

        if decompressed.len() >= self.max_decompressed_size {
            return Err(CompressionError::SizeLimitExceeded(
                self.max_decompressed_size,
            ));
        }

        Ok(decompressed)
    }
}

impl Default for CompressionHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_gzip_by_extension_is_now_hint() {
        // #757: a `.gz` URL whose payload lacks gzip magic bytes must NOT be
        // decompressed — transport already decoded it (or the URL lies).
        let url = "https://example.com/sitemap.xml.gz";

        // CASE C: plain content behind a .gz URL -> pass-through (empty).
        let plain = b"<?xml version=\"1.0\"?><urlset/>";
        let formats = CompressionHandler::detect_compression(plain, url);
        assert!(
            formats.is_empty(),
            "plain content with .gz URL must pass through, got: {formats:?}"
        );

        // CASE B: real gzip magic behind a .gz URL -> still detected via magic
        // bytes, regardless of the extension hint.
        let gzip_content = [0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00];
        let formats = CompressionHandler::detect_compression(&gzip_content, url);
        assert_eq!(formats, vec![CompressionType::Gzip]);
    }

    #[test]
    fn test_detect_gzip_by_magic() {
        let url = "https://example.com/sitemap.xml";
        let content = &[0x1f, 0x8b, b'f', b'a', b'k', b'e'];
        let formats = CompressionHandler::detect_compression(content, url);
        assert!(formats.contains(&CompressionType::Gzip));
    }

    #[test]
    fn test_detect_zstd_by_magic() {
        // Zstd magic wins even for a misleading non-compressed URL.
        let url = "https://example.com/data.bin";
        let content = [0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x00];
        let formats = CompressionHandler::detect_compression(&content, url);
        assert_eq!(formats, vec![CompressionType::Zstd]);
    }

    #[test]
    fn test_detect_zstd_extension_without_magic_passes_through() {
        // #757 guard applies to .zst too: no magic bytes -> no decompression.
        let url = "https://example.com/sitemap.xml.zst";
        let plain = b"<urlset/>";
        let formats = CompressionHandler::detect_compression(plain, url);
        assert!(formats.is_empty());
    }

    #[test]
    fn test_detect_brotli_extension_hint_when_not_snifable() {
        // Brotli has no magic bytes; the .br extension remains the hint and
        // stays fail-closed (decoding errors propagate).
        let url = "https://example.com/sitemap.xml.br";
        let content = b"brotli has no magic prefix";
        let formats = CompressionHandler::detect_compression(content, url);
        assert_eq!(formats, vec![CompressionType::Brotli]);
    }

    #[test]
    fn test_detect_compression_uppercase_extension() {
        // Extension matching is case-insensitive.
        let url = "https://example.com/sitemap.xml.BR";
        let content = b"payload";
        let formats = CompressionHandler::detect_compression(content, url);
        assert_eq!(formats, vec![CompressionType::Brotli]);
    }

    #[tokio::test]
    async fn test_detect_and_decompress_uncompressed() {
        let handler = CompressionHandler::new();
        let content = b"<xml>test</xml>";

        let result = handler
            .detect_and_decompress(content, "https://example.com/sitemap.xml")
            .await;
        assert!(result.is_ok());
        let decompressed = result.unwrap();
        assert_eq!(decompressed, content);
    }

    #[test]
    fn test_detect_multiple_formats() {
        let url = "https://example.com/sitemap.xml.gz";
        let content = &[0x1f, 0x8b, b'g', b'z', b'i', b'p']; // Gzip magic
        let formats = CompressionHandler::detect_compression(content, url);
        assert_eq!(formats.len(), 1); // Should only include Gzip once
        assert!(formats.contains(&CompressionType::Gzip));
    }

    #[test]
    fn test_detect_no_compression() {
        let url = "https://example.com/sitemap.xml";
        let content = b"<xml>no compression</xml>";
        let formats = CompressionHandler::detect_compression(content, url);
        assert!(formats.is_empty());
    }
}

//! Compression Handler Module
//!
//! Multi-format compression detection and decompression for sitemap processing.
//! Supports gzip, deflate, brotli, and zstd formats with automatic detection.

use crate::domain::CompressionType;
use async_compression::tokio::bufread::{BrotliDecoder, DeflateDecoder, GzipDecoder, ZstdDecoder};
use std::io::Cursor;
use tokio::io::{AsyncReadExt, BufReader};

/// Errors that can occur during compression handling
#[derive(Debug, thiserror::Error)]
pub enum CompressionError {
    #[error("unsupported compression format: {0}")]
    UnsupportedFormat(String),
    #[error("decompression failed: {0}")]
    DecompressionFailed(String),
    #[error("size limit exceeded: {0} bytes")]
    SizeLimitExceeded(usize),
}

/// Result type for compression operations
pub type Result<T> = std::result::Result<T, CompressionError>;

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
    pub fn detect_compression(content: &[u8], url: &str) -> Vec<CompressionType> {
        let mut formats = Vec::new();

        // Check URL extensions
        if url.ends_with(".gz") || url.ends_with(".gzip") {
            formats.push(CompressionType::Gzip);
        }
        if url.ends_with(".br") {
            formats.push(CompressionType::Brotli);
        }
        if url.ends_with(".zst") {
            formats.push(CompressionType::Zstd);
        }

        // Check content signatures
        if content.len() >= 2 {
            // Gzip magic: 0x1f 0x8b
            if content[0] == 0x1f && content[1] == 0x8b && !formats.contains(&CompressionType::Gzip)
            {
                formats.push(CompressionType::Gzip);
            }
            // Zstd magic: 0x28 0xb5 0x2f 0xfd or 0x37 0xa4 0x30 0xec
            if content.len() >= 4
                && !formats.contains(&CompressionType::Zstd)
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

        formats
    }

    /// Detect compression format and decompress content
    pub async fn detect_and_decompress(&self, content: &[u8], url: &str) -> Result<Vec<u8>> {
        let formats = Self::detect_compression(content, url);

        if formats.is_empty() {
            // No compression detected, return as-is
            return Ok(content.to_vec());
        }

        // Try each detected format in order
        for format in formats {
            match format {
                CompressionType::Gzip => {
                    if let Ok(decompressed) = self.decompress_gzip(content).await {
                        return Ok(decompressed);
                    }
                },
                CompressionType::Deflate => {
                    if let Ok(decompressed) = self.decompress_deflate(content).await {
                        return Ok(decompressed);
                    }
                },
                CompressionType::Brotli => {
                    if let Ok(decompressed) = self.decompress_brotli(content).await {
                        return Ok(decompressed);
                    }
                },
                CompressionType::Zstd => {
                    if let Ok(decompressed) = self.decompress_zstd(content).await {
                        return Ok(decompressed);
                    }
                },
                CompressionType::None => {},
            }
        }

        // If no format worked, return original content
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
    fn test_detect_gzip_by_extension() {
        let url = "https://example.com/sitemap.xml.gz";
        let content = b"fake gzip content";
        let formats = CompressionHandler::detect_compression(content, url);
        assert!(formats.contains(&CompressionType::Gzip));
    }

    #[test]
    fn test_detect_gzip_by_magic() {
        let url = "https://example.com/sitemap.xml";
        let content = &[0x1f, 0x8b, b'f', b'a', b'k', b'e'];
        let formats = CompressionHandler::detect_compression(content, url);
        assert!(formats.contains(&CompressionType::Gzip));
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

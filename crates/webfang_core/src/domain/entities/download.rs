//! Downloaded asset entity

use serde::{Deserialize, Serialize};

/// Represents a downloaded asset (image or document)
///
/// Contains metadata about the original URL and local storage location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadedAsset {
    /// Original URL of the asset
    pub url: String,
    /// Local path where asset was saved
    pub local_path: String,
    /// Asset type: "image" or "document"
    pub asset_type: String,
    /// File size in bytes
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_downloaded_asset_creation() {
        let asset = DownloadedAsset {
            url: "https://example.com/image.png".to_string(),
            local_path: "/tmp/image.png".to_string(),
            asset_type: "image".to_string(),
            size: 1024,
        };

        assert_eq!(asset.url, "https://example.com/image.png");
        assert_eq!(asset.size, 1024);
    }

    #[test]
    fn test_downloaded_asset_serde_roundtrip() {
        let asset = DownloadedAsset {
            url: "https://example.com/img.png".to_string(),
            local_path: "/tmp/img.png".to_string(),
            asset_type: "image".to_string(),
            size: 2048,
        };
        let json = serde_json::to_string(&asset).unwrap();
        let deserialized: DownloadedAsset = serde_json::from_str(&json).unwrap();
        assert_eq!(asset, deserialized);
    }
}

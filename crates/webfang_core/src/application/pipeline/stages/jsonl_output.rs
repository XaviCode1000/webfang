//! JSONL output stage — writes each [`ScrapedItem`] as a JSON line to a file.

use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;

use crate::application::pipeline::stages::output::{OutputError, OutputStage};
use crate::domain::pipeline_item::ScrapedItem;

/// Output stage that writes [`ScrapedItem`]s as JSONL (JSON Lines) to a file.
///
/// Each item is serialized as a single JSON object on its own line. The file is
/// opened in append mode so existing content is preserved.
pub struct JsonlOutputStage {
    path: PathBuf,
    file: Mutex<std::fs::File>,
}

impl JsonlOutputStage {
    /// Create a new JSONL output stage writing to `path`.
    ///
    /// The file is created if it doesn't exist, or opened for append if it does.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, OutputError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.as_ref())?;
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            file: Mutex::new(file),
        })
    }

    /// Returns the output file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl OutputStage for JsonlOutputStage {
    fn name(&self) -> &str {
        "jsonl_output"
    }

    fn write<'a>(
        &'a self,
        item: &'a ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutputError>> + Send + 'a>> {
        Box::pin(async {
            let json = jzon_serde::to_string(item)
                .map_err(|e| OutputError::Serialization(e.to_string()))?;
            let mut file = self
                .file
                .lock()
                .map_err(|e| OutputError::Backend(e.to_string()))?;
            writeln!(file, "{json}").map_err(|e| OutputError::Backend(e.to_string()))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pipeline_item::ScrapedItem;
    use std::io::{BufRead, BufReader};

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("webfang_jsonl_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}.jsonl"))
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    fn make_item(url: &str) -> ScrapedItem {
        ScrapedItem {
            url: url.into(),
            raw_html: "<p>test</p>".into(),
            text_content: Some("test content".into()),
            status_code: 200,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_writes_valid_json_line() {
        let path = temp_path("valid_json");
        let stage = JsonlOutputStage::new(&path).unwrap();
        let item = make_item("https://example.com");

        stage.write(&item).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let line = content.trim();
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["url"].as_str().unwrap(), "https://example.com");
        assert_eq!(parsed["status_code"].as_u64().unwrap(), 200);
        cleanup(&path);
    }

    #[tokio::test]
    async fn test_appends_not_overwrites() {
        let path = temp_path("append_test");
        let item1 = make_item("https://first.com");
        let item2 = make_item("https://second.com");

        {
            let stage = JsonlOutputStage::new(&path).unwrap();
            stage.write(&item1).await.unwrap();
        }
        {
            let stage = JsonlOutputStage::new(&path).unwrap();
            stage.write(&item2).await.unwrap();
        }

        let file = std::fs::File::open(&path).unwrap();
        let lines: Vec<String> = BufReader::new(file).lines().map(|l| l.unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("first.com"));
        assert!(lines[1].contains("second.com"));
        cleanup(&path);
    }

    #[tokio::test]
    async fn test_writes_multiple_items() {
        let path = temp_path("multi_items");
        let stage = JsonlOutputStage::new(&path).unwrap();

        for i in 0..5 {
            let item = make_item(&format!("https://example{i}.com"));
            stage.write(&item).await.unwrap();
        }

        let content = std::fs::read_to_string(&path).unwrap();
        let line_count = content.lines().count();
        assert_eq!(line_count, 5);

        for i in 0..5 {
            assert!(content.contains(&format!("example{i}.com")));
        }
        cleanup(&path);
    }

    #[test]
    fn test_stage_name() {
        let path = temp_path("name_test");
        let stage = JsonlOutputStage::new(&path).unwrap();
        assert_eq!(stage.name(), "jsonl_output");
        cleanup(&path);
    }

    #[test]
    fn test_path_accessor() {
        let path = temp_path("path_test");
        let stage = JsonlOutputStage::new(&path).unwrap();
        assert_eq!(stage.path(), path);
        cleanup(&path);
    }
}

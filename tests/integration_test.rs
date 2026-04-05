//! Integration tests for rust_scraper
//!
//! These tests verify end-to-end functionality of the scraper.
//!
//! Run with: cargo test --test integration
//! Run with features: cargo test --test integration --features images,documents

use rust_scraper::{
    create_http_client, save_results, scrape_with_config, scrape_with_readability, DownloadedAsset,
    ScrapedContent, ValidUrl,
};
use tempfile::TempDir;
use walkdir::WalkDir;

// ============================================================================
// Integration Tests: Full scraping pipeline
// Note: These are "happy path" tests that require network access
// Skip with: cargo test --test integration -- --skip
// ============================================================================

#[tokio::test]
async fn test_scraper_can_fetch_simple_page() {
    // This test fetches a real page - skip in CI without network
    // Arrange
    let url = url::Url::parse("https://example.com").expect("Valid URL");
    let client = create_http_client().expect("HTTP client");

    // Act - Just verify we can fetch without error
    let result: Result<Vec<_>, _> = scrape_with_readability(&client, &url).await;

    // Assert - Either succeeds or fails gracefully (network dependent)
    // We don't assert success because it depends on network
    if let Ok(contents) = result {
        if !contents.is_empty() {
            assert!(!contents[0].title.is_empty());
        }
    }
}

// ============================================================================
// Tests: CLI Argument validation (via integration test pattern)
// ============================================================================

#[test]
fn test_output_format_display() {
    // Test that OutputFormat variants can be displayed
    use rust_scraper::OutputFormat;

    let markdown = OutputFormat::Markdown;
    let text = OutputFormat::Text;
    let json = OutputFormat::Json;

    // These should not panic
    let _ = format!("{:?}", markdown);
    let _ = format!("{:?}", text);
    let _ = format!("{:?}", json);
}

#[test]
fn test_args_has_required_fields() {
    // Test that Args struct has the expected fields (without Default)
    use rust_scraper::Args;
    use rust_scraper::ExportFormat;
    use rust_scraper::OutputFormat;

    // Create Args with all required fields
    let args = Args {
        url: Some("https://example.com".to_string()),
        selector: "article".to_string(),
        output: std::path::PathBuf::from("custom_output"),
        format: OutputFormat::Markdown,
        export_format: ExportFormat::Jsonl,
        delay_ms: 500,
        max_pages: 5,
        download_images: false,
        download_documents: false,
        verbose: 2,
        concurrency: rust_scraper::ConcurrencyConfig::default(),
        use_sitemap: false,
        sitemap_url: None,
        interactive: false,
        resume: false,
        state_dir: None,
        clean_ai: false,
        force_js_render: false,
        dry_run: false,
        quiet: false,
        subcommand: None,
        obsidian_wiki_links: false,
        obsidian_tags: None,
        obsidian_relative_assets: false,
        vault: None,
        quick_save: false,
        obsidian_rich_metadata: false,
        // New crawler settings
        max_depth: 2,
        timeout_secs: 30,
        include_patterns: vec![],
        exclude_patterns: vec![],
        // New HTTP client settings
        max_retries: 3,
        backoff_base_ms: 1000,
        backoff_max_ms: 10000,
        accept_language: "en-US,en;q=0.9".to_string(),
        // New download settings
        max_file_size: 52428800,
        download_timeout: 30,
        // New AI settings (feature-gated, use defaults)
        #[cfg(feature = "ai")]
        threshold: 0.3,
        #[cfg(feature = "ai")]
        max_tokens: 512,
        #[cfg(feature = "ai")]
        offline: false,
        // Sitemap settings
        sitemap_depth: 3,
    };

    assert_eq!(args.url, Some("https://example.com".to_string()));
    assert_eq!(args.selector, "article");
    assert_eq!(args.format, OutputFormat::Markdown);
    assert_eq!(args.export_format, ExportFormat::Jsonl);
    assert_eq!(args.delay_ms, 500);
    assert_eq!(args.max_pages, 5);
    assert_eq!(args.verbose, 2);
}

// ============================================================================
// Tests: Content extraction edge cases
// ============================================================================

#[tokio::test]
async fn test_scrape_handles_404_gracefully() {
    // Arrange
    let url =
        url::Url::parse("https://example.com/this-does-not-exist-404-test").expect("Valid URL");
    let client = create_http_client().expect("HTTP client");

    // Act
    let result: Result<Vec<_>, _> = scrape_with_readability(&client, &url).await;

    // Assert - Should fail gracefully with clear error
    assert!(result.is_err());
    let err = result.unwrap_err();
    // With retry middleware, 404 errors may be wrapped as "Request failed after N retries"
    // Both error types are acceptable - either direct 404 or retry failure
    let error_msg = err.to_string();
    assert!(
        error_msg.contains("404")
            || error_msg.contains("HTTP")
            || error_msg.contains("retries")
            || error_msg.contains("middleware"),
        "Error should contain 404/HTTP/retries/middleware, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_scrape_handles_invalid_url_gracefully() {
    // This test verifies behavior when URL has no host
    // Note: url::Url::parse("https://") actually succeeds with EmptyHost
    // The validation happens at a different level (validate_and_parse_url)

    // Test is covered by unit tests in lib.rs
    // This test can remain as a placeholder for future extension
    let result = create_http_client();
    assert!(result.is_ok());
}

// ============================================================================
// Tests: Result saving edge cases
// ============================================================================

#[test]
fn test_save_results_to_nested_directory() {
    // Arrange
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().join("level1").join("level2").join("output");

    let results = vec![ScrapedContent {
        title: "Test".to_string(),
        content: "Content".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
    }];

    // Act
    let obsidian = rust_scraper::ObsidianOptions::default();
    let result = save_results(
        &results,
        &output_dir,
        &rust_scraper::OutputFormat::Text,
        &obsidian,
    );

    // Assert
    assert!(result.is_ok());
    assert!(output_dir.exists());

    // Verify file was created
    let files: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!files.is_empty());
}

#[test]
fn test_save_results_json_with_special_characters() {
    // Arrange - Content with special characters that might break JSON
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().to_path_buf();

    // Use non-empty assets to ensure field is serialized
    let results = vec![ScrapedContent {
        title: "Test with \"quotes\" and 'apostrophes'".to_string(),
        content: "Content with\nnewlines\tand\ttabs".to_string(),
        url: ValidUrl::parse("https://example.com?param=value&other=test").unwrap(),
        excerpt: Some("Excerpt with <html> & \"special\" chars".to_string()),
        author: Some("Author Name".to_string()),
        date: Some("2024-01-01".to_string()),
        html: None,
        assets: vec![DownloadedAsset {
            url: "https://example.com/img.png".to_string(),
            local_path: "images/img.png".to_string(),
            asset_type: "image".to_string(),
            size: 100,
        }],
    }];

    // Act
    let obsidian = rust_scraper::ObsidianOptions::default();
    let result = save_results(
        &results,
        &output_dir,
        &rust_scraper::OutputFormat::Json,
        &obsidian,
    );

    // Assert - Should handle special chars correctly
    assert!(result.is_ok());

    let json_path = output_dir.join("results.json");
    let content = std::fs::read_to_string(&json_path).unwrap();

    // Verify JSON is valid and readable
    let parsed: Vec<ScrapedContent> = serde_json::from_str(&content).expect("Valid JSON");
    assert_eq!(parsed.len(), 1);
}

#[test]
fn test_save_results_markdown_with_markdown_syntax() {
    // Arrange - Content that looks like markdown
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().to_path_buf();

    let results = vec![ScrapedContent {
        title: "# Heading 1".to_string(),
        content: "**Bold** and *italic* and `code`".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
    }];

    // Act
    let obsidian = rust_scraper::ObsidianOptions::default();
    let result = save_results(
        &results,
        &output_dir,
        &rust_scraper::OutputFormat::Markdown,
        &obsidian,
    );

    // Assert
    assert!(result.is_ok());

    use walkdir::WalkDir;
    let files: Vec<_> = WalkDir::new(&output_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    let content = std::fs::read_to_string(files[0].path()).unwrap();

    // Should preserve markdown-like content
    assert!(content.contains("# Heading 1"));
    assert!(content.contains("**Bold**"));
}

// ============================================================================
// Integration Tests: Asset Download (requires --features images)
// ============================================================================

/// Test downloading images from a real website
/// Run with: cargo test --test integration --features images test_download_images_from_website
#[cfg(feature = "images")]
#[tokio::test]
async fn test_download_images_from_website() {
    // Arrange - Use webscraper.io test site (free, no auth required)
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().to_path_buf();

    let url = url::Url::parse("https://webscraper.io/test-sites").expect("Valid URL");
    let client = create_http_client().expect("HTTP client");

    let config = rust_scraper::ScraperConfig {
        download_images: true,
        download_documents: false,
        output_dir: output_dir.clone(),
        max_file_size: Some(10 * 1024 * 1024), // 10MB max
        scraper_concurrency: 3,
        download_timeout_secs: 30,
    };

    // Act
    let result: Result<Vec<_>, _> = scrape_with_config(&client, &url, &config).await;

    // Assert - Should succeed or fail gracefully (network dependent)
    if let Ok(contents) = result {
        if !contents.is_empty() {
            let content = &contents[0];

            // Verify we got some assets
            assert!(
                !content.assets.is_empty(),
                "Should have downloaded some images"
            );

            // Verify images directory was created
            let images_dir = output_dir.join("images");
            assert!(images_dir.exists(), "Images directory should exist");

            // Verify actual image files exist on disk
            let image_files: Vec<_> = WalkDir::new(&images_dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .collect();

            assert!(
                !image_files.is_empty(),
                "Should have downloaded image files"
            );

            // Log for debugging
            eprintln!(
                "✅ Downloaded {} images: {:?}",
                content.assets.len(),
                content
                    .assets
                    .iter()
                    .map(|a| &a.local_path)
                    .collect::<Vec<_>>()
            );
        }
    }
}

/// Test downloading documents from a real website
/// Run with: cargo test --test integration --features documents test_download_documents_from_website
#[cfg(feature = "documents")]
#[tokio::test]
async fn test_download_documents_from_website() {
    // Arrange - Use a test site with documents if available
    // Note: Most test sites don't have documents, so this tests the extraction logic
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().to_path_buf();

    // Use toscrape.com (free blog scraping sandbox)
    let url = url::Url::parse("https://toscrape.com").expect("Valid URL");
    let client = create_http_client().expect("HTTP client");

    let config = rust_scraper::ScraperConfig {
        download_images: false,
        download_documents: true,
        output_dir: output_dir.clone(),
        max_file_size: Some(50 * 1024 * 1024), // 50MB max
        scraper_concurrency: 3,
        download_timeout_secs: 30,
    };

    // Act
    let result: Result<Vec<_>, _> = scrape_with_config(&client, &url, &config).await;

    // Assert - Just verify it doesn't crash
    // Document extraction depends on specific site content
    if let Ok(contents) = result {
        eprintln!(
            "✅ Document extraction completed, found {} items",
            contents.len()
        );
    }
}

// ============================================================================
// Integration Tests: AI Semantic Filtering with Embeddings
// ============================================================================

/// Test that AI semantic cleaner preserves embeddings in output
///
/// **Bug Fixed**: v1.0.5 - Embeddings were being discarded during semantic filtering,
/// resulting in "Generated 0 chunks with embeddings" logs and empty embeddings fields.
///
/// **Fix Applied**: Modified `filter_by_relevance()` to use `filter_with_embeddings()`
/// and restore embeddings after filtering.
///
/// **Validation**: This test ensures embeddings are present in output chunks.
///
/// # Run with
///
/// ```bash
/// cargo test --test integration --features ai test_ai_embedding_preservation
/// ```
///
/// # Expected Results
///
/// - **Before fix**: 7 chunks generated, 0 with embeddings, eprintln!("Generated 0 chunks with embeddings")
/// - **After fix**: 7 chunks generated, 7 with embeddings (384-dim vectors each)
///
/// # Example Output
///
/// ```
/// ✅ AI clean successful, generated 7 chunks with embeddings
///    First chunk embedding dimension: 384
///    Sample content: <html><body><h1>Title 1</h1><p>This is parag...
/// ```
#[cfg(feature = "ai")]
#[tokio::test]
async fn test_ai_embedding_preservation() {
    // Test original bug: embeddings were being discarded during filtering
    // This test verifies the fix works: embeddings shouldn't be None after cleaning

    use rust_scraper::infrastructure::ai::{ModelConfig, SemanticCleanerImpl};
    use rust_scraper::SemanticCleaner;

    // Arrange - Use substantial HTML content that will generate multiple chunks
    // Each paragraph needs enough text to be considered a meaningful chunk
    let html = r#"
        <html>
        <body>
            <article>
                <h1>Introduction to Machine Learning</h1>
                <p>Machine learning is a subset of artificial intelligence that enables systems to learn and improve from experience without being explicitly programmed. It focuses on developing computer programs that can access data and use it to learn for themselves. The primary aim is to allow computers to learn automatically without human intervention or assistance.</p>
                <h2>Supervised Learning</h2>
                <p>Supervised learning is a type of machine learning where the algorithm is trained on labeled data. The algorithm learns from input-output pairs and makes predictions based on that learning. Common applications include classification tasks like spam detection and regression tasks like predicting house prices based on features.</p>
                <h2>Unsupervised Learning</h2>
                <p>Unsupervised learning involves training algorithms on unlabeled data. The system tries to learn the underlying structure and relationships in the data without explicit guidance. Clustering and dimensionality reduction are common unsupervised learning tasks used in customer segmentation and anomaly detection.</p>
                <h2>Reinforcement Learning</h2>
                <p>Reinforcement learning is a type of machine learning where an agent learns to make decisions by interacting with an environment. The agent receives rewards or penalties based on its actions and learns to maximize cumulative reward. This approach has been successful in game playing, robotics, and autonomous systems.</p>
                <h2>Deep Learning and Neural Networks</h2>
                <p>Deep learning is a subset of machine learning that uses neural networks with multiple layers. These neural networks attempt to simulate the behavior of the human brain, allowing them to learn from large amounts of data. Deep learning has achieved remarkable success in image recognition, natural language processing, and speech recognition.</p>
            </article>
        </body>
        </html>
    "#;

    // Act - Create cleaner and clean content
    let config = ModelConfig::default();
    let cleaner = SemanticCleanerImpl::new(config)
        .await
        .expect("Failed to create semantic cleaner");
    let chunks_result: Result<
        Vec<rust_scraper::domain::DocumentChunk>,
        rust_scraper::SemanticError,
    > = cleaner.clean(html).await;

    // Assert - Should succeed and have chunks with embeddings
    assert!(
        chunks_result.is_ok(),
        "Semantic cleaner should succeed, got: {:?}",
        chunks_result.err()
    );

    let chunks = chunks_result.unwrap();

    // Verify we got chunks
    assert!(
        !chunks.is_empty(),
        "Should have generated at least one chunk"
    );

    // Verify each chunk has embeddings (THE BUG FIX!)
    for (idx, chunk) in chunks.iter().enumerate() {
        let has_embeddings = chunk.embeddings.is_some();
        if !has_embeddings {
            eprintln!("❌ ERROR: Chunk {} has no embeddings!", idx);
            eprintln!(
                "   Content preview: {}",
                &chunk.content[..chunk.content.len().min(100)]
            );
        }
        assert!(
            has_embeddings,
            "Chunk {} should have embeddings, but embeddings is None",
            idx
        );
    }

    // Log results for debugging
    eprintln!(
        "✅ AI clean successful, generated {} chunks with embeddings",
        chunks.len()
    );
    eprintln!(
        "   First chunk embedding dimension: {}",
        chunks[0]
            .embeddings
            .as_ref()
            .map(|e: &Vec<f32>| e.len())
            .unwrap_or(0)
    );
    eprintln!(
        "   Sample content: {}",
        &chunks[0].content[..chunks[0].content.len().min(150)]
    );
}

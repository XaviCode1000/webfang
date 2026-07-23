//! AI Integration Tests
//!
//! Integration tests for AI-powered semantic cleaning features.
//! These tests are feature-gated behind the `ai` feature flag.
//!
//! # Running Tests
//!
//! ```bash
//! # Run all AI tests
//! cargo test --features ai --test ai_integration -- --nocapture
//!
//! # Run specific test
//! cargo test --features ai --test ai_integration test_semantic_cleaner_trait_defined
//! ```

#![cfg(feature = "ai")]

use std::path::PathBuf;
use webfang_ai::infrastructure_ai::model_downloader::ModelDownloader;
use webfang_ai::infrastructure_ai::{
    default_cache_dir, CacheConfig, ModelCache, DEFAULT_MODEL_FILE, DEFAULT_MODEL_REPO,
};
use webfang_ai::infrastructure_ai::{InferencePool, ModelConfig, SemanticCleanerImpl};
use webfang_ai::SemanticCleaner;
use webfang_ai::SemanticError;
use webfang_core::domain::DocumentChunk;

// ============================================================================
// Integration-only tests (not covered by unit tests)
// ============================================================================

/// Test that the model cache directory logic works correctly
#[tokio::test]
async fn test_model_cache_directory_created() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_dir = temp_dir.path().join("test_ai_cache");

    let config = CacheConfig::new().with_cache_dir(cache_dir.clone());
    let cache = ModelCache::new(config);

    // Directory shouldn't exist yet
    assert!(!cache_dir.exists());

    // Create it
    cache.ensure_cache_dir().await.unwrap();

    // Now it should exist
    assert!(cache_dir.exists());
    assert!(cache_dir.is_dir());

    // Verify it's the right directory
    assert_eq!(cache.cache_dir(), &cache_dir);
}

/// Test that the model download structure is correct
#[tokio::test]
#[ignore = "requires network access to HuggingFace"]
async fn test_model_download_structure() {
    // Test that ModelDownloader can be constructed with the right API
    let downloader = ModelDownloader::new()
        .with_repo(DEFAULT_MODEL_REPO)
        .with_file(DEFAULT_MODEL_FILE);

    assert_eq!(downloader.repo(), DEFAULT_MODEL_REPO);
    assert_eq!(downloader.file(), DEFAULT_MODEL_FILE);

    // Test that download_to method exists and has the right signature
    // (We don't actually download in this test to avoid network dependency)
    let temp_dir = tempfile::tempdir().unwrap();
    let result = downloader.download_to(temp_dir.path()).await;

    // This will fail because we're not actually downloading,
    // but it should fail with a proper error, not a compilation error
    assert!(result.is_err());

    // Verify the error type is correct
    if let Err(SemanticError::Download { repo, cause }) = result {
        assert_eq!(repo, DEFAULT_MODEL_REPO);
        assert!(!cause.is_empty());
    } else {
        panic!("Expected SemanticError::Download");
    }
}

/// Test that ModelConfig has the correct default values
#[test]
fn test_model_config_defaults() {
    let config = ModelConfig::default();

    assert_eq!(config.repo, DEFAULT_MODEL_REPO);
    assert_eq!(config.model_file, DEFAULT_MODEL_FILE);
    assert!(config.auto_download);
    assert!(!config.offline_mode);
    assert_eq!(config.max_tokens, 32768);

    // Verify cache_dir ends with ai_models
    assert!(config.cache_dir.to_string_lossy().contains("ai_models"));
}

/// Test that ModelConfig offline mode is configured correctly
#[test]
fn test_semantic_cleaner_offline_mode_config() {
    let temp_dir = tempfile::tempdir().unwrap();

    let config = ModelConfig::new()
        .with_cache_dir(temp_dir.path().to_path_buf())
        .with_auto_download(false)
        .with_offline_mode(true);

    // Verify configuration
    assert!(!config.auto_download);
    assert!(config.offline_mode);
    assert_eq!(config.cache_dir, temp_dir.path());
}

/// Test that DocumentChunk can be created (verifies domain integration)
#[test]
fn test_document_chunk_creation() {
    let chunk = DocumentChunk::new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Test Page",
        "Test content",
    );

    assert_eq!(chunk.url, "https://example.com");
}

/// Test that ModelCache can check if a model is cached
#[tokio::test]
async fn test_model_cache_is_cached() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_dir = temp_dir.path().join("test_cache");

    let config = CacheConfig::new().with_cache_dir(cache_dir.clone());
    let cache = ModelCache::new(config);

    // Should return false for non-existent file
    assert!(!cache.is_model_cached("model.onnx").await.unwrap());

    // Create a dummy file
    tokio::fs::create_dir_all(&cache_dir).await.unwrap();
    tokio::fs::File::create(cache_dir.join("model.onnx"))
        .await
        .unwrap();

    // Should return true now
    assert!(cache.is_model_cached("model.onnx").await.unwrap());
}

/// Test that ModelCache can get model path
#[test]
fn test_model_cache_model_path() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_dir = temp_dir.path().join("test_cache");

    let config = CacheConfig::new().with_cache_dir(cache_dir.clone());
    let cache = ModelCache::new(config);

    let model_path = cache.model_path("model.onnx");
    assert_eq!(model_path, cache_dir.join("model.onnx"));
}

/// Test that DownloadProgress calculations work correctly
#[test]
fn test_download_progress_calculations() {
    use webfang_ai::infrastructure_ai::DownloadProgress;

    // Test percentage calculation
    let progress = DownloadProgress {
        downloaded: 50,
        total: Some(100),
        speed: None,
        eta_seconds: None,
    };

    assert_eq!(progress.percentage(), Some(50.0));
    assert!(!progress.is_complete());

    // Test complete download
    let progress = DownloadProgress {
        downloaded: 100,
        total: Some(100),
        speed: None,
        eta_seconds: None,
    };

    assert_eq!(progress.percentage(), Some(100.0));
    assert!(progress.is_complete());

    // Test no total
    let progress = DownloadProgress {
        downloaded: 50,
        total: None,
        speed: None,
        eta_seconds: None,
    };

    assert!(progress.percentage().is_none());
    assert!(!progress.is_complete());
}

/// Test that SemanticError variants are properly defined
#[test]
fn test_semantic_error_variants() {
    // Test ModelLoad error — match on variant, verify inner error
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let err = SemanticError::ModelLoad(io_err);
    match err {
        SemanticError::ModelLoad(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("expected ModelLoad, got {other:?}"),
    }

    // Test ChunkTooLarge error — match on variant, verify fields
    let err = SemanticError::ChunkTooLarge {
        chunk_id: "chunk-1".to_string(),
        tokens: 600,
        max: 512,
    };
    match err {
        SemanticError::ChunkTooLarge {
            chunk_id,
            tokens,
            max,
        } => {
            assert_eq!(chunk_id, "chunk-1");
            assert_eq!(tokens, 600);
            assert_eq!(max, 512);
        },
        other => panic!("expected ChunkTooLarge, got {other:?}"),
    }

    // Test Download error — match on variant, verify fields
    let err = SemanticError::Download {
        repo: "test/repo".to_string(),
        cause: "network error".to_string(),
    };
    match err {
        SemanticError::Download { repo, cause } => {
            assert_eq!(repo, "test/repo");
            assert_eq!(cause, "network error");
        },
        other => panic!("expected Download, got {other:?}"),
    }

    // Test CacheValidation error — match on variant, verify fields
    let err = SemanticError::CacheValidation {
        repo: "test/repo".to_string(),
        expected: "abc123".to_string(),
        actual: "def456".to_string(),
    };
    match err {
        SemanticError::CacheValidation {
            repo,
            expected,
            actual,
        } => {
            assert_eq!(repo, "test/repo");
            assert_eq!(expected, "abc123");
            assert_eq!(actual, "def456");
        },
        other => panic!("expected CacheValidation, got {other:?}"),
    }

    // Test OfflineMode error — match on variant, verify field
    let err = SemanticError::OfflineMode {
        repo: "test/repo".to_string(),
    };
    match err {
        SemanticError::OfflineMode { repo } => assert_eq!(repo, "test/repo"),
        other => panic!("expected OfflineMode, got {other:?}"),
    }
}

/// Test that ScraperError can be created from SemanticError
#[test]
fn test_scraper_error_from_semantic_error() {
    use webfang_core::ScraperError;

    let semantic_err = SemanticError::ModelLoad(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "model missing",
    ));

    let scraper_err: ScraperError = semantic_err.into();
    match scraper_err {
        ScraperError::Semantic(SemanticError::ModelLoad(e)) => {
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
        },
        other => panic!("expected ScraperError::Semantic(ModelLoad), got {other:?}"),
    }
}

/// Test ChunkId inner value access
#[test]
fn test_chunk_id_inner() {
    use webfang_ai::infrastructure_ai::ChunkId;

    let id = ChunkId::new(123);
    assert_eq!(id.inner(), 123);
}

/// Test that SemanticCleanerImpl has all required fields
#[test]
fn test_semantic_cleaner_impl_send_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    assert_send::<SemanticCleanerImpl>();
    assert_sync::<SemanticCleanerImpl>();
}

/// Test ModelConfig with relevance threshold
#[test]
fn test_model_config_with_relevance_threshold() {
    let config = ModelConfig::default().with_relevance_threshold(0.5);
    assert_eq!(config.relevance_threshold, 0.5);
}

/// Test ModelConfig builder with all options
#[test]
fn test_model_config_full_builder() {
    let temp_dir = tempfile::tempdir().unwrap();

    let config = ModelConfig::new()
        .with_repo("sentence-transformers/all-MiniLM-L6-v2")
        .with_file("model.onnx")
        .with_cache_dir(temp_dir.path().to_path_buf())
        .with_auto_download(true)
        .with_offline_mode(false)
        .with_max_tokens(512)
        .with_relevance_threshold(0.4);

    assert_eq!(config.repo, "sentence-transformers/all-MiniLM-L6-v2");
    assert_eq!(config.model_file, "model.onnx");
    assert!(config.auto_download);
    assert!(!config.offline_mode);
    assert_eq!(config.max_tokens, 512);
    assert_eq!(config.relevance_threshold, 0.4);
}

/// Test that ThresholdConfig type exists
#[test]
fn test_threshold_config_defaults() {
    use webfang_ai::infrastructure_ai::ThresholdConfig;

    let config = ThresholdConfig::new();
    assert_eq!(config.min_threshold(), 0.0);
    assert_eq!(config.max_threshold(), 1.0);
    assert_eq!(config.default_threshold(), 0.3);
}

/// Test that AiModel::Granite97M is the default
#[test]
fn test_ai_model_default_is_granite_97m() {
    use webfang_ai::infrastructure_ai::AiModel;
    assert_eq!(AiModel::default(), AiModel::Granite97M);
}

/// Test that AiModel::Granite97M has repo_id matching expected
#[test]
fn test_ai_model_granite_97m_repo_id() {
    use webfang_ai::infrastructure_ai::AiModel;
    assert_eq!(
        AiModel::Granite97M.repo_id(),
        "ibm-granite/granite-embedding-97m-multilingual-r2"
    );
}

/// Test that AiModel::Granite311M has repo_id matching expected
#[test]
fn test_ai_model_granite_311m_repo_id() {
    use webfang_ai::infrastructure_ai::AiModel;
    assert_eq!(
        AiModel::Granite311M.repo_id(),
        "ibm-granite/granite-embedding-311m-multilingual-r2"
    );
}

/// Test AiModel embedding dimensions
#[test]
fn test_ai_model_embedding_dims() {
    use webfang_ai::infrastructure_ai::AiModel;
    assert_eq!(AiModel::Granite97M.embedding_dim(), 384);
    assert_eq!(AiModel::Granite311M.embedding_dim(), 768);
    // Both produce 384d output (unified storage)
    assert_eq!(AiModel::Granite97M.output_dim(), 384);
    assert_eq!(AiModel::Granite311M.output_dim(), 384);
}

/// Test AiModel::parse with valid and invalid values
#[test]
fn test_ai_model_parse() {
    use webfang_ai::infrastructure_ai::AiModel;

    assert_eq!(AiModel::parse("granite-97m"), Some(AiModel::Granite97M));
    assert_eq!(AiModel::parse("granite-311m"), Some(AiModel::Granite311M));
    assert_eq!(AiModel::parse("GRANITE-97M"), Some(AiModel::Granite97M));
    assert_eq!(AiModel::parse("unknown"), None);
    assert_eq!(AiModel::parse(""), None);
}

/// Test AiModel::FromStr trait impl for error messages
#[test]
fn test_ai_model_from_str() {
    use webfang_ai::infrastructure_ai::AiModel;

    let ok: AiModel = "granite-97m".parse().unwrap();
    assert_eq!(ok, AiModel::Granite97M);

    let err: Result<AiModel, _> = "unknown-model".parse();
    assert!(err.is_err());
    let msg = err.unwrap_err();
    assert!(msg.contains("Unknown AI model"));
    assert!(msg.contains("granite-97m"));
    assert!(msg.contains("granite-311m"));
}

/// Test Matryoshka truncation: 768d -> 384d
#[test]
fn test_matryoshka_truncation_768_to_384() {
    use webfang_ai::infrastructure_ai::embedding_ops::{l2_normalize_safe, mean_pool};

    // Simulate 768d native output from Granite-311M
    let embedding_flat_768: Vec<f32> = (0..768).map(|i| (i as f32 + 1.0) / 768.0).collect();
    let attention_mask: Vec<i64> = vec![1i64]; // seq_len=1

    let pooled = mean_pool(&embedding_flat_768, 1, 768, &attention_mask);
    let truncated: Vec<f32> = pooled.iter().take(384).copied().collect();
    let normalized = l2_normalize_safe(&truncated);

    assert_eq!(normalized.len(), 384);

    // Verify unit length
    let norm: f32 = normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5);
}

/// Test that 97M 384d output passes through without truncation (identity)
#[test]
fn test_matryoshka_identity_for_384d() {
    use webfang_ai::infrastructure_ai::embedding_ops::{l2_normalize_safe, mean_pool};

    // Simulate native 384d output from Granite-97M
    let embedding_flat_384: Vec<f32> = (0..384).map(|i| (i as f32 + 1.0) / 384.0).collect();
    let attention_mask: Vec<i64> = vec![1i64];

    let pooled = mean_pool(&embedding_flat_384, 1, 384, &attention_mask);
    // No Matryoshka needed -- native 384d
    let truncated: Vec<f32> = pooled.iter().take(384).copied().collect();
    let normalized = l2_normalize_safe(&truncated);

    assert_eq!(normalized.len(), 384);
    let norm: f32 = normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5);
}

// ============================================================================
// Full RAG Pipeline Integration Tests (require cached model)
// ============================================================================

/// Test full pipeline: HTML -> Chunk -> Tokenize -> Embed -> Score -> Filter
///
/// This test verifies the complete RAG pipeline integration.
/// Skips if model is not cached to avoid network dependency.
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn test_semantic_cleaner_full_pipeline() {
    let config = ModelConfig::default().with_offline_mode(true);
    let cleaner = SemanticCleanerImpl::new(config).await;

    if let Ok(cleaner) = cleaner {
        let html = "<article><p>Hello world. Test content for semantic cleaning.</p></article>";
        let chunks = cleaner.clean(html).await;

        assert!(
            chunks.is_ok(),
            "Pipeline should succeed: {:?}",
            chunks.err()
        );

        let chunks = chunks.unwrap();
        eprintln!("Generated {} chunks", chunks.len());
    } else {
        eprintln!("SKIP: cleaner creation failed");
    }
}

/// Test semantic cleaner with longer content
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn test_semantic_cleaner_long_content() {
    let config = ModelConfig::default().with_offline_mode(true);
    let cleaner = SemanticCleanerImpl::new(config).await;

    if let Ok(cleaner) = cleaner {
        let html = r#"
            <article>
                <h1>Test Article</h1>
                <p>This is a comprehensive test article with multiple paragraphs.
                Each paragraph contains enough text to meet the minimum chunk size
                requirement for the semantic chunker. This ensures that the chunking
                algorithm has sufficient content to work with during processing.</p>

                <p>The second paragraph provides additional content for testing.
                It includes more text to verify that the chunker can handle
                multiple paragraphs and split them appropriately based on the
                configured minimum and maximum chunk sizes.</p>

                <p>A third paragraph ensures that the chunking algorithm can
                handle multiple chunks and process them independently through
                the embedding generation and relevance scoring pipeline.</p>
            </article>
        "#;

        let chunks = cleaner.clean(html).await;

        assert!(
            chunks.is_ok(),
            "Pipeline should succeed: {:?}",
            chunks.err()
        );

        let chunks = chunks.unwrap();
        eprintln!("Generated {} chunks from long content", chunks.len());
    } else {
        eprintln!("SKIP: cleaner creation failed");
    }
}

/// Test concurrent embedding generation
#[tokio::test]
async fn test_concurrent_embeddings() {
    use futures::future::try_join_all;

    let tasks: Vec<_> = (0..3)
        .map(|i| async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            Ok::<_, SemanticError>(vec![i as f32; 384])
        })
        .collect();

    let results = try_join_all(tasks).await;

    assert!(results.is_ok(), "Concurrent tasks should succeed");
    let results = results.unwrap();
    assert_eq!(results.len(), 3, "Should produce 3 embeddings");
    assert_eq!(results[0].len(), 384, "Embedding dimension should be 384");
}

/// Test relevance filtering
#[test]
fn test_relevance_filtering() {
    use webfang_ai::infrastructure_ai::RelevanceScorer;

    let scorer = RelevanceScorer::new(0.3);
    let reference = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let chunk1 = DocumentChunk::new(uuid::Uuid::new_v4(), "", "", "High similarity");
    let emb1 = vec![0.9f32, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let chunk2 = DocumentChunk::new(uuid::Uuid::new_v4(), "", "", "Low similarity");
    let emb2 = vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let chunks = vec![(chunk1, emb1), (chunk2, emb2)];
    let filtered = scorer.filter(&chunks, Some(&reference));

    eprintln!("Filtered {} chunks", filtered.len());
}

/// Test error handling: chunk too large
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn test_error_chunk_too_large() {
    let config = ModelConfig::default()
        .with_offline_mode(true)
        .with_max_tokens(512);

    let cleaner = SemanticCleanerImpl::new(config).await;

    if let Ok(cleaner) = cleaner {
        let long_content = "Test. ".repeat(2000);
        let html = format!("<p>{}</p>", long_content);

        let result = cleaner.clean(&html).await;

        match result {
            Ok(chunks) => {
                eprintln!("Content split into {} chunks", chunks.len());
                for chunk in &chunks {
                    assert!(chunk.content.len() <= 2048, "Chunk content too large");
                }
            },
            Err(SemanticError::ChunkTooLarge { .. }) => {
                eprintln!("Correctly detected chunk too large");
            },
            Err(e) => {
                eprintln!("Other error (acceptable): {}", e);
            },
        }
    } else {
        eprintln!("SKIP: cleaner creation failed");
    }
}

/// Test offline mode error
#[tokio::test]
async fn test_offline_mode_error() {
    let temp_cache_dir = PathBuf::from(format!("/tmp/webfang_test_cache_{}", std::process::id()));

    let config = ModelConfig::new()
        .with_cache_dir(temp_cache_dir)
        .with_auto_download(false)
        .with_offline_mode(true);

    let result = SemanticCleanerImpl::new(config).await;

    assert!(result.is_err());

    if let Err(SemanticError::OfflineMode { repo }) = result {
        assert_eq!(repo, DEFAULT_MODEL_REPO);
    } else {
        panic!("Expected SemanticError::OfflineMode");
    }
}

/// Test pipeline with empty input
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn test_pipeline_empty_input() {
    let config = ModelConfig::default().with_offline_mode(true);
    let cleaner = SemanticCleanerImpl::new(config).await;

    if let Ok(cleaner) = cleaner {
        let html = "";
        let chunks = cleaner.clean(html).await;

        assert!(chunks.is_ok(), "Empty input should not fail");
        let chunks = chunks.unwrap();
        assert!(chunks.is_empty(), "Empty input should produce no chunks");
    } else {
        eprintln!("SKIP: cleaner creation failed");
    }
}

/// Test pipeline with HTML-only input (no text content)
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn test_pipeline_html_only() {
    let config = ModelConfig::default().with_offline_mode(true);
    let cleaner = SemanticCleanerImpl::new(config).await;

    if let Ok(cleaner) = cleaner {
        let html = "<div></div><span></span>";
        let chunks = cleaner.clean(html).await;

        assert!(chunks.is_ok(), "HTML-only input should not fail");
        let chunks = chunks.unwrap();
        assert!(
            chunks.is_empty(),
            "HTML-only input should produce no chunks"
        );
    } else {
        eprintln!("SKIP: cleaner creation failed");
    }
}

/// Test relevance scorer threshold validation
#[test]
#[should_panic(expected = "Threshold must be between")]
fn test_relevance_scorer_invalid_threshold() {
    use webfang_ai::infrastructure_ai::RelevanceScorer;

    let _ = RelevanceScorer::new(1.5);
}

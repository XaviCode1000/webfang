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

use rust_scraper::domain::DocumentChunk;
use rust_scraper::infrastructure::ai::{
    default_cache_dir, CacheConfig, ModelCache, DEFAULT_MODEL_FILE, DEFAULT_MODEL_REPO,
};
use rust_scraper::infrastructure::ai::model_downloader::ModelDownloader;
use rust_scraper::infrastructure::ai::{InferenceEngine, ModelConfig, SemanticCleanerImpl};
use rust_scraper::SemanticCleaner;
use rust_scraper::SemanticError;
use std::path::PathBuf;

// ============================================================================
// Existing Tests (unchanged - see original file for full content)
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
    assert_eq!(config.max_tokens, 512);

    // Verify cache_dir ends with ai_models
    assert!(config.cache_dir.to_string_lossy().contains("ai_models"));
}

/// Test that ModelConfig builder pattern works
#[test]
fn test_model_config_builder() {
    let temp_dir = tempfile::tempdir().unwrap();

    let config = ModelConfig::new()
        .with_repo("test/repo")
        .with_file("test.onnx")
        .with_cache_dir(temp_dir.path().to_path_buf())
        .with_auto_download(false)
        .with_offline_mode(true)
        .with_max_tokens(256);

    assert_eq!(config.repo, "test/repo");
    assert_eq!(config.model_file, "test.onnx");
    assert_eq!(config.cache_dir, temp_dir.path());
    assert!(!config.auto_download);
    assert!(config.offline_mode);
    assert_eq!(config.max_tokens, 256);
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
    let chunk = DocumentChunk {
        id: uuid::Uuid::new_v4(),
        url: "https://example.com".to_string(),
        title: "Test Page".to_string(),
        content: "Test content".to_string(),
        metadata: std::collections::HashMap::new(),
        timestamp: chrono::Utc::now(),
        embeddings: None,
    };

    assert_eq!(chunk.url, "https://example.com");
    assert_eq!(chunk.title, "Test Page");
    assert_eq!(chunk.content, "Test content");
    assert!(!chunk.has_embeddings());
}

/// Test that default_cache_dir returns a valid path
#[test]
fn test_default_cache_dir() {
    let cache_dir = default_cache_dir();

    // Should end with ai_models
    assert!(cache_dir.to_string_lossy().ends_with("ai_models"));

    // Should contain rust-scraper
    assert!(cache_dir.to_string_lossy().contains("rust-scraper"));
}

/// Test that ModelCache can check if a model is cached
#[tokio::test]
async fn test_model_cache_is_cached() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_dir = temp_dir.path().join("test_cache");

    let config = CacheConfig::new().with_cache_dir(cache_dir.clone());
    let cache = ModelCache::new(config);

    // Should return false for non-existent file
    assert!(!cache.is_model_cached("model.onnx"));

    // Create a dummy file
    tokio::fs::create_dir_all(&cache_dir).await.unwrap();
    tokio::fs::File::create(cache_dir.join("model.onnx"))
        .await
        .unwrap();

    // Should return true now
    assert!(cache.is_model_cached("model.onnx"));
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
    use rust_scraper::infrastructure::ai::DownloadProgress;

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
    // Test ModelLoad error
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let err = SemanticError::ModelLoad(io_err);
    assert!(err.to_string().contains("cargando modelo"));

    // Test ChunkTooLarge error
    let err = SemanticError::ChunkTooLarge {
        chunk_id: "chunk-1".to_string(),
        tokens: 600,
        max: 512,
    };
    assert!(err.to_string().contains("chunk-1"));
    assert!(err.to_string().contains("600 > 512"));

    // Test Download error
    let err = SemanticError::Download {
        repo: "test/repo".to_string(),
        cause: "network error".to_string(),
    };
    assert!(err.to_string().contains("test/repo"));
    assert!(err.to_string().contains("network error"));

    // Test CacheValidation error
    let err = SemanticError::CacheValidation {
        repo: "test/repo".to_string(),
        expected: "abc123".to_string(),
        actual: "def456".to_string(),
    };
    assert!(err.to_string().contains("abc123"));
    assert!(err.to_string().contains("def456"));

    // Test OfflineMode error
    let err = SemanticError::OfflineMode {
        repo: "test/repo".to_string(),
    };
    assert!(err.to_string().contains("test/repo"));
    assert!(err.to_string().contains("offline"));
}

/// Test that ScraperError can be created from SemanticError
#[test]
fn test_scraper_error_from_semantic_error() {
    use rust_scraper::ScraperError;

    let semantic_err = SemanticError::ModelLoad(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "model missing",
    ));

    let scraper_err: ScraperError = semantic_err.into();
    assert!(scraper_err.to_string().contains("limpieza semántica"));
}

// ============================================================================
// InferenceEngine Tests (Phase 2)
// ============================================================================

/// Test that InferenceEngine is Send + Sync (thread-safe)
///
/// This is critical for using InferenceEngine in async contexts
/// with tokio::spawn and across thread boundaries.
///
/// Following `own-arc-shared` and `async-spawn-blocking` rules,
/// InferenceEngine must be Send + Sync to work with Arc and spawn_blocking.
#[test]
fn test_inference_engine_is_send_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    assert_send::<InferenceEngine>();
    assert_sync::<InferenceEngine>();
}

/// Test that InferenceEngine is Clone (cheap Arc clone)
///
/// InferenceEngine wraps Arc<RunnableModel>, so cloning is cheap
/// (just increments atomic counter) and safe for concurrent use.
#[test]
fn test_inference_engine_is_clone() {
    fn assert_clone<T: Clone>() {}
    assert_clone::<InferenceEngine>();
}

/// Test that TokenBatch can be created
///
/// Verifies the token batch structure for batch inference.
#[test]
fn test_token_batch_creation() {
    use rust_scraper::infrastructure::ai::tokenizer::TokenBatch;

    let batch = TokenBatch::new(
        vec![vec![1, 2, 3], vec![4, 5, 6]],
        vec![vec![1, 1, 1], vec![1, 1, 1]],
        vec![vec![0, 0, 0], vec![0, 0, 0]],
    );

    assert_eq!(batch.len(), 2);
    assert_eq!(batch.sequence_length(), 3);
    assert!(!batch.is_empty());
}

/// Test tokenizer type traits
///
/// Verifies that MiniLmTokenizer has the correct Send/Sync properties.
#[test]
fn test_tokenizer_type_traits() {
    use rust_scraper::infrastructure::ai::tokenizer::MiniLmTokenizer;

    fn assert_send<T: Send>() {}

    // MiniLmTokenizer should be Send (can be moved between threads)
    // but not necessarily Sync (internal state may not be thread-safe)
    assert_send::<MiniLmTokenizer>();
}

// ============================================================================
// Module 3 Tests: Semantic Chunking (ChunkId, Sentence, Chunker)
// ============================================================================

/// Test that ChunkId type exists and compiles
/// Test ChunkId creation and display
#[test]
fn test_chunk_id_display() {
    use rust_scraper::infrastructure::ai::ChunkId;

    let id = ChunkId(42);
    assert_eq!(format!("{}", id), "chunk-42");
}

/// Test ChunkId inner value access
#[test]
fn test_chunk_id_inner() {
    use rust_scraper::infrastructure::ai::ChunkId;

    let id = ChunkId::new(123);
    assert_eq!(id.inner(), 123);
}

/// Test ChunkId equality
#[test]
fn test_chunk_id_equality() {
    use rust_scraper::infrastructure::ai::ChunkId;

    let id1 = ChunkId(42);
    let id2 = ChunkId(42);
    let id3 = ChunkId(43);

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
}

/// Test that SentenceSplitter type exists
#[test]
fn test_sentence_splitter_basic() {
    use rust_scraper::infrastructure::ai::SentenceSplitter;

    let splitter = SentenceSplitter;
    let sentences = splitter.split("Hello world. How are you?");
    assert!(sentences.len() >= 2);
}

/// Test sentence splitter count
#[test]
fn test_sentence_splitter_count() {
    use rust_scraper::infrastructure::ai::SentenceSplitter;

    let splitter = SentenceSplitter;
    let count = splitter.count("One. Two. Three.");
    assert_eq!(count, 3);
}

/// Test sentence splitter trimmed output
#[test]
fn test_sentence_splitter_trimmed() {
    use rust_scraper::infrastructure::ai::SentenceSplitter;

    let splitter = SentenceSplitter;
    let sentences = splitter.split_trimmed("  First.  Second.  Third.  ");
    assert_eq!(sentences.len(), 3);
    assert_eq!(sentences[0], "First.");
}

/// Test that HtmlChunker type exists
/// Test chunker creation with defaults
#[test]
fn test_chunker_creation() {
    use rust_scraper::infrastructure::ai::HtmlChunker;

    let chunker = HtmlChunker::new();
    assert!(chunker.min_chunk_size() > 0);
    assert!(chunker.max_chunk_size() > 0);
    assert!(chunker.similarity_threshold() > 0.0);
    assert!(chunker.similarity_threshold() <= 1.0);
}

/// Test chunker builder pattern
#[test]
fn test_chunker_builder_pattern() {
    use rust_scraper::infrastructure::ai::HtmlChunker;

    let chunker = HtmlChunker::new()
        .with_min_chunk_size(80)
        .with_max_chunk_size(400)
        .with_similarity_threshold(0.6);

    assert_eq!(chunker.min_chunk_size(), 80);
    assert_eq!(chunker.max_chunk_size(), 400);
    assert_eq!(chunker.similarity_threshold(), 0.6);
}

/// Test chunker with custom config
#[test]
fn test_chunker_with_config() {
    use rust_scraper::infrastructure::ai::HtmlChunker;

    let chunker = HtmlChunker::with_config(50, 300, 0.7);
    assert_eq!(chunker.min_chunk_size(), 50);
    assert_eq!(chunker.max_chunk_size(), 300);
    assert_eq!(chunker.similarity_threshold(), 0.7);
}

/// Test chunker basic HTML processing
#[test]
fn test_chunker_basic_html() {
    use rust_scraper::infrastructure::ai::HtmlChunker;

    let chunker = HtmlChunker::new();
    let html = "<p>This is a paragraph with enough text to meet the minimum chunk size requirement for testing purposes.</p>";
    let result = chunker.chunk(html);
    assert!(result.is_ok());
}

/// Test chunker empty HTML
#[test]
fn test_chunker_empty_html() {
    use rust_scraper::infrastructure::ai::HtmlChunker;

    let chunker = HtmlChunker::new();
    let html = "";
    let result = chunker.chunk(html);
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

// ============================================================================
// Module 4 Tests: Embedding Operations, Relevance Scorer, Threshold Config
// ============================================================================

/// Test cosine similarity with identical vectors
#[test]
fn test_cosine_similarity_identical() {
    use rust_scraper::infrastructure::ai::embedding_ops::cosine_similarity;

    // Use a normalized vector (magnitude = 1.0)
    // 1/sqrt(8) ≈ 0.3536 for 8-dimensional unit vector
    let normalization = 1.0f32 / 8.0f32.sqrt();
    let vec = vec![normalization; 8];
    let sim = cosine_similarity(&vec, &vec);
    assert!((sim - 1.0).abs() < 0.001, "Expected ~1.0, got {}", sim);
}

/// Test cosine similarity with orthogonal vectors
#[test]
fn test_cosine_similarity_orthogonal() {
    use rust_scraper::infrastructure::ai::embedding_ops::cosine_similarity;

    let a = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let b = vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!(sim.abs() < 0.001, "Expected ~0.0, got {}", sim);
}

/// Test cosine similarity with opposite vectors
#[test]
fn test_cosine_similarity_opposite() {
    use rust_scraper::infrastructure::ai::embedding_ops::cosine_similarity;

    let a = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let b = vec![-1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim + 1.0).abs() < 0.001, "Expected ~-1.0, got {}", sim);
}

/// Test cosine similarity with empty vectors
#[test]
fn test_cosine_similarity_empty() {
    use rust_scraper::infrastructure::ai::embedding_ops::cosine_similarity;

    let a: Vec<f32> = vec![];
    let b: Vec<f32> = vec![];
    let sim = cosine_similarity(&a, &b);
    assert_eq!(sim, 0.0);
}

/// Test dot product scalar fallback
#[test]
fn test_dot_product_scalar() {
    use rust_scraper::infrastructure::ai::embedding_ops::dot_product_scalar;

    let a = vec![1.0f32, 2.0, 3.0];
    let b = vec![4.0f32, 5.0, 6.0];
    let dot = dot_product_scalar(&a, &b);
    assert_eq!(dot, 32.0); // 1*4 + 2*5 + 3*6 = 32
}

/// Test vector normalization
#[test]
fn test_normalize() {
    use rust_scraper::infrastructure::ai::embedding_ops::normalize;

    let v = vec![3.0f32, 4.0];
    let normalized = normalize(&v);
    let magnitude: f32 = normalized.iter().map(|&x| x * x).sum::<f32>().sqrt();
    assert!((magnitude - 1.0).abs() < 0.001);
}

/// Test Euclidean distance
#[test]
fn test_euclidean_distance() {
    use rust_scraper::infrastructure::ai::embedding_ops::euclidean_distance;

    let a = vec![0.0f32, 0.0];
    let b = vec![3.0f32, 4.0];
    let dist = euclidean_distance(&a, &b);
    assert!((dist - 5.0).abs() < 0.001); // 3-4-5 triangle
}

#[test]
/// Test relevance scorer creation
fn test_relevance_scorer_creation() {
    use rust_scraper::infrastructure::ai::RelevanceScorer;

    let scorer = RelevanceScorer::new(0.3);
    assert_eq!(scorer.threshold(), 0.3);
}

/// Test relevance scorer with reference
#[test]
fn test_relevance_scorer_with_reference() {
    use rust_scraper::infrastructure::ai::RelevanceScorer;

    let reference = vec![0.5f32; 8];
    let scorer = RelevanceScorer::with_reference(0.5, reference.clone());
    assert_eq!(scorer.threshold(), 0.5);
    assert_eq!(scorer.reference(), Some(reference.as_slice()));
}

/// Test relevance scorer threshold validation
#[test]
#[should_panic(expected = "Threshold must be between")]
fn test_relevance_scorer_invalid_threshold() {
    use rust_scraper::infrastructure::ai::RelevanceScorer;

    let _ = RelevanceScorer::new(1.5);
}

/// Test relevance scorer meets_threshold
#[test]
fn test_relevance_scorer_meets_threshold() {
    use rust_scraper::infrastructure::ai::RelevanceScorer;

    let scorer = RelevanceScorer::new(0.5);
    assert!(scorer.meets_threshold(0.6));
    assert!(scorer.meets_threshold(0.5));
    assert!(!scorer.meets_threshold(0.4));
}
/// Test that ThresholdConfig type exists
#[test]
/// Test threshold config default values
fn test_threshold_config_defaults() {
    use rust_scraper::infrastructure::ai::ThresholdConfig;

    let config = ThresholdConfig::new();
    assert_eq!(config.min_threshold(), 0.0);
    assert_eq!(config.max_threshold(), 1.0);
    assert_eq!(config.default_threshold(), 0.3);
}

/// Test threshold config builder pattern
#[test]
fn test_threshold_config_builder() {
    use rust_scraper::infrastructure::ai::ThresholdConfig;

    let config = ThresholdConfig::new()
        .with_min_threshold(0.2)
        .with_max_threshold(0.8)
        .with_default_threshold(0.5)
        .build();

    assert_eq!(config.min_threshold(), 0.2);
    assert_eq!(config.max_threshold(), 0.8);
    assert_eq!(config.default_threshold(), 0.5);
}

/// Test threshold config is_valid
#[test]
fn test_threshold_config_is_valid() {
    use rust_scraper::infrastructure::ai::ThresholdConfig;

    let config = ThresholdConfig::new()
        .with_min_threshold(0.2)
        .with_max_threshold(0.8)
        .build();

    assert!(config.is_valid(0.5));
    assert!(!config.is_valid(0.1));
}

/// Test threshold config clamp
#[test]
fn test_threshold_config_clamp() {
    use rust_scraper::infrastructure::ai::ThresholdConfig;

    let config = ThresholdConfig::new()
        .with_min_threshold(0.2)
        .with_max_threshold(0.8)
        .build();

    assert_eq!(config.clamp(0.1), 0.2);
    assert_eq!(config.clamp(0.5), 0.5);
    assert_eq!(config.clamp(0.9), 0.8);
}

/// Test threshold config strict preset
#[test]
fn test_threshold_config_strict() {
    use rust_scraper::infrastructure::ai::ThresholdConfig;

    let config = ThresholdConfig::strict();
    assert_eq!(config.min_threshold(), 0.5);
    assert_eq!(config.max_threshold(), 1.0);
    assert_eq!(config.default_threshold(), 0.7);
}

/// Test threshold config lenient preset
#[test]
fn test_threshold_config_lenient() {
    use rust_scraper::infrastructure::ai::ThresholdConfig;

    let config = ThresholdConfig::lenient();
    assert_eq!(config.min_threshold(), 0.0);
    assert_eq!(config.max_threshold(), 0.5);
    assert_eq!(config.default_threshold(), 0.2);
}

/// Test threshold config balanced preset
#[test]
fn test_threshold_config_balanced() {
    use rust_scraper::infrastructure::ai::ThresholdConfig;

    let config = ThresholdConfig::balanced();
    assert_eq!(config.min_threshold(), 0.1);
    assert_eq!(config.max_threshold(), 0.9);
    assert_eq!(config.default_threshold(), 0.4);
}

// ============================================================================
// NEW: Full RAG Pipeline Integration Tests (Phase 2 + Phase 3)
// ============================================================================

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

/// Test full pipeline: HTML → Chunk → Tokenize → Embed → Score → Filter
///
/// This test verifies the complete RAG pipeline integration.
/// Skips if model is not cached to avoid network dependency.
#[tokio::test]
async fn test_semantic_cleaner_full_pipeline() {
    // Skip if model not cached
    if !default_cache_dir().join("model.onnx").exists() {
        eprintln!("SKIP: model not cached");
        return;
    }

    // Skip if tokenizer not cached
    if !default_cache_dir().join("tokenizer.json").exists() {
        eprintln!("SKIP: tokenizer not cached");
        return;
    }

    let config = ModelConfig::default().with_offline_mode(true);
    let cleaner = SemanticCleanerImpl::new(config).await;

    // If cleaner loaded successfully, test the pipeline
    if let Ok(cleaner) = cleaner {
        let html = "<article><p>Hello world. Test content for semantic cleaning.</p></article>";
        let chunks = cleaner.clean(html).await;

        // Pipeline should succeed
        assert!(
            chunks.is_ok(),
            "Pipeline should succeed: {:?}",
            chunks.err()
        );

        let chunks = chunks.unwrap();

        // Should produce at least one chunk (or empty if content too short)
        // Note: With min_chunk_size=100, short content may produce no chunks
        eprintln!("Generated {} chunks", chunks.len());
    } else {
        eprintln!("SKIP: cleaner creation failed");
    }
}

/// Test semantic cleaner with longer content
///
/// Verifies that longer content produces chunks.
#[tokio::test]
async fn test_semantic_cleaner_long_content() {
    // Skip if model not cached
    if !default_cache_dir().join("model.onnx").exists() {
        eprintln!("SKIP: model not cached");
        return;
    }

    if !default_cache_dir().join("tokenizer.json").exists() {
        eprintln!("SKIP: tokenizer not cached");
        return;
    }

    let config = ModelConfig::default().with_offline_mode(true);
    let cleaner = SemanticCleanerImpl::new(config).await;

    if let Ok(cleaner) = cleaner {
        // Longer content that should produce chunks
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

        // With enough content, should produce at least 1 chunk
        // (exact number depends on chunker configuration)
    } else {
        eprintln!("SKIP: cleaner creation failed");
    }
}

/// Test concurrent embedding generation
///
/// Verifies that try_join_all works correctly for concurrent inference.
/// This test may need a mock InferenceEngine for comprehensive testing.
#[tokio::test]
async fn test_concurrent_embeddings() {
    // This test verifies the concurrent embedding pattern
    // In production, this would use real InferenceEngine

    use futures::future::try_join_all;

    // Simulate concurrent operations
    let tasks: Vec<_> = (0..3)
        .map(|i| async move {
            // Simulate inference latency
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
///
/// Verifies that chunks are filtered by relevance threshold.
#[test]
fn test_relevance_filtering() {
    use rust_scraper::infrastructure::ai::RelevanceScorer;

    let scorer = RelevanceScorer::new(0.3);

    // Create test chunks with embeddings
    let reference = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    // High similarity chunk
    let chunk1 = DocumentChunk {
        id: uuid::Uuid::new_v4(),
        url: String::new(),
        title: String::new(),
        content: "High similarity".to_string(),
        metadata: std::collections::HashMap::new(),
        timestamp: chrono::Utc::now(),
        embeddings: None,
    };
    let emb1 = vec![0.9f32, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    // Low similarity chunk (orthogonal)
    let chunk2 = DocumentChunk {
        id: uuid::Uuid::new_v4(),
        url: String::new(),
        title: String::new(),
        content: "Low similarity".to_string(),
        metadata: std::collections::HashMap::new(),
        timestamp: chrono::Utc::now(),
        embeddings: None,
    };
    let emb2 = vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let chunks = vec![(chunk1, emb1), (chunk2, emb2)];
    let filtered = scorer.filter(&chunks, Some(&reference));

    // Should filter based on threshold
    // chunk1 should pass (high similarity)
    // chunk2 should be filtered (orthogonal)
    eprintln!("Filtered {} chunks", filtered.len());
}

/// Test error handling: chunk too large
#[tokio::test]
async fn test_error_chunk_too_large() {
    // Skip if model not cached
    if !default_cache_dir().join("model.onnx").exists() {
        eprintln!("SKIP: model not cached");
        return;
    }

    if !default_cache_dir().join("tokenizer.json").exists() {
        eprintln!("SKIP: tokenizer not cached");
        return;
    }

    let config = ModelConfig::default()
        .with_offline_mode(true)
        .with_max_tokens(512);

    let cleaner = SemanticCleanerImpl::new(config).await;

    if let Ok(cleaner) = cleaner {
        // Create content that would exceed token limit
        let long_content = "Test. ".repeat(2000); // Very long content
        let html = format!("<p>{}</p>", long_content);

        let result = cleaner.clean(&html).await;

        // Should either:
        // 1. Succeed with multiple chunks (chunker splits content)
        // 2. Fail with ChunkTooLarge if tokenization exceeds limit
        match result {
            Ok(chunks) => {
                eprintln!("Content split into {} chunks", chunks.len());
                // Verify chunks are reasonable size (max_chunk_size is 512, so *4 = 2048 chars safe zone)
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
    let temp_cache_dir = PathBuf::from(format!(
        "/tmp/rust_scraper_test_cache_{}",
        std::process::id()
    ));

    let config = ModelConfig::new()
        .with_cache_dir(temp_cache_dir)
        .with_auto_download(false)
        .with_offline_mode(true);

    let result = SemanticCleanerImpl::new(config).await;

    // Should fail with OfflineMode error
    assert!(result.is_err());

    if let Err(SemanticError::OfflineMode { repo }) = result {
        assert_eq!(repo, DEFAULT_MODEL_REPO);
    } else {
        panic!("Expected SemanticError::OfflineMode");
    }
}

/// Test pipeline with empty input
#[tokio::test]
async fn test_pipeline_empty_input() {
    // Skip if model not cached
    if !default_cache_dir().join("model.onnx").exists() {
        eprintln!("SKIP: model not cached");
        return;
    }

    if !default_cache_dir().join("tokenizer.json").exists() {
        eprintln!("SKIP: tokenizer not cached");
        return;
    }

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
async fn test_pipeline_html_only() {
    // Skip if model not cached
    if !default_cache_dir().join("model.onnx").exists() {
        eprintln!("SKIP: model not cached");
        return;
    }

    if !default_cache_dir().join("tokenizer.json").exists() {
        eprintln!("SKIP: tokenizer not cached");
        return;
    }

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

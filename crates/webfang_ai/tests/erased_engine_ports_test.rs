//! Erased-engine port seam (issue #1569).
//!
//! Mock-backed: NO model download, NO ORT session. Proves that a
//! `Pool`-mode cleaner (`SemanticCleanerImpl<dyn InferenceEngine + Send +
//! Sync>`) shares its ONE engine + tokenizer with both erased consumers:
//!
//! 1. the vault-search embedding port ([`EmbeddingAdapter`]), and
//! 2. the Tier 2 semantic inspector ([`GraniteDomInspector`]),
//!
//! so the default `WEBFANG_AI_ENGINE` rollout serves both ports from a single
//! model in memory — no degradation warning, no second model load. The same
//! wiring compiles for the `Single` rollback hatch via unsized coercion of the
//! concrete `Arc<InferencePool>` into the erased field.
//!
//! Requires the `ai` feature (same gate as the other AI integration tests).

#![cfg(feature = "ai")]

use std::sync::Arc;
use std::time::Duration;

use webfang_ai::infrastructure_ai::{
    InferenceEngine, MockInferenceEngine, ModelConfig, SemanticCleanerImpl,
};
use webfang_ai::{EmbeddingAdapter, GraniteDomInspector, MiniLmTokenizer};
use webfang_core::domain::embedding_port::EmbeddingPort;
use webfang_core::domain::semantic_inspector::{SemanticContext, SemanticInspectorPort};

/// Build a minimal in-memory WordPiece tokenizer — no tokenizer.json file
/// required. Only needs to EXIST for component construction; the mock engine
/// never inspects tokens.
fn in_memory_tokenizer() -> tokenizers::Tokenizer {
    use tokenizers::models::wordpiece::WordPiece;
    let vocab = [
        ("[PAD]".to_string(), 0u32),
        ("[UNK]".to_string(), 100),
        ("[CLS]".to_string(), 101),
        ("[SEP]".to_string(), 102),
        ("hello".to_string(), 5),
        ("world".to_string(), 6),
    ];
    let model = WordPiece::builder()
        .vocab(vocab)
        .unk_token("[UNK]".to_string())
        .build()
        .expect("wordpiece model must build from an inline vocab");
    tokenizers::Tokenizer::new(model)
}

/// One fixed-latency mock engine, erased exactly the way
/// `build_engine` returns it for `EngineConfig::Pool { N }`.
fn erased_mock_engine() -> Arc<dyn InferenceEngine + Send + Sync> {
    Arc::new(MockInferenceEngine::new(Duration::from_millis(1)))
}

/// The Pool-mode cleaner erases to `SemanticCleanerImpl<dyn InferenceEngine +
/// Send + Sync>` and `shared_inference` hands out the erased pair — the exact
/// type the CLI Pool arm and the MCP daemon now consume (#1569).
#[test]
fn pool_cleaner_erases_and_shares_erased_engine() {
    fn assert_erased_cleaner(_: &SemanticCleanerImpl<dyn InferenceEngine + Send + Sync>) {}

    let cleaner = SemanticCleanerImpl::from_parts(
        erased_mock_engine(),
        Arc::new(MiniLmTokenizer::new(in_memory_tokenizer(), 512)),
        ModelConfig::default(),
    );
    assert_erased_cleaner(&cleaner);

    let (engine, _tokenizer) = cleaner.shared_inference();
    assert!(engine.is_ready());
    assert_eq!(engine.embedding_dim(), 384);
}

/// Default Pool mode: the cleaner's ONE shared engine + tokenizer back BOTH
/// downstream ports — vault-search embedding and Tier 2 semantic repair —
/// with no concrete `InferencePool` type anywhere in the wiring.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_cleaner_serves_vault_embedding_and_tier2_from_one_engine() {
    let cleaner = SemanticCleanerImpl::from_parts(
        erased_mock_engine(),
        Arc::new(MiniLmTokenizer::new(in_memory_tokenizer(), 512)),
        ModelConfig::default(),
    );
    let (engine, tokenizer) = cleaner.shared_inference();

    // Port 1 — vault-search embedding over the erased engine.
    let adapter = EmbeddingAdapter::new(Arc::clone(&engine), Arc::clone(&tokenizer));
    let vault: Arc<dyn EmbeddingPort> = Arc::new(adapter);
    let embedding = vault
        .embed("hello world")
        .await
        .expect("mock engine must embed");
    assert_eq!(embedding.len(), 384, "Granite output dim is 384");
    assert_eq!(vault.embedding_dim(), 384);

    // Port 2 — Tier 2 semantic inspector over the SAME erased engine. The mock
    // engine answers every input with the same deterministic embedding, so the
    // target matches every fragment at cosine 1.0 and the best fragment wins.
    let inspector = GraniteDomInspector::new(engine, tokenizer, 0.75);
    let tier2: Arc<dyn SemanticInspectorPort> = Arc::new(inspector);
    let ctx = SemanticContext {
        target_text: "hello world".to_string(),
        dom_fragments: vec!["hello world".to_string(), "other text".to_string()],
        domain_hint: None,
    };
    let matched = tier2
        .find_semantic_match(ctx)
        .await
        .expect("tier2 inspection must not error");
    let hit = matched.expect("cosine 1.0 clears the 0.75 threshold");
    assert_eq!(hit.selector, "hello world");
    assert!(
        (hit.confidence - 1.0).abs() < 1e-5,
        "got {}",
        hit.confidence
    );
    assert_eq!(
        hit.source,
        webfang_core::domain::semantic_inspector::TierSource::Semantic
    );
}

/// Rollback hatch (`WEBFANG_AI_ENGINE=single`): the concrete Single-path
/// `Arc<InferencePool>` coerces into the erased `EmbeddingAdapter::new` /
/// `GraniteDomInspector::new` parameters, so both consumers keep compiling
/// unchanged for the concrete engine too (compile-time proof, no model load —
/// an unloadable model path still constructs the drainer-backed pool).
#[test]
fn concrete_single_pool_coerces_into_erased_ports() {
    use webfang_ai::infrastructure_ai::{AiModel, InferencePool};

    let pool = Arc::new(
        InferencePool::new(
            std::path::PathBuf::from("/nonexistent/webfang-fake-model.onnx"),
            AiModel::Granite97M,
        )
        .expect("pool creation must succeed even with an unloadable model file"),
    );
    let tokenizer = Arc::new(MiniLmTokenizer::new(in_memory_tokenizer(), 512));

    // No type annotation needed: a concrete `Arc<InferencePool>` value
    // unsized-coerces to `Arc<dyn InferenceEngine + Send + Sync>` at both call
    // sites (`Arc::clone` results must first land in a local, since the
    // generic `clone` blocks coercion inference).
    let pool_for_adapter = Arc::clone(&pool);
    let _adapter = EmbeddingAdapter::new(pool_for_adapter, Arc::clone(&tokenizer));
    let _inspector = GraniteDomInspector::new(pool, tokenizer, 0.75);
}

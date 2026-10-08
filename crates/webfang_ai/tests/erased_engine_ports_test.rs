//! Port-erased cleaner seam (issue #1569, ADR-0004 Tramo D slice B).
//!
//! Mock-backed: NO model download, NO ORT session. Proves the seam the cleaner
//! actually has now: ONE `SemanticCleanerImpl` type that takes an erased
//! `Arc<dyn EmbeddingPort>` and NOTHING engine-shaped. The same ONE engine +
//! tokenizer still backs three consumers — the cleaner, the vault-search
//! embedding port ([`EmbeddingAdapter`]) and the Tier 2 semantic inspector
//! ([`GraniteDomInspector`]) — so the default `WEBFANG_AI_ENGINE` rollout
//! serves all of them from a single model in memory. The `Single` rollback
//! hatch compiles unchanged via unsized coercion of the concrete
//! `Arc<InferencePool>` into the erased engine.
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
use webfang_core::domain::semantic_cleaner::SemanticCleaner;
use webfang_core::domain::semantic_inspector::{SemanticContext, SemanticInspectorPort};

// The ONE home of the in-memory tokenizer, shared with the `embedding_adapter`
// unit tests in `src/infrastructure_ai/embedding_adapter.rs` (#1575). Included by
// path because a `#[cfg(test)]` module inside the library is invisible to an
// integration test; see the fixture's module docs for why no dev-dependency
// crate is used.
#[path = "../src/infrastructure_ai/ai_test_fixture.rs"]
mod ai_test_fixture;

use ai_test_fixture::{in_memory_wordpiece_tokenizer, UNLOADABLE_MODEL_PATH};

/// One fixed-latency mock engine, erased exactly the way
/// `build_engine` returns it for `EngineConfig::Pool { N }`.
fn erased_mock_engine() -> Arc<dyn InferenceEngine + Send + Sync> {
    Arc::new(MockInferenceEngine::new(Duration::from_millis(1)))
}

/// A `MiniLmTokenizer` over the shared in-memory WordPiece model, wrapped in
/// the `Arc` every consumer of a shared engine expects.
fn in_memory_mini_lm() -> Arc<MiniLmTokenizer> {
    Arc::new(MiniLmTokenizer::new(in_memory_wordpiece_tokenizer(), 512))
}

/// The same three-tuple `build_onnx_embedding_port` hands its callers: the
/// erased domain port for the cleaner, plus the erased engine and tokenizer
/// the other two consumers still need. Built by hand here because the
/// production helper downloads a model.
fn mock_port_and_engine() -> (
    Arc<dyn EmbeddingPort>,
    Arc<dyn InferenceEngine + Send + Sync>,
    Arc<MiniLmTokenizer>,
) {
    let engine = erased_mock_engine();
    let tokenizer = in_memory_mini_lm();
    let port: Arc<dyn EmbeddingPort> = Arc::new(EmbeddingAdapter::new(
        Arc::clone(&engine),
        Arc::clone(&tokenizer),
    ));
    (port, engine, tokenizer)
}

/// The cleaner is NON-GENERIC: it takes an erased port, erases to
/// `Arc<dyn SemanticCleaner>`, and leaves the engine it was built over fully
/// usable for the other consumers (ADR-0004).
#[test]
fn cleaner_erases_to_the_domain_trait_and_leaves_the_engine_free() {
    fn assert_non_generic_cleaner(_: &SemanticCleanerImpl) {}

    let (port, engine, _tokenizer) = mock_port_and_engine();
    let cleaner = SemanticCleanerImpl::from_parts(port, ModelConfig::default());
    assert_non_generic_cleaner(&cleaner);

    let erased: Arc<dyn SemanticCleaner> = Arc::new(cleaner);
    assert!(erased.is_ready());

    assert!(engine.is_ready());
    assert_eq!(engine.embedding_dim(), 384);
}

/// Default Pool mode: the cleaner's ONE shared engine + tokenizer back BOTH
/// downstream ports — vault-search embedding and Tier 2 semantic repair —
/// with no concrete `InferencePool` type anywhere in the wiring.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_cleaner_serves_vault_embedding_and_tier2_from_one_engine() {
    let (port, engine, tokenizer) = mock_port_and_engine();
    let cleaner = SemanticCleanerImpl::from_parts(port, ModelConfig::default());
    let cleaner: Arc<dyn SemanticCleaner> = Arc::new(cleaner);
    assert!(cleaner.is_ready());

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
            std::path::PathBuf::from(UNLOADABLE_MODEL_PATH),
            AiModel::Granite97M,
        )
        .expect("pool creation must succeed even with an unloadable model file"),
    );
    let tokenizer = in_memory_mini_lm();

    // No type annotation needed: a concrete `Arc<InferencePool>` value
    // unsized-coerces to `Arc<dyn InferenceEngine + Send + Sync>` at both call
    // sites (`Arc::clone` results must first land in a local, since the
    // generic `clone` blocks coercion inference).
    let pool_for_adapter = Arc::clone(&pool);
    let _adapter = EmbeddingAdapter::new(pool_for_adapter, Arc::clone(&tokenizer));
    let _inspector = GraniteDomInspector::new(pool, tokenizer, 0.75);
}

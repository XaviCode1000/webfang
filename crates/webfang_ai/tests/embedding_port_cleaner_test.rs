//! Cleaner-over-port seam (ADR-0004, Tramo D slice B).
//!
//! The semantic cleaner no longer owns an inference engine and a tokenizer:
//! it holds an `Arc<dyn EmbeddingPort>` and embeds each chunk through it.
//! That is what makes the cleaner reachable over the ALREADY-COMPILED domain
//! port — the seam ADR-0004 exists to open — and this file is its proof.
//!
//! Everything here is deterministic: a hand-written [`EmbeddingPort`] returns
//! FIXED vectors keyed by chunk text. No ONNX model, no ORT session, no
//! tokenizer, no worker threads, no network. The chunk count is derived from
//! the real `HtmlChunker` (the same one `clean()` uses) instead of being
//! hard-coded, so the assertions survive chunker tuning.
//!
//! This file is the EVIDENCE that the seam is real (ADR-0004, slice C): it
//! carries no `#[cfg(feature = "ai")]` gate, because it needs none. The
//! cleaner holds an erased `EmbeddingPort` and no engine, so a
//! `cargo test -p webfang_ai` with no features runs the whole file. Before
//! slice C it was gated, and a build without `ai` ran ZERO of these tests —
//! which is precisely the gap: the port seam could only be proven by a build
//! that already carried ONNX.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use webfang_ai::infrastructure_ai::content_pruner::{ContentPruner, LegibleContentPruner};
use webfang_ai::infrastructure_ai::{HtmlChunker, ModelConfig, SemanticCleanerImpl};
use webfang_core::domain::embedding_port::EmbeddingPort;
use webfang_core::domain::semantic_cleaner::SemanticCleaner;
use webfang_core::error::SemanticError;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Deterministic [`EmbeddingPort`]: one fixed vector per known text, plus a
/// fallback for anything unexpected.
///
/// Records every embedded text in call order under a mutex so the test can
/// assert the cleaner embeds each chunk exactly once, through the port, in
/// chunk order.
struct FakeEmbeddingPort {
    vectors: HashMap<String, Vec<f32>>,
    fallback: Vec<f32>,
    recorder: Arc<Mutex<Vec<String>>>,
}

/// A fake port plus the call log the test reads after the cleaner owns it.
type FakePort = (Arc<dyn EmbeddingPort>, Arc<Mutex<Vec<String>>>);

impl FakeEmbeddingPort {
    /// Port answering `[1.0]` for every text in `known` (and the same for
    /// anything else).
    fn uniform(known: &[String]) -> Self {
        let vectors = known.iter().map(|t| (t.clone(), vec![1.0f32])).collect();
        Self {
            vectors,
            fallback: vec![1.0f32],
            recorder: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Same, but `outlier_text` is answered with `[-1.0]` — the negation of
    /// every other chunk's vector.
    fn with_negated_outlier(known: &[String], outlier_text: &str) -> Self {
        let mut port = Self::uniform(known);
        port.vectors.insert(outlier_text.to_string(), vec![-1.0f32]);
        port
    }

    /// Erase into the port the cleaner consumes, handing the call log back to
    /// the caller so it can assert on the embedded texts after the fact.
    fn erased(self) -> FakePort {
        let recorder = Arc::clone(&self.recorder);
        (Arc::new(self), recorder)
    }
}

impl EmbeddingPort for FakeEmbeddingPort {
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
        if let Ok(mut calls) = self.recorder.lock() {
            calls.push(text.to_string());
        }
        let vector = self
            .vectors
            .get(text)
            .cloned()
            .unwrap_or_else(|| self.fallback.clone());
        Box::pin(async move { Ok(vector) })
    }

    fn embedding_dim(&self) -> usize {
        1
    }
}

/// Six block-separated paragraphs, each comfortably under the chunker's
/// 512-character packing ceiling, so `clean()` sees six chunks.
const PARAGRAPHS: [&str; 6] = [
    "The first paragraph carries enough prose to stand on its own as a single semantic block inside the article body of this fixture, without merging into its neighbour during chunk packing.",
    "The second paragraph carries enough prose to stand on its own as a single semantic block inside the article body of this fixture, without merging into its neighbour during chunk packing.",
    "The third paragraph carries enough prose to stand on its own as a single semantic block inside the article body of this fixture, without merging into its neighbour during chunk packing.",
    "The fourth paragraph carries enough prose to stand on its own as a single semantic block inside the article body of this fixture, without merging into its neighbour during chunk packing.",
    "The fifth paragraph carries enough prose to stand on its own as a single semantic block inside the article body of this fixture, without merging into its neighbour during chunk packing.",
    "The sixth paragraph carries enough prose to stand on its own as a single semantic block inside the article body of this fixture, without merging into its neighbour during chunk packing.",
];

const URL: &str = "https://example.com/embedding-port";

/// HTML built from [`PARAGRAPHS`].
fn fixture_html() -> String {
    let body: String = PARAGRAPHS.iter().map(|p| format!("<p>{p}</p>")).collect();
    format!("<html><body><article>{body}</article></body></html>")
}

/// The chunk list `clean()` is expected to embed, derived from the real
/// pruner + chunker instead of hard-coded, so the assertions stay honest when
/// either is retuned.
fn expected_chunk_texts() -> Vec<String> {
    let html = fixture_html();
    let pruned = LegibleContentPruner::standard().prune(&html);
    let effective = if pruned.is_empty() {
        html.as_str()
    } else {
        pruned.as_str()
    };
    HtmlChunker::new()
        .chunk(effective)
        .expect("chunking the fixture must succeed")
        .iter()
        .map(|c| c.content.clone())
        .collect()
}

/// Snapshot of the port's call log.
fn recorded_calls(recorder: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    recorder.lock().map(|c| c.clone()).unwrap_or_default()
}

/// Every chunk is embedded exactly once, through the port, in chunk order —
/// and its vector comes back on the chunk (`embeddings: Some(..)`).
///
/// This is the contract that replaced `tokenize` + `infer` inside the
/// cleaner: the pipeline order is still prune → chunk → guard → embed →
/// filter, but the embed step is now one port call per chunk.
#[tokio::test]
async fn clean_embeds_each_chunk_once_through_the_port_in_order() {
    let texts = expected_chunk_texts();
    assert!(
        texts.len() >= 2,
        "the fixture must produce at least two chunks to prove ordering; got {}",
        texts.len()
    );

    let (port, recorder) = FakeEmbeddingPort::uniform(&texts).erased();
    let cleaner = SemanticCleanerImpl::from_parts(
        port,
        ModelConfig::default()
            .with_relevance_threshold(0.5)
            .expect("0.5 is a valid threshold"),
    );

    let chunks = cleaner
        .clean(URL, &fixture_html())
        .await
        .expect("clean must succeed over the fake port");

    assert!(
        !chunks.is_empty(),
        "clean must return chunks for a substantive document"
    );

    // One port call per chunk, in chunk order — `clean()` fans the calls out
    // with `try_join_all`, which preserves order in its results, and the
    // relevance filter only removes entries, so the surviving calls are a
    // prefix-preserving subsequence of the chunk list.
    let recorded = recorded_calls(&recorder);
    assert_eq!(
        recorded, texts,
        "clean must embed every chunk exactly once, in chunk order, through the port"
    );

    for chunk in &chunks {
        let embedding = chunk
            .embeddings
            .as_ref()
            .expect("every returned chunk must carry its embedding");
        assert_eq!(
            embedding,
            &vec![1.0f32],
            "the chunk must carry the vector the port returned for it"
        );
        assert!(
            texts.contains(&chunk.content),
            "returned chunk `{}` must come from the chunked document",
            chunk.content
        );
    }
}

/// Relevance filtering is driven by the vectors the PORT returns, not by
/// anything the cleaner computes locally.
///
/// Every chunk is embedded as `[1.0]` except the LAST one, which is `[-1.0]`
/// — the exact negation of every other vector. With `n` chunks the
/// mean-pooled centroid is a positive multiple of `[1.0]` (for `n >= 3`), so
/// cosine similarity is `+1.0` for the first `n - 1` chunks and `-1.0` for the
/// outlier; the Z-scores are therefore `|d - d̄| / σ` with a single large
/// outlier and `n - 1` identical small ones. For every `n >= 3` the outlier's
/// Z is at least `1.414` while the others never exceed `0.707`, so the
/// `z_limit = 3 * (1 - 0.7) = 0.9` of a `0.7` threshold sits strictly between
/// them: the outlier is dropped, every other chunk survives, and order holds.
#[tokio::test]
async fn relevance_filter_drops_the_statistical_outlier_chunk() {
    let texts = expected_chunk_texts();
    assert!(
        texts.len() >= 3,
        "the Z-score argument above needs at least three chunks; the fixture \
         produced {} — update the arithmetic in this test if the chunker changed",
        texts.len()
    );
    let outlier = texts.last().expect("non-empty chunk list").clone();

    let (port, _recorder) = FakeEmbeddingPort::with_negated_outlier(&texts, &outlier).erased();
    let cleaner = SemanticCleanerImpl::from_parts(
        port,
        ModelConfig::default()
            .with_relevance_threshold(0.7)
            .expect("0.7 is a valid threshold"),
    );

    let chunks = cleaner
        .clean(URL, &fixture_html())
        .await
        .expect("clean must succeed over the fake port");

    let expected_survivors = &texts[..texts.len() - 1];
    let actual: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let expected: Vec<&str> = expected_survivors.iter().map(String::as_str).collect();

    assert_eq!(
        actual, expected,
        "only the negated (outlier) chunk must be filtered, and order must survive"
    );
    assert!(
        !chunks.iter().any(|c| c.content == outlier),
        "the outlier chunk must not survive the relevance filter"
    );
}

/// The chunk-size guard is a CHARACTER budget now: a chunk longer than
/// `max_chars` fails with [`SemanticError::ChunkTooLarge`] carrying the
/// character count, before the port is ever asked to embed it.
#[tokio::test]
async fn chunk_guard_rejects_chunks_over_the_char_budget() {
    let texts = expected_chunk_texts();
    let over_limit = 8usize;
    let (port, recorder) = FakeEmbeddingPort::uniform(&texts).erased();
    let cleaner =
        SemanticCleanerImpl::from_parts(port, ModelConfig::default().with_max_chars(over_limit));

    let result = cleaner.clean(URL, &fixture_html()).await;
    let err = match result {
        Ok(_) => panic!("a chunk over the character budget must be rejected"),
        Err(e) => e,
    };

    match err {
        SemanticError::ChunkTooLarge {
            chunk_id,
            chars,
            max,
        } => {
            assert_eq!(
                max, over_limit,
                "the error must report the configured budget"
            );
            assert!(
                chars > over_limit,
                "the error must report the offending character count; got {chars}"
            );
            assert!(
                chunk_id.starts_with("chunk-"),
                "the error must name the offending chunk; got {chunk_id}"
            );
        },
        other => panic!("expected ChunkTooLarge, got {other}"),
    }

    assert!(
        recorded_calls(&recorder).is_empty(),
        "the guard must run BEFORE any embedding: a rejected chunk is never sent to the port"
    );
}

/// A cleaner exists only if its port was built successfully, so `is_ready()`
/// is unconditionally true and the implementation still erases to
/// `Arc<dyn SemanticCleaner>`.
#[tokio::test]
async fn cleaner_is_ready_and_erases_to_the_domain_trait() {
    let (port, _recorder) = FakeEmbeddingPort::uniform(&[]).erased();
    let cleaner = SemanticCleanerImpl::from_parts(port, ModelConfig::default());

    assert!(
        cleaner.is_ready(),
        "a constructed cleaner is ready: there is no second lazy-load state"
    );

    let erased: Arc<dyn SemanticCleaner> = Arc::new(cleaner);
    assert!(erased.is_ready());
}

/// `chars_per_token` is the documented conversion ratio between the deprecated
/// `--max-tokens` budget and the character budget it becomes. Non-positive
/// ratios are rejected at build time, never at clean time.
///
/// The DEFAULT comes from `webfang_core`'s SSOT constant, because
/// `webfang_core` translates the legacy flag value with it: two independent
/// `3.0` literals would make `--max-tokens 4096` and `--max-chars 4096` mean
/// different budgets. This is the only place both crates are visible.
#[test]
fn chars_per_token_defaults_to_the_core_sso_constant() {
    let sso = webfang_core::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN;
    assert_eq!(
        ModelConfig::default().chars_per_token,
        sso,
        "ModelConfig::default() must read the SSOT constant, not its own literal"
    );
    assert_eq!(
        webfang_core::domain::options_spec::ai::max_tokens_to_max_chars(32_768),
        ModelConfig::default().max_chars,
        "the retired 32 768-token ceiling and the default character budget must \
         describe the same effective guard"
    );
}

/// `chars_per_token` rejects non-positive ratios.
#[test]
fn chars_per_token_rejects_non_positive_ratios() {
    assert!(
        ModelConfig::default().with_chars_per_token(3.5).is_ok(),
        "a positive ratio must be accepted"
    );
    assert!(
        ModelConfig::default().with_chars_per_token(0.0).is_err(),
        "a zero ratio must be rejected"
    );
    assert!(
        ModelConfig::default().with_chars_per_token(-1.0).is_err(),
        "a negative ratio must be rejected"
    );
}

//! Semantic cleaner implementation — Infrastructure layer (ADR-0004 paso 6).
//!
//! Implements the sealed [`SemanticCleaner`] domain trait on top of an erased
//! [`EmbeddingPort`], replacing the ONNX-tied implementation that lived in
//! `webfram_ai` (deleted in ADR-0004 paso 6). The cleaner itself holds no
//! inference concerns: it extracts text, chunks by paragraph, embeds through
//! the port, and filters by relevance — the port decides whether the backend
//! is a remote OpenAI-compatible endpoint or any future local stack.
//!
//! # Pipeline
//!
//! 1. Extract readable text from HTML (`legible`, tag-strip fallback)
//! 2. Split into paragraph chunks under `--max-chars` (a single oversized
//!    paragraph fails with [`SemanticError::ChunkTooLarge`])
//! 3. Embed the batch through the port; a batch failure degrades by
//!    bisecting the batch until the failing content is isolated
//! 4. Filter by Z-score relevance against the centroid (behavior ported
//!    verbatim from the pre-ADR implementation, #648)
//! 5. Return `DocumentChunk`s whose `embeddings` field carries the vector

use uuid::Uuid;

use crate::domain::embedding_port::EmbeddingPort;
use crate::domain::entities::content::DocumentChunk;
use crate::domain::semantic_cleaner::private::Sealed;
use crate::domain::semantic_cleaner::SemanticCleaner;
use crate::error::SemanticError;

/// Threshold at which filtering is reported as aggressive (>50 % discarded).
const AGGRESSIVE_FILTER_RATIO: f64 = 0.5;

/// Semantic cleaner over an erased [`EmbeddingPort`] (ADR-0004 paso 6).
///
/// Clonable: the cloned cleaner shares the same port `Arc`.
#[derive(Clone)]
pub struct SemanticCleanerImpl {
    embedding_port: std::sync::Arc<dyn EmbeddingPort>,
    max_chars: usize,
    relevance_threshold: f32,
}

impl SemanticCleanerImpl {
    /// Build a cleaner over `embedding_port`.
    ///
    /// * `max_chars` — chunk-rejection budget in characters (`--max-chars`)
    /// * `relevance_threshold` — adaptive filter strictness in `0..1`
    ///   (`--threshold`, 0.3 default)
    #[must_use]
    pub fn new(
        embedding_port: std::sync::Arc<dyn EmbeddingPort>,
        max_chars: usize,
        relevance_threshold: f32,
    ) -> Self {
        Self {
            embedding_port,
            max_chars,
            relevance_threshold,
        }
    }

    /// Embed `contents` through the port, degrading on batch failure by
    /// bisecting the batch until the failing content is isolated.
    ///
    /// A single [`EmbeddingPort::embed_batch`] call is preferred (one
    /// round-trip for remote ports). On failure with more than one content,
    /// the batch is split in two halves and each half is retried recursively;
    /// a single content that still fails propagates the error, because the
    /// cleaner cannot honestly synthesize an embedding it never obtained.
    async fn embed_with_bisection(
        &self,
        contents: &[String],
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        // `Box::pin` breaks the infinitely-sized future a recursive async fn
        // would otherwise require (rustc E0733).
        Box::pin(self.embed_batch_raw(contents)).await
    }

    /// Raw batch call through the port with bisect-on-failure degradation.
    async fn embed_batch_raw(&self, contents: &[String]) -> Result<Vec<Vec<f32>>, SemanticError> {
        match self.embedding_port.embed_batch(contents).await {
            Ok(vectors) => Ok(vectors),
            Err(_) if contents.len() > 1 => {
                let mid = contents.len() / 2;
                let (left, right) = contents.split_at(mid);
                let mut left_emb = Box::pin(self.embed_batch_raw(left)).await?;
                let mut right_emb = Box::pin(self.embed_batch_raw(right)).await?;
                left_emb.append(&mut right_emb);
                Ok(left_emb)
            },
            Err(e) => Err(e),
        }
    }

    /// Filter chunks by relevance and preserve every kept embedding.
    ///
    /// Scores each chunk's distance to the mean-pooled centroid of all
    /// embeddings and drops statistical outliers (Z-score adaptive
    /// thresholding, ported verbatim from the pre-ADR-0004 implementation,
    /// #648). Emits a `warn!` when more than half of the chunks are
    /// discarded — possible over-aggressive threshold or an off-topic page.
    fn filter_by_relevance(
        &self,
        url: &str,
        chunks: Vec<DocumentChunk>,
        embeddings: Vec<Vec<f32>>,
    ) -> Result<Vec<DocumentChunk>, SemanticError> {
        if chunks.len() != embeddings.len() {
            return Err(SemanticError::Inference(format!(
                "Length mismatch: got {} chunks but {} embedding vectors. \
                 Each chunk must have exactly one embedding vector.",
                chunks.len(),
                embeddings.len()
            )));
        }
        let distances = distances_to_centroid(&embeddings)?;
        let chunks_before = chunks.len();
        let kept = z_score_filter(chunks, embeddings, self.relevance_threshold, &distances);

        warn_aggressive_filter(url, chunks_before, kept.len());

        Ok(kept)
    }
}

/// Mean-pooled centroid of `embeddings`, then the distance
/// (`1 - cosine similarity`) of every vector to it.
fn distances_to_centroid(embeddings: &[Vec<f32>]) -> Result<Vec<f32>, SemanticError> {
    let dim = embeddings.first().map_or(0, Vec::len);
    if dim == 0 {
        return Err(SemanticError::Inference(
            "No embeddings available for relevance scoring".to_string(),
        ));
    }
    let mut centroid = vec![0.0_f32; dim];
    for embedding in embeddings {
        for (&val, slot) in embedding.iter().zip(centroid.iter_mut()) {
            *slot += val;
        }
    }
    let n = embeddings.len() as f32;
    for val in &mut centroid {
        *val /= n;
    }
    Ok(embeddings
        .iter()
        .map(|emb| 1.0 - cosine_similarity(emb, &centroid))
        .collect())
}

/// Z-score adaptive filtering: chunks whose centroid distance sits more than
/// `z_limit` deviations from the mean distance are statistical outliers and
/// are dropped; survivors carry their embedding in `embeddings`.
///
/// Thresholds map to strictness: `1.0` → Z=0 (centroid-only), `0.0` → Z=3
/// (~99.7 % kept), the `0.3` default → Z=2.1.
fn z_score_filter(
    chunks: Vec<DocumentChunk>,
    embeddings: Vec<Vec<f32>>,
    relevance_threshold: f32,
    distances: &[f32],
) -> Vec<DocumentChunk> {
    let sample_count = distances.len() as f32;
    let mean_dist = distances.iter().copied().sum::<f32>() / sample_count;
    let variance = distances
        .iter()
        .map(|d| (d - mean_dist).powi(2))
        .sum::<f32>()
        / sample_count;
    let std_dev = variance.sqrt();
    let z_limit = 3.0 * (1.0 - relevance_threshold);

    chunks
        .into_iter()
        .zip(embeddings)
        .zip(distances.iter())
        .filter(|(_, &distance)| {
            let z_score = (distance - mean_dist).abs() / std_dev.max(1e-6);
            z_score <= z_limit
        })
        .map(|((mut chunk, embedding), _)| {
            chunk.embeddings = Some(embedding);
            chunk
        })
        .collect()
}

/// `warn!` when the filter discards more than half of the chunks, so an
/// over-aggressive threshold or off-topic page is queryable from the trace.
fn warn_aggressive_filter(url: &str, chunks_before: usize, chunks_after: usize) {
    let filtered_out = chunks_before.saturating_sub(chunks_after);
    let loss_ratio = filtered_out as f64 / chunks_before as f64;
    if chunks_before > 0 && loss_ratio > AGGRESSIVE_FILTER_RATIO {
        tracing::warn!(
            url = %url,
            chunks_before = chunks_before,
            chunks_after = chunks_after,
            filtered_out = filtered_out,
            loss_ratio = format!("{:.0}%", loss_ratio * 100.0),
            "Relevance filter discarded >50% of chunks for this page — possible over-aggressive threshold"
        );
    }
}

/// Cosine similarity of two non-empty vectors; zero-norm degenerates to 0.0.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// Extract readable text from `html`, falling back to a tag-strip when
/// `legible` fails or yields nothing but whitespace.
fn extract_text_content(html: &str) -> String {
    match legible::parse(html, None, None) {
        Ok(article) if !article.content.trim().is_empty() => article.content,
        _ => strip_html_tags(html),
    }
}

/// Strip HTML tags, returning only text content.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut inside_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            c if !inside_tag => result.push(c),
            _ => {},
        }
    }
    result
}

/// Split `text` into chunks joined from non-empty paragraphs, staying under
/// `max_chars` when possible.
///
/// A single paragraph longer than the budget cannot be split honestly, so it
/// fails with [`SemanticError::ChunkTooLarge`] — the caller layer degrades
/// the page instead of writing an unembeddable chunk.
fn split_into_chunks(text: &str, max_chars: usize) -> Result<Vec<String>, SemanticError> {
    let paragraphs: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for paragraph in paragraphs {
        if !current.is_empty() && current.len() + paragraph.len() + 1 > max_chars {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(paragraph);
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    for (chunk_id, chunk) in chunks.iter().enumerate() {
        if chunk.len() > max_chars {
            return Err(SemanticError::ChunkTooLarge {
                chunk_id: chunk_id.to_string(),
                chars: chunk.len(),
                max: max_chars,
            });
        }
    }

    Ok(chunks)
}

impl Sealed for SemanticCleanerImpl {}

impl SemanticCleaner for SemanticCleanerImpl {
    fn clean<'a>(
        &'a self,
        url: &'a str,
        html: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a,
        >,
    > {
        tracing::debug!(
            url = %url,
            max_chars = self.max_chars,
            threshold = self.relevance_threshold,
            "Starting semantic clean (batch + bisect + z-score filter)"
        );

        Box::pin(async move {
            let text = extract_text_content(html);
            if text.trim().is_empty() {
                return Ok(Vec::new());
            }

            let contents = split_into_chunks(&text, self.max_chars)?;
            let embeddings = self.embed_with_bisection(&contents).await?;

            let chunks = contents
                .into_iter()
                .map(|content| DocumentChunk::new(Uuid::new_v4(), "", "", content))
                .collect::<Vec<_>>();

            self.filter_by_relevance(url, chunks, embeddings)
        })
    }

    fn max_chars(&self) -> usize {
        self.max_chars
    }

    /// Always `true`: the cleaner has no lazily-loadable state of its own. Its
    /// embedding port was built successfully before the cleaner was
    /// constructed, so a `SemanticCleanerImpl` exists only if the backend it
    /// embeds through is ready.
    fn is_ready(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::Relaxed;
    use std::sync::Arc;

    type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// Deterministic fake port: counts batch calls and returns fixed
    /// 3-dimensional vectors. `fail_when_len_gt` makes `embed_batch` fail
    /// (simulating a 413 payload-too-large) whenever the batch carries more
    /// items than that threshold; `None` never fails. One struct covers every
    /// failure policy the tests need, so there is a single source of truth
    /// for the trait ceremony.
    struct FakeEmbeddingPort {
        batch_calls: AtomicUsize,
        fail_when_len_gt: Option<usize>,
    }

    impl FakeEmbeddingPort {
        fn counting() -> Self {
            Self {
                batch_calls: AtomicUsize::new(0),
                fail_when_len_gt: None,
            }
        }

        fn failing_batches_larger_than(max_len: usize) -> Self {
            Self {
                batch_calls: AtomicUsize::new(0),
                fail_when_len_gt: Some(max_len),
            }
        }

        fn batch_calls(&self) -> usize {
            self.batch_calls.load(Relaxed)
        }
    }

    impl EmbeddingPort for FakeEmbeddingPort {
        fn embed<'a>(&'a self, _text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
            Box::pin(async move { Ok(vec![1.0, 2.0, 3.0]) })
        }

        fn embed_batch<'a>(
            &'a self,
            texts: &'a [String],
        ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
            self.batch_calls.fetch_add(1, Relaxed);
            let fail_gt = self.fail_when_len_gt;
            Box::pin(async move {
                if texts.len() > fail_gt.unwrap_or(usize::MAX) {
                    return Err(SemanticError::Inference(
                        "el endpoint de embeddings devolvió el estado 413".to_string(),
                    ));
                }
                Ok(texts.iter().map(|_| vec![1.0, 2.0, 3.0]).collect())
            })
        }

        fn embedding_dim(&self) -> usize {
            3
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// Fixed-answer mock used for the shape-level tests below: one fixed
    /// 3-dimensional vector per input text.
    struct MockEmbeddingPort;

    impl MockEmbeddingPort {
        fn new() -> Self {
            Self
        }
    }

    impl EmbeddingPort for MockEmbeddingPort {
        fn embed<'a>(&'a self, _text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
            Box::pin(async move { Ok(vec![1.0, 0.0, 0.0]) })
        }

        fn embed_batch<'a>(
            &'a self,
            texts: &'a [String],
        ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
            let embeddings = texts.iter().map(|_| vec![1.0, 0.0, 0.0]).collect();
            Box::pin(async move { Ok(embeddings) })
        }

        fn embedding_dim(&self) -> usize {
            3
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn clean_returns_chunks_with_embeddings() {
        // Every chunk produced from real HTML must come out of the filter
        // carrying its embedding and nonempty content. (The exact chunk count
        // depends on `legible`'s text normalization — asserted in
        // `split_into_chunks_*` below.)
        let port = Arc::new(MockEmbeddingPort::new());
        let cleaner = SemanticCleanerImpl::new(port, 1000, 0.3);
        let html = "<p>First paragraph</p><p>Second paragraph</p>";
        let chunks = cleaner.clean("https://example.com", html).await.unwrap();
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(
                chunk.embeddings.is_some(),
                "every returned chunk must carry its embedding"
            );
            assert!(!chunk.content.is_empty());
        }
    }

    #[tokio::test]
    async fn clean_handles_empty_html() {
        let port = Arc::new(MockEmbeddingPort::new());
        let cleaner = SemanticCleanerImpl::new(port, 1000, 0.3);
        let chunks = cleaner.clean("https://example.com", "").await.unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn clean_respects_max_chars() {
        let port = Arc::new(MockEmbeddingPort::new());
        let cleaner = SemanticCleanerImpl::new(port, 10, 0.3);
        let html = "<p>This paragraph is way too long for the limit</p>";
        let result = cleaner.clean("https://example.com", html).await;
        assert!(
            matches!(result, Err(SemanticError::ChunkTooLarge { .. })),
            "an oversized paragraph must fail with ChunkTooLarge, got a {:?}",
            result.map(|_| "Ok(...)")
        );
    }

    #[test]
    fn split_into_chunks_accumulates_paragraphs_under_budget() {
        let chunks = split_into_chunks("First paragraph\nSecond paragraph", 1000)
            .expect("valid-sized text must chunk");
        assert_eq!(chunks.len(), 1, "paragraphs accumulate into one window");
        assert_eq!(chunks[0], "First paragraph\nSecond paragraph");
    }

    #[test]
    fn split_into_chunks_splits_when_budget_forces_flush() {
        let chunks = split_into_chunks("first\nsecond\nthird", 15).expect("chunks");
        assert_eq!(
            chunks,
            vec!["first\nsecond".to_string(), "third".to_string()]
        );
    }

    // ADR-0004 S8: one `embed_batch` call batch over total contents (never
    // one `embed` per chunk — `embed` stays untouched by the cleaner).
    #[tokio::test]
    async fn embed_uses_single_embed_batch() {
        let port = Arc::new(FakeEmbeddingPort::counting());
        let contents = vec!["uno".to_string(), "dos".to_string(), "tres".to_string()];

        let embeddings = cleaner_with_port(port.clone(), 1000, 0.3)
            .embed_with_bisection(&contents)
            .await
            .expect("embedded content batch");
        assert_eq!(embeddings.len(), 3);
        assert_eq!(port.batch_calls(), 1);
    }

    // ADR-0004 bisection: a batch failure (p. ej. HTTP 413 por payload grande)
    // se recupera troceando la tanda hasta aislar el contenido defectuoso.
    #[tokio::test]
    async fn embed_bisects_batch_on_failure() {
        // Falla sólo cuando el batch trae más de un contenido: simula un 413
        // por payload grande que desaparece al trocear.
        let port = Arc::new(FakeEmbeddingPort::failing_batches_larger_than(1));
        let contents = vec!["uno".to_string(), "dos".to_string()];

        let embeddings = cleaner_with_port(port.clone(), 1000, 0.3)
            .embed_with_bisection(&contents)
            .await
            .expect("clean must recover by bisecting the batch");

        assert_eq!(embeddings.len(), 2, "both contents survive bisection");
        assert_eq!(
            port.batch_calls(),
            3,
            "one failed batch of 2 + two recovered batches of 1"
        );
    }

    // Si ni siquiera un contenido aislado se puede embedder, el error se
    // propaga: nunca silencio.
    #[tokio::test]
    async fn embed_propagates_when_every_item_fails() {
        let port = Arc::new(FakeEmbeddingPort::failing_batches_larger_than(0));
        let contents = vec!["uno".to_string(), "dos".to_string()];

        let result = cleaner_with_port(port.clone(), 1000, 0.3)
            .embed_with_bisection(&contents)
            .await;

        assert!(result.is_err(), "must fail when no content can be embedded");
        assert!(port.batch_calls() > 1, "bisection must be attempted first");
    }

    /// Build a cleaner directly over a fake port (helpers keep the arrange
    /// boilerplate out of the clippy duplication ratchet).
    fn cleaner_with_port(
        port: Arc<FakeEmbeddingPort>,
        max_chars: usize,
        threshold: f32,
    ) -> SemanticCleanerImpl {
        SemanticCleanerImpl::new(port as Arc<dyn EmbeddingPort>, max_chars, threshold)
    }

    #[test]
    fn cleaner_impl_is_send_and_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<SemanticCleanerImpl>();
        assert_sync::<SemanticCleanerImpl>();
    }
}

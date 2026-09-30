//! AI PHASE PROFILE — per-phase cost of the real AI pipeline (issue #1618).
//!
//! ## What this measures
//!
//! A **phase profile** answers one question: for a realistic page, where does
//! the wall time actually go? It decomposes the `--clean-ai` path into the
//! stages production really runs, in pipeline order, each timed against the
//! PRODUCTION type (no re-implementation):
//!
//! | phase | production seam | scope |
//! |:--|:--|:--|
//! | `prune+chunk` | `LegibleContentPruner` + `HtmlChunker` | per page |
//! | `model_load` | `PooledInferenceEngine::open` | per session set |
//! | `tokenizer_load` | `MiniLmTokenizer::from_file` | once |
//! | `tokenize` | `MiniLmTokenizer::tokenize` | per chunk |
//! | `infer` | `PooledInferenceEngine::infer` via `join_all` | per chunk |
//! | `embed_postprocess` | `mean_pool` + `l2_normalize_safe` | per chunk |
//! | `persist` | `StreamRepository` as a `VectorRepository` | per chunk |
//!
//! ## Why it exists
//!
//! The pool-size decision (N) of issue #1456 was taken against a MOCK with a
//! hardcoded 45 ms per-inference latency
//! (`crates/webfang_ai/tests/mock_inference_benchmark.rs`). A per-phase
//! profile on a real model is the evidence that replaces it (#1618
//! PERF-EVID-2): it shows how much of a page's AI cost is model inference and
//! how much is everything around it.
//!
//! ## Ground rules
//!
//! * **Real ONNX model, read-only from the local HuggingFace cache.** Never a
//!   download, never a mock, never a synthetic latency. When the cache is
//!   absent the bench prints one `SKIP` line and PASSES — it can never fail
//!   because a model is missing, and CI never depends on these numbers.
//! * **Asserts nothing about timing.** The only assertions are structural
//!   (embedding width, count, unit norm, one JSONL line per chunk). No number
//!   in the table is a gate; a slow machine and a fast machine both pass.
//! * **Numbers are comparable only within one machine, run and shape.** The
//!   header prints the variant, the corpus size, the pool size and the Tokio
//!   worker count precisely so a reader can tell when two runs are NOT
//!   comparable.
//!
//! ## Own `main`, not criterion
//!
//! `criterion` is not a dev-dependency of this crate and adding dependencies is
//! out of scope (#1618). A timing-only criterion group also cannot express
//! "build a real ORT session, load a real tokenizer, persist real embeddings".
//! So this target is `harness = false`: `cargo bench` runs its binary
//! directly, and the binary measures and prints.
//!
//! ## Reporting surfaces
//!
//! The **printed table on stdout is the reporting surface**. In addition, each
//! phase emits one structured `tracing::info!` event (English field names) so a
//! host that installs its own subscriber can capture them. This target
//! deliberately does NOT install a subscriber: `tracing-subscriber` is not
//! available here, and a bench that installs one would leak global state into
//! any harness that runs beside it.
//!
//! ## How to run
//!
//! ```text
//! cargo bench -p webfang_ai --features ai --bench ai_phase_profile
//! ```
//!
//! Expect roughly one to two minutes on a 16-core workstation with the default
//! corpus: three reps of 393 real ONNX inferences, plus one ORT session per
//! pool size in the marginal table.
//!
//! Env knobs (all optional):
//!
//! * `WEBFANG_AI_PHASE_PROFILE_VARIANT` — `97m` (default) | `311m`.
//! * `WEBFANG_AI_PHASE_PROFILE_PARAGRAPHS` — default `400` (the issue's
//!   ~393-chunk page shape; same fixture as
//!   `crates/webfang_ai/tests/p0_001_common.rs`).
//! * `WEBFANG_AI_PHASE_PROFILE_REPS` — default `3`.
//! * `WEBFANG_AI_PHASE_PROFILE_POOL_SIZES` — default `1,2,3,4` (the per-N
//!   model-load marginal table).

/// No-op entry point when the `ai` feature is off.
///
/// The nightly bench lane (`.github/workflows/benches.yml`) runs a bare
/// `cargo bench --locked` with no `--all-features`, so this target is built
/// there. Without the feature there is no inference stack to measure, so it
/// exits 0 having done nothing — zero CI cost, and CI never depends on the
/// numbers (see the module docs).
#[cfg(not(feature = "ai"))]
fn main() {}

/// Real entry point: `cargo bench` runs this target's binary directly
/// (`harness = false`, because `criterion` is not a dev-dependency of this
/// crate and adding one is out of scope for #1618).
#[cfg(feature = "ai")]
fn main() {
    profile::run();
}

/// The measurement itself, behind the `ai` feature.
#[cfg(feature = "ai")]
mod profile {
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use futures::future::join_all;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;
    use tracing::info;
    use webfang_ai::infrastructure_ai::embedding_ops::{l2_normalize_safe, mean_pool};
    use webfang_ai::infrastructure_ai::{
        AiModel, ContentPruner, EngineConfig, HtmlChunker, InferenceEngine, LegibleContentPruner,
        MiniLmTokenizer, ModelInput, PooledInferenceEngine,
    };
    use webfang_core::domain::repository::VectorRepository;
    use webfang_core::domain::DocumentChunk;
    use webfang_core::infrastructure::stream::{SinkPath, StreamRepository};

    /// Graceful-skip marker, mirroring the `SKIP <name>: …` convention of the
    /// P0-001 harness files so the two are greppable with the same pattern.
    const SKIP_PREFIX: &str = "SKIP ai_phase_profile";

    /// Env knob: model variant (`97m` | `311m`).
    const ENV_VARIANT: &str = "WEBFANG_AI_PHASE_PROFILE_VARIANT";
    /// Env knob: paragraphs per synthetic page.
    const ENV_PARAGRAPHS: &str = "WEBFANG_AI_PHASE_PROFILE_PARAGRAPHS";
    /// Env knob: repetitions per timed phase.
    const ENV_REPS: &str = "WEBFANG_AI_PHASE_PROFILE_REPS";
    /// Env knob: comma-separated pool sizes for the model-load marginal table.
    const ENV_POOL_SIZES: &str = "WEBFANG_AI_PHASE_PROFILE_POOL_SIZES";

    /// Sentence repeated inside every synthetic paragraph. Copied verbatim from
    /// `synthetic_page()` in `crates/webfang_ai/tests/p0_001_common.rs:29-39` so
    /// the corpus shape is the one the N decision was taken on.
    const SENTENCE: &str = "hello world hello world hello world hello world hello world. ";

    /// URL used by the persist phase. It must be a real fetchable `ValidUrl` —
    /// the JSONL sink validates the dedup key and the URL at its boundary.
    const SAMPLE_URL: &str = "https://example.com/ai-phase-profile";

    // ---------------------------------------------------------------------------
    // Configuration
    // ---------------------------------------------------------------------------

    /// One synthetic page: an article of identical paragraphs, each sized to fill
    /// roughly one chunk (512-char cap). The same shape as `p0_001_common`.
    fn synthetic_page(paragraphs: usize) -> String {
        let mut html = String::from("<html><body><article>");
        for i in 0..paragraphs {
            html.push_str(&format!("<p>Párrafo {i}: {}</p>", SENTENCE.repeat(6)));
        }
        html.push_str("</article></body></html>");
        html
    }

    /// Parse a positive integer env knob, falling back to `default` on absence or
    /// on any unparsable/non-positive value (a knob typo degrades to the default
    /// instead of failing a measurement run).
    fn env_usize(name: &str, default: usize) -> usize {
        match std::env::var(name) {
            Ok(raw) => match raw.trim().parse::<usize>() {
                Ok(value) if value > 0 => value,
                _ => default,
            },
            Err(_) => default,
        }
    }

    /// Parse the pool-size list (`1,2,3,4`). Unparsable entries are dropped and a
    /// list that ends up empty falls back to `1,2,3,4` so the marginal table is
    /// never silently empty.
    fn env_pool_sizes() -> Vec<usize> {
        let parsed: Vec<usize> = std::env::var(ENV_POOL_SIZES)
            .ok()
            .map(|raw| {
                raw.split(',')
                    .filter_map(|part| part.trim().parse::<usize>().ok())
                    .filter(|size| *size > 0)
                    .collect()
            })
            .unwrap_or_default();
        if parsed.is_empty() {
            vec![1, 2, 3, 4]
        } else {
            parsed
        }
    }

    /// Resolve the model variant from the env knob.
    fn env_variant() -> AiModel {
        match std::env::var(ENV_VARIANT).ok().as_deref().map(str::trim) {
            Some("311m") => AiModel::Granite311M,
            _ => AiModel::Granite97M,
        }
    }

    // ---------------------------------------------------------------------------
    // Asset discovery (offline, read-only, never downloads)
    //
    // Deliberately local rather than `#[path]`-included from `tests/`: a
    // `#[path]` reaching outside the package root is rejected by `cargo package`
    // verification, and this bench must stay packageable.
    // ---------------------------------------------------------------------------

    /// HF cache root: `$HF_HUB_CACHE`, else `$HOME/.cache/huggingface/hub`.
    fn hub_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("HF_HUB_CACHE") {
            let path = PathBuf::from(dir);
            if path.is_dir() {
                return Some(path);
            }
        }
        std::env::var("HOME")
            .ok()
            .map(|home| PathBuf::from(home).join(".cache/huggingface/hub"))
            .filter(|path| path.is_dir())
    }

    /// Locate the cached `(model, tokenizer.json)` pair for a variant,
    /// read-only: scan `<hub>/models--<owner>--<name>/snapshots/*/` for the first
    /// snapshot that carries both files. Returns `None` (→ graceful skip) when
    /// anything is missing. Never downloads, never writes to the cache.
    ///
    /// `variant.model_file()` already carries its own `onnx/` prefix
    /// (`cache_config::DEFAULT_MODEL_FILE`), so it is joined onto the snapshot
    /// root directly. Prefixing it a second time is the bug this note exists to
    /// prevent: the discovery finds nothing and degrades to a skip.
    fn discover_assets(variant: AiModel) -> Option<(PathBuf, PathBuf)> {
        let root = hub_root()?;
        // Cache dir name: "models--" + repo_id with every '/' replaced by "--".
        let repo_dir = format!("models--{}", variant.repo_id().replace('/', "--"));
        let snapshots = root.join(repo_dir).join("snapshots");
        let entries = std::fs::read_dir(&snapshots).ok()?;
        for entry in entries.flatten() {
            let model = entry.path().join(variant.model_file());
            let tokenizer = entry.path().join("tokenizer.json");
            if model.is_file() && tokenizer.is_file() {
                return Some((model, tokenizer));
            }
        }
        None
    }

    /// Directory the discovery looked in, echoed by the skip message. A silent
    /// skip with no path is undiagnosable when the cache layout moves.
    fn search_root(variant: AiModel) -> String {
        let repo_dir = format!("models--{}", variant.repo_id().replace('/', "--"));
        match hub_root() {
            Some(root) => root.join(repo_dir).join("snapshots").display().to_string(),
            None => format!(
                "<sin raiz de cache: ni $HF_HUB_CACHE ni $HOME/.cache/huggingface/hub: {repo_dir}>"
            ),
        }
    }

    // ---------------------------------------------------------------------------
    // Statistics (local, no external crate)
    // ---------------------------------------------------------------------------

    /// min / median / p90 / max over a set of samples, in milliseconds.
    struct Stats {
        count: usize,
        min_ms: f64,
        median_ms: f64,
        p90_ms: f64,
        max_ms: f64,
    }

    impl Stats {
        /// Build from raw samples. `nearest-rank` on the sorted sample vector, so
        /// the median of an even-sized set is the upper of the two middle ranks
        /// (never an interpolation invented here).
        fn from(samples: &[Duration]) -> Self {
            assert!(!samples.is_empty(), "a phase must have at least one sample");
            let mut sorted: Vec<f64> = samples.iter().map(as_ms_f64).collect();
            sorted.sort_by(f64::total_cmp);
            let last = sorted.len() - 1;
            Self {
                count: sorted.len(),
                min_ms: sorted[0],
                median_ms: sorted[last / 2],
                p90_ms: sorted[last * 9 / 10],
                max_ms: sorted[last],
            }
        }
    }

    /// Duration → milliseconds as `f64`.
    fn as_ms_f64(duration: &Duration) -> f64 {
        duration.as_secs_f64() * 1_000.0
    }

    // ---------------------------------------------------------------------------
    // Reporting
    // ---------------------------------------------------------------------------

    /// One row of the phase table. `per_unit` is the median normalized to the
    /// phase's own unit, pre-formatted with its unit label (`us/chunk`,
    /// `ms/page`).
    struct PhaseRow {
        phase: &'static str,
        scope: String,
        stats: Stats,
        per_unit: String,
    }

    impl PhaseRow {
        /// Print the row and emit the matching structured `tracing` event.
        /// English field names; the printed table remains the reporting surface
        /// (no subscriber is installed here — see the module docs).
        fn emit(&self) {
            println!(
                "{:<18} {:<24} {:>7} {:>10} {:>10} {:>10} {:>10} {:>18}",
                self.phase,
                self.scope,
                self.stats.count,
                fmt_ms(self.stats.min_ms),
                fmt_ms(self.stats.median_ms),
                fmt_ms(self.stats.p90_ms),
                fmt_ms(self.stats.max_ms),
                self.per_unit,
            );
            info!(
                phase = self.phase,
                scope = self.scope.as_str(),
                samples = self.stats.count,
                min_ms = self.stats.min_ms,
                median_ms = self.stats.median_ms,
                p90_ms = self.stats.p90_ms,
                max_ms = self.stats.max_ms,
                "ai_phase_profile"
            );
        }
    }

    /// Fixed-3-decimals millisecond cell.
    fn fmt_ms(value: f64) -> String {
        format!("{value:.3}")
    }

    /// Header of the phase table.
    fn print_table_header() {
        println!(
            "{:<18} {:<24} {:>7} {:>10} {:>10} {:>10} {:>10} {:>18}",
            "phase", "scope", "samples", "min_ms", "median_ms", "p90_ms", "max_ms", "per_unit"
        );
        println!("{}", "-".repeat(115));
    }

    // ---------------------------------------------------------------------------
    // Phase 1 — prune + chunk (per page, one-time CPU)
    // ---------------------------------------------------------------------------

    /// DOM prune + semantic chunking of one synthetic page, `reps` times.
    /// Returns the chunks of the LAST rep (identical across reps by
    /// determinism) plus the samples.
    fn measure_prune_chunk(paragraphs: usize, reps: usize) -> (Vec<DocumentChunk>, Vec<Duration>) {
        let html = synthetic_page(paragraphs);
        let pruner = LegibleContentPruner::standard();
        let chunker = HtmlChunker::new();
        let mut samples = Vec::with_capacity(reps);
        let mut chunks = Vec::new();
        for _ in 0..reps {
            let started = Instant::now();
            let pruned = pruner.prune(&html);
            let page = chunker
                .chunk(&pruned)
                .expect("el troceado de la página sintética debe funcionar");
            samples.push(started.elapsed());
            chunks = page;
        }
        (chunks, samples)
    }

    // ---------------------------------------------------------------------------
    // Phase 2 — model load (per session set, one-time)
    // ---------------------------------------------------------------------------

    /// Time `PooledInferenceEngine::open` for each N. Returns `(N, total_ms)`
    /// in the requested order. Each engine is dropped before the next N, so the
    /// peak footprint is one pool, not the sum of all of them.
    fn measure_model_load(
        model_path: &Path,
        variant: AiModel,
        pool_sizes: &[usize],
    ) -> Vec<(usize, f64)> {
        pool_sizes
            .iter()
            .map(|size| {
                let n = NonZeroUsize::new(*size).expect("los tamaños de pool deben ser > 0");
                let started = Instant::now();
                let engine = PooledInferenceEngine::open(model_path, variant, n)
                    .expect("el modelo en caché debe abrir sesiones ORT");
                let elapsed = started.elapsed();
                assert_eq!(
                    engine.pool_size(),
                    *size,
                    "el pool debe reportar el tamaño solicitado"
                );
                drop(engine);
                (*size, as_ms_f64(&elapsed))
            })
            .collect()
    }

    /// Print the per-N model-load table plus the marginal cost of going N→N+1.
    /// The marginal of the FIRST N equals its total (there is no N=0).
    fn print_model_load_table(rows: &[(usize, f64)]) {
        println!();
        println!("model_load marginal (pool size N, one PooledInferenceEngine::open per N)");
        println!(
            "{:<8} {:>14} {:>18}",
            "pool_n", "total_ms", "marginal_ms(N)"
        );
        println!("{}", "-".repeat(42));
        let mut previous: Option<(usize, f64)> = None;
        for (n, total) in rows {
            let marginal = match previous {
                Some((_, prev_total)) => total - prev_total,
                None => *total,
            };
            let marginal_label = match previous {
                Some((prev_n, _)) => format!("{prev_n}->{n}"),
                None => format!("{n} (base)"),
            };
            println!(
                "{:<8} {:>14.3} {:>18}",
                n,
                total,
                format!("{marginal:.3} [{marginal_label}]")
            );
            previous = Some((*n, *total));
        }
    }

    // ---------------------------------------------------------------------------
    // Phase 3 — tokenizer load (one-time)
    // ---------------------------------------------------------------------------

    /// Time `MiniLmTokenizer::from_file` once and hand the tokenizer back so the
    /// later phases share this exact instance (loading it twice would report a
    /// number production pays only once).
    async fn measure_tokenizer_load(tokenizer_path: &Path) -> (MiniLmTokenizer, Duration) {
        let started = Instant::now();
        let tokenizer = MiniLmTokenizer::from_file(tokenizer_path)
            .await
            .expect("el tokenizer.json en caché debe cargar");
        (tokenizer, started.elapsed())
    }

    // ---------------------------------------------------------------------------
    // Phase 4 — tokenize (per chunk)
    // ---------------------------------------------------------------------------

    /// Tokenize every chunk of the page, `reps` times. Returns the samples
    /// (total per page) and the `ModelInput`s of the LAST rep, reused verbatim by
    /// the infer and postprocess phases so tokenization is not re-counted there.
    fn measure_tokenize(
        tokenizer: &MiniLmTokenizer,
        chunks: &[DocumentChunk],
        reps: usize,
    ) -> (Vec<Duration>, Vec<ModelInput>) {
        let mut samples = Vec::with_capacity(reps);
        let mut inputs = Vec::with_capacity(chunks.len());
        for _ in 0..reps {
            let started = Instant::now();
            let batch: Vec<ModelInput> = chunks
                .iter()
                .map(|chunk| {
                    tokenizer
                        .tokenize(&chunk.content)
                        .expect("la tokenización de un chunk debe funcionar")
                })
                .collect();
            samples.push(started.elapsed());
            inputs = batch;
        }
        (samples, inputs)
    }

    // ---------------------------------------------------------------------------
    // Phase 5 — infer (per chunk, the expensive phase)
    // ---------------------------------------------------------------------------

    /// One rep of the production inference fan-out: every chunk CONCURRENTLY
    /// through `join_all`, exactly as
    /// `crates/webfang_ai/src/infrastructure_ai/semantic_cleaner_impl.rs:503-509`
    /// does with `try_join_all`. Returns the rep's wall time and its embeddings.
    async fn infer_one_rep(
        engine: &PooledInferenceEngine,
        inputs: &[ModelInput],
    ) -> (Duration, Vec<Vec<f32>>) {
        let started = Instant::now();
        let embeddings = join_all(inputs.iter().map(|input| engine.infer(input)))
            .await
            .into_iter()
            .map(|result| result.expect("la inferencia de un chunk debe funcionar"))
            .collect::<Vec<Vec<f32>>>();
        (started.elapsed(), embeddings)
    }

    /// `reps` reps of [`infer_one_rep`]. Returns the samples and the embeddings of
    /// the LAST rep (needed by the postprocess and persist phases).
    async fn measure_infer(
        engine: &PooledInferenceEngine,
        inputs: &[ModelInput],
        reps: usize,
    ) -> (Vec<Duration>, Vec<Vec<f32>>) {
        let mut samples = Vec::with_capacity(reps);
        let mut embeddings = Vec::new();
        for _ in 0..reps {
            let (elapsed, produced) = infer_one_rep(engine, inputs).await;
            samples.push(elapsed);
            embeddings = produced;
        }
        (samples, embeddings)
    }

    // ---------------------------------------------------------------------------
    // Phase 6 — embed postprocess (per chunk, ISOLATED)
    // ---------------------------------------------------------------------------

    /// Deterministic filler for the `(seq_len x embedding_dim)` token-embedding
    /// slab fed to the pure arithmetic pooling ops. The VALUES are irrelevant:
    /// `mean_pool` and `l2_normalize_safe` do not read data beyond the attention
    /// mask, and the mask here is the real one from the [`ModelInput`]. This
    /// measures SHAPE, not model output — and it replaces nothing the real
    /// inference already measured in phase 5.
    fn fill_slab(slab: &mut [f32]) {
        for (index, slot) in slab.iter_mut().enumerate() {
            // Cheap, deterministic, branch-free, and never all-zero (an all-zero
            // slab would short-circuit `l2_normalize_safe` and measure nothing).
            *slot = ((index % 977) as f32) * 1e-3 - 0.5;
        }
    }

    /// Measure mean-pooling + Matryoshka truncation + L2 normalization in
    /// isolation, at the exact pipeline shapes (each chunk's own `seq_len`).
    ///
    /// `run_session_inference` fuses ORT's `session.run` with this tail and ORT
    /// exposes no timer that separates them, so the tail is timed on its own
    /// here. `samples` are per page; the returned vectors are the last rep's
    /// normalized+truncated embeddings (used by the structural assertions).
    fn measure_embed_postprocess(
        inputs: &[ModelInput],
        variant: AiModel,
        reps: usize,
    ) -> (Vec<Duration>, Vec<Vec<f32>>) {
        let native_dim = variant.embedding_dim();
        let slabs: Vec<(usize, Vec<i64>, Vec<f32>)> = inputs
            .iter()
            .map(|input| {
                let seq_len = input.seq_len();
                let mut slab = vec![0.0f32; seq_len * native_dim];
                fill_slab(&mut slab);
                (seq_len, input.attention_mask.clone(), slab)
            })
            .collect();

        let mut samples = Vec::with_capacity(reps);
        let mut normalized = Vec::new();
        for _ in 0..reps {
            let started = Instant::now();
            let batch: Vec<Vec<f32>> = slabs
                .iter()
                .map(|(seq_len, mask, slab)| {
                    let pooled = mean_pool(slab, *seq_len, native_dim, mask);
                    // Matryoshka truncation, same order as production:
                    // mean_pool -> take(output_dim) -> l2_normalize_safe.
                    let truncated: Vec<f32> =
                        pooled.iter().take(variant.output_dim()).copied().collect();
                    l2_normalize_safe(&truncated)
                })
                .collect();
            samples.push(started.elapsed());
            normalized = batch;
        }
        (samples, normalized)
    }

    // ---------------------------------------------------------------------------
    // Phase 7 — persist (per chunk)
    // ---------------------------------------------------------------------------

    /// Persist the REAL embeddings from phase 5 through the production JSONL
    /// sink: one `save_resource` then one `save_chunk` per chunk, writing into a
    /// `TempDir`. Returns the total elapsed and the resource URL the sink
    /// resolved (its `ValidUrl` rendering, reused for every chunk).
    async fn persist_once(
        repo: &StreamRepository,
        chunks: &[DocumentChunk],
        embeddings: &[Vec<f32>],
    ) -> (Duration, String) {
        let started = Instant::now();
        let resource_url = repo
            .save_resource(SAMPLE_URL, "ai_phase_profile", &content_hash(b"page"), 0)
            .await
            .expect("el recurso debe guardarse en el sink JSONL");
        for (index, (chunk, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            // The sink derives the dedup key from the segment before the first
            // '-', so the id must be "{64-hex}-{index}" exactly as
            // `ElasticIngestion::run` formats it.
            let id = format!("{}-{}", content_hash(chunk.content.as_bytes()), index);
            let boxed = repo.save_chunk(
                &id,
                &resource_url,
                index as i64,
                &chunk.content,
                Some(embedding),
            );
            boxed
                .await
                .expect("el chunk debe guardarse en el sink JSONL");
        }
        (started.elapsed(), resource_url)
    }

    /// Real lowercase SHA-256 hex of `payload` (the sink validates the shape).
    fn content_hash(payload: &[u8]) -> String {
        format!("{:x}", Sha256::digest(payload))
    }

    /// `reps` reps of [`persist_once`] over a fresh `TempDir` file each. Returns
    /// the samples and the number of non-empty lines the last rep produced.
    async fn measure_persist(
        chunks: &[DocumentChunk],
        embeddings: &[Vec<f32>],
        reps: usize,
    ) -> (Vec<Duration>, usize) {
        let mut samples = Vec::with_capacity(reps);
        let mut lines = 0;
        for _ in 0..reps {
            let dir = TempDir::new().expect("el TempDir del sink debe crearse");
            let path = dir.path().join("vectors.jsonl");
            let repo = StreamRepository::new(SinkPath::File(path.clone()))
                .expect("el sink JSONL debe abrirse");
            let (elapsed, _) = persist_once(&repo, chunks, embeddings).await;
            drop(repo);
            samples.push(elapsed);
            lines = count_non_empty_lines(&path);
        }
        (samples, lines)
    }

    /// Count non-empty lines in a file (a blank trailing line is not a record).
    fn count_non_empty_lines(path: &Path) -> usize {
        let body = std::fs::read_to_string(path).expect("el archivo JSONL debe poder leerse");
        body.lines().filter(|line| !line.trim().is_empty()).count()
    }

    // ---------------------------------------------------------------------------
    // Header + structural assertions (never timing)
    // ---------------------------------------------------------------------------

    /// Everything a reader needs to decide whether two runs are comparable.
    /// Grouped into one struct (rather than nine parameters) because this IS the
    /// run identity: two runs are comparable only when every field matches.
    struct RunShape<'a> {
        variant: AiModel,
        model_path: &'a Path,
        tokenizer_path: &'a Path,
        paragraphs: usize,
        chunk_count: usize,
        reps: usize,
        pool_n: usize,
        intra_threads: usize,
        workers: usize,
    }

    /// Print the run identity above the phase table.
    fn print_header(shape: &RunShape<'_>) {
        println!("ai_phase_profile — real Granite ONNX model, read-only from the local HF cache");
        println!(
            "  variant        : {} ({}d native -> {}d out)",
            shape.variant.display_name(),
            shape.variant.embedding_dim(),
            shape.variant.output_dim()
        );
        println!("  model file     : {}", shape.model_path.display());
        println!("  tokenizer file : {}", shape.tokenizer_path.display());
        println!(
            "  corpus         : {} paragraphs -> {} chunks (one page)",
            shape.paragraphs, shape.chunk_count
        );
        println!(
            "  reps           : {} per timed phase (median is the headline)",
            shape.reps
        );
        println!(
            "  pool           : N={} (EngineConfig::default_pool_size), intra_threads={}",
            shape.pool_n, shape.intra_threads
        );
        println!("  tokio workers  : {}", shape.workers);
        println!(
            "  NOTE           : numbers are machine-specific and are asserted NOWHERE; \
             compare runs only on the same machine, model and shape."
        );
        println!();
    }

    /// The only assertions in this bench — all structural. A slow machine and a
    /// fast machine both pass; none of these can be moved by timing.
    fn assert_structural(
        variant: AiModel,
        chunk_count: usize,
        inputs: &[ModelInput],
        embeddings: &[Vec<f32>],
        normalized: &[Vec<f32>],
        jsonl_lines: usize,
    ) {
        let expected = variant.output_dim();
        assert_eq!(
            embeddings.len(),
            chunk_count,
            "cada chunk debe producir exactamente un embedding"
        );
        for (index, embedding) in embeddings.iter().enumerate() {
            assert_eq!(
                embedding.len(),
                expected,
                "el embedding {index} debe tener {expected} dimensiones"
            );
        }
        assert_eq!(
            inputs.len(),
            chunk_count,
            "cada chunk debe tokenizarse en exactamente un ModelInput"
        );
        for (index, vector) in normalized.iter().enumerate() {
            let magnitude = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!(
                (magnitude - 1.0).abs() < 1e-3,
                "el embedding normalizado {index} debe tener magnitud 1.0 (fue {magnitude})"
            );
        }
        assert_eq!(
            jsonl_lines, chunk_count,
            "el sink JSONL debe escribir exactamente una línea por chunk"
        );
    }

    // ---------------------------------------------------------------------------
    // The profile
    // ---------------------------------------------------------------------------

    /// Everything the measurement phases produce, gathered so the reporting code
    /// never has to juggle a wide tuple.
    struct Profile {
        /// Per-N `model_load` totals, in the requested order.
        load_rows: Vec<(usize, f64)>,
        /// Intra-op thread budget each session was built with.
        intra_threads: usize,
        tokenizer_load: Duration,
        tokenize: Vec<Duration>,
        inputs: Vec<ModelInput>,
        infer: Vec<Duration>,
        embeddings: Vec<Vec<f32>>,
        postprocess: Vec<Duration>,
        normalized: Vec<Vec<f32>>,
        persist: Vec<Duration>,
        jsonl_lines: usize,
    }

    /// Build a per-chunk table row: the headline is the per-page median, and
    /// `per_unit` normalizes it to microseconds per chunk. One-time phases use
    /// [`one_time_row`] instead, because labelling a one-time cost "per page"
    /// would invite a reader to add it up per page.
    fn chunk_row(
        phase: &'static str,
        scope: &str,
        samples: &[Duration],
        chunk_count: usize,
    ) -> PhaseRow {
        let stats = Stats::from(samples);
        let per_chunk = stats.median_ms * 1_000.0 / chunk_count.max(1) as f64;
        PhaseRow {
            phase,
            scope: scope.to_string(),
            stats,
            per_unit: format!("{per_chunk:.1} us/chunk"),
        }
    }

    /// Build a one-time table row (a cost production pays once per run, not per
    /// page): `per_unit` states the cost as a whole number of milliseconds.
    fn one_time_row(phase: &'static str, scope: &str, sample: Duration) -> PhaseRow {
        let stats = Stats::from(std::slice::from_ref(&sample));
        let per_unit = format!("{:.3} ms one-time", stats.median_ms);
        PhaseRow {
            phase,
            scope: scope.to_string(),
            stats,
            per_unit,
        }
    }

    /// Pick the pool size the `infer` phase runs at: the production default when
    /// it is part of the measured list, otherwise the largest measured N (so the
    /// headline figure always rests on a real, measured load).
    fn resolve_pool_n(pool_sizes: &[usize]) -> usize {
        let default_n = EngineConfig::default_pool_size().get();
        if pool_sizes.contains(&default_n) {
            default_n
        } else {
            pool_sizes.iter().copied().max().unwrap_or(default_n)
        }
    }

    /// `PooledInferenceEngine::open` total for a given N, as a single sample so
    /// it can share the table-row shape. `0.0` when N was not measured.
    fn single_load_sample(rows: &[(usize, f64)], n: usize) -> Duration {
        let ms = rows
            .iter()
            .find(|(size, _)| *size == n)
            .map_or(0.0, |(_, total)| *total);
        Duration::from_secs_f64(ms / 1_000.0)
    }

    /// Run every phase that needs the Tokio runtime, in pipeline order.
    async fn run_runtime_phases(
        model_path: &Path,
        tokenizer_path: &Path,
        variant: AiModel,
        pool_sizes: &[usize],
        pool_n: usize,
        chunks: &[DocumentChunk],
        reps: usize,
    ) -> Profile {
        // Phase 2 runs BEFORE the inference reps on purpose: the weights are
        // paged in here, so no later phase pays first-touch page faults that
        // production pays once at engine construction.
        let load_rows = measure_model_load(model_path, variant, pool_sizes);
        let (tokenizer, tokenizer_load) = measure_tokenizer_load(tokenizer_path).await;
        let (tokenize, inputs) = measure_tokenize(&tokenizer, chunks, reps);

        let size = NonZeroUsize::new(pool_n).expect("el tamaño de pool debe ser > 0");
        let engine = PooledInferenceEngine::open(model_path, variant, size)
            .expect("el motor de inferencia agrupada debe abrirse");
        let intra_threads = engine.intra_threads();
        let (infer, embeddings) = measure_infer(&engine, &inputs, reps).await;
        drop(engine);

        let (postprocess, normalized) = measure_embed_postprocess(&inputs, variant, reps);
        let (persist, jsonl_lines) = measure_persist(chunks, &embeddings, reps).await;

        Profile {
            load_rows,
            intra_threads,
            tokenizer_load,
            tokenize,
            inputs,
            infer,
            embeddings,
            postprocess,
            normalized,
            persist,
            jsonl_lines,
        }
    }

    /// Entry point: run the whole profile and print the table. Called by the
    /// crate's `main`, which is gated on the `ai` feature.
    pub fn run() {
        let variant = env_variant();
        let paragraphs = env_usize(ENV_PARAGRAPHS, 400);
        let reps = env_usize(ENV_REPS, 3);
        let pool_sizes = env_pool_sizes();

        let Some((model_path, tokenizer_path)) = discover_assets(variant) else {
            println!(
                "{SKIP_PREFIX}: no hay modelo/tokenizer en la caché local para {} \
                 (buscado en {}; sin descargas, sin mocks)",
                variant.display_name(),
                search_root(variant)
            );
            return;
        };

        let (chunks, prune_chunk) = measure_prune_chunk(paragraphs, reps);
        let chunk_count = chunks.len();
        let pool_n = resolve_pool_n(&pool_sizes);

        let workers = std::thread::available_parallelism().map_or(1, |value| value.get());
        // Built explicitly rather than via `#[tokio::test(worker_threads = N)]`,
        // which requires a LITERAL; the value is reported in the header so the
        // fan-out is never silently assumed.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .enable_all()
            .build()
            .expect("el runtime multi-hilo debe construirse");

        let profile = runtime.block_on(run_runtime_phases(
            &model_path,
            &tokenizer_path,
            variant,
            &pool_sizes,
            pool_n,
            &chunks,
            reps,
        ));

        assert_structural(
            variant,
            chunk_count,
            &profile.inputs,
            &profile.embeddings,
            &profile.normalized,
            profile.jsonl_lines,
        );

        print_header(&RunShape {
            variant,
            model_path: &model_path,
            tokenizer_path: &tokenizer_path,
            paragraphs,
            chunk_count,
            reps,
            pool_n,
            intra_threads: profile.intra_threads,
            workers,
        });
        print_table_header();
        chunk_row("prune+chunk", "per page", &prune_chunk, chunk_count).emit();
        one_time_row(
            "model_load",
            &format!("per session set (N={pool_n})"),
            single_load_sample(&profile.load_rows, pool_n),
        )
        .emit();
        one_time_row("tokenizer_load", "one-time", profile.tokenizer_load).emit();
        chunk_row("tokenize", "per page", &profile.tokenize, chunk_count).emit();
        chunk_row("infer", "per page (join_all)", &profile.infer, chunk_count).emit();
        chunk_row(
            "embed_postprocess",
            "per page (ISOLATED)",
            &profile.postprocess,
            chunk_count,
        )
        .emit();
        chunk_row("persist", "per page (JSONL)", &profile.persist, chunk_count).emit();
        println!();
        println!(
            "  * embed_postprocess is ALREADY CONTAINED inside the infer figure: the tail is reported \
             to SIZE the residual, NOT to be added to it."
        );
        print_model_load_table(&profile.load_rows);
    }
}

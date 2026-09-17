//! P0-001 MEASURE (issue #1456): engine sweep `Single` vs `Pool{N}`.
//!
//! BENCH-style harness, not a CI gate: the parent sweep
//! [`p0_001_measure_sweep`] is `#[ignore]`-gated, runs REAL models only from
//! the read-only local HuggingFace cache (offline, never downloads), and
//! skips gracefully when the cache is absent.
//!
//! Matrix: engine configs {Single baseline, Pool{2,4,8}(+Pool{15} when the
//! RAM gate passes)} × pages {1,2,4,8} of synthetic ~153KB content mirroring
//! the issue baseline (~393 chunks/page with the paso-0 fixture shape). Per
//! cell (3 repetitions, median reported): tiempo AI, speedup vs 1 page of the
//! same config AND vs Single-1page, plus peak RSS DELTA.
//!
//! RSS methodology (the earlier probe's monotonic-HWM caveat must not
//! repeat): EVERY cell runs in a FRESH child process (the same test binary
//! re-executed with `WEBFANG_P0_001_CHILD` set). Inside the child, VmHWM is
//! sampled right after model load and again after the reps; the delta is
//! peak-minus-post-load WITHIN that single cell's lifetime, so no earlier
//! cell can contaminate it. `/usr/bin/time -v` was rejected: it reports the
//! whole-process peak including model weights, not the inference delta.
//!
//! Run in RELEASE (dev numbers are order-of-magnitude only, as the batch
//! probe showed):
//!
//! ```bash
//! cargo test --release -p webfang_ai --all-features --test p0_001_measure \
//!     -- --ignored --nocapture
//! # 311m spot check of the chosen N (not the full matrix):
//! WEBFANG_P0_001_VARIANT=311m WEBFANG_P0_001_CONFIGS=single,pool4 \
//! WEBFANG_P0_001_PAGES=1,8 cargo test --release -p webfang_ai \
//!     --all-features --test p0_001_measure -- --ignored --nocapture
//! ```
//!
//! Env knobs: `WEBFANG_P0_001_VARIANT` (`97m` default, `311m`), `WEBFANG_P0_001_CONFIGS`
//! (default `single,pool2,pool4,pool8,pool15`), `WEBFANG_P0_001_PAGES` (default
//! `1,2,4,8`). Every spec reuses the production [`EngineConfig`] parser, so the
//! harness syntax can never drift from the shipped `WEBFANG_AI_ENGINE` syntax.
#![cfg(feature = "ai")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures::future::join_all;
use webfang_ai::infrastructure_ai::{
    AiModel, EngineConfig, MiniLmTokenizer, ModelConfig, PooledInferenceEngine, SemanticCleanerImpl,
};
use webfang_ai::SemanticCleaner;

/// Repetitions per cell; the reported time is the median.
const REPS: usize = 3;

/// Paragraphs per synthetic page. The chunker packs ≤512 chars per chunk, so
/// ~400 × ~380-char paragraphs land near ~393 chunks — the issue's 153KB shape
/// (same fixture as the paso-0 mock benchmark; the real tokenizer decides the
/// exact count, which the child reports per cell).
const PARAGRAPHS_PER_PAGE: usize = 400;

/// Child-mode env: `WEBFANG_P0_001_CHILD="<config>|<variant>|<pages>"`, e.g.
/// `"pool4|97m|8"`. The parent sweep re-executes this same test binary with it
/// set so every cell gets a fresh process (see module docs for why).
const CHILD_ENV: &str = "WEBFANG_P0_001_CHILD";

/// Marker prefix of the child's machine-readable result line on stdout.
const CELL_PREFIX: &str = "P0_001_CELL ";
/// Marker prefix of the child's graceful-skip line (cache absent, non-Linux…).
const SKIP_PREFIX: &str = "P0_001_SKIP ";

/// Tokio workers per child: fixed literal for every cell so Single and Pool
/// share the same fan-out executor (production runs `num_cpus` workers; this
/// machine has 16 — the value is reported per cell, not silently assumed).
/// A literal (not `const`) because `#[tokio::test(worker_threads)]` requires one.
const CHILD_WORKERS: usize = 16;

/// Closeness gate for the Single-vs-Pool parity test: identical weights plus
/// identical per-row summation order ⇒ only float-rounding diffs on
/// unit-scale vectors; 1e-5 is the same conservative slack the batch-parity
/// probe used (a pool session with `intra_threads > 1` may reduce in a
/// different order than the single session).
const PARITY_TOLERANCE: f32 = 1e-5;

// ---------------------------------------------------------------------------
// Shared helpers (parent + child)
// ---------------------------------------------------------------------------

/// One synthetic page: an article of identical paragraphs, each sized to fill
/// roughly one chunk (~380 chars < 512-char chunk cap). Same shape as the
/// paso-0 mock fixture so chunk counts stay comparable with that verdict.
fn synthetic_page() -> String {
    const SENTENCE: &str = "hello world hello world hello world hello world hello world. ";
    let mut html = String::from("<html><body><article>");
    for i in 0..PARAGRAPHS_PER_PAGE {
        html.push_str(&format!("<p>Párrafo {i}: {}</p>", SENTENCE.repeat(6)));
    }
    html.push_str("</article></body></html>");
    html
}

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

/// Locate the cached `(model.onnx, tokenizer.json)` pair for a variant,
/// read-only: scan `<hub>/models--ibm-granite--granite-embedding-<tag>-multilingual-r2/snapshots/*/`.
/// Returns `None` (→ graceful skip, never download) when anything is missing.
fn discover_assets(tag: &str) -> Option<(PathBuf, PathBuf)> {
    let root = hub_root()?;
    let model_dir = root.join(format!(
        "models--ibm-granite--granite-embedding-{tag}-multilingual-r2"
    ));
    let snapshots = model_dir.join("snapshots");
    let entries = std::fs::read_dir(&snapshots).ok()?;
    for entry in entries.flatten() {
        let model = entry.path().join("onnx/model.onnx");
        let tokenizer = entry.path().join("tokenizer.json");
        if model.is_file() && tokenizer.is_file() {
            return Some((model, tokenizer));
        }
    }
    None
}

/// Parse `97m` | `311m` into the model variant.
fn parse_variant(tag: &str) -> Option<AiModel> {
    match tag.trim() {
        "97m" => Some(AiModel::Granite97M),
        "311m" => Some(AiModel::Granite311M),
        _ => None,
    }
}

/// Parse one engine spec from the sweep `WEBFANG_P0_001_CONFIGS` list
/// (`single` | `pool2` …) via the production [`EngineConfig`] parser, so the
/// harness can never disagree with the shipped `WEBFANG_AI_ENGINE` syntax.
fn parse_config(spec: &str) -> Option<EngineConfig> {
    let spec = spec.trim();
    if spec == "single" {
        return Some(EngineConfig::Single);
    }
    if let Some(count) = spec.strip_prefix("pool") {
        return format!("pool:{count}").parse().ok();
    }
    None
}

/// Current process peak RSS in KiB (`VmHWM` from `/proc/self/status`).
/// `None` off Linux — the caller degrades to "no RSS" instead of failing.
fn peak_rss_kib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmHWM:") {
                return rest.split_whitespace().next()?.parse().ok();
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Available RAM in KiB (`MemAvailable` from `/proc/meminfo`). `None` off Linux.
fn mem_available_kib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let info = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in info.lines() {
            if let Some(rest) = line.strip_prefix("MemAvailable:") {
                return rest.split_whitespace().next()?.parse().ok();
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// RAM gate for pool configs: `size × model_bytes × 1.3` must fit in
/// `MemAvailable`, otherwise the cell is skipped with a printed reason
/// (15 × 311m ≈ 18 GiB of weights alone would OOM this class of machine).
/// Returns the skip reason, or `None` when the config may run.
fn ram_gate(config: &EngineConfig, model_bytes: u64) -> Option<String> {
    let size = match *config {
        EngineConfig::Single => return None,
        EngineConfig::Pool { size } => size.get() as u64,
    };
    let available = mem_available_kib()?;
    let need_kib = size * model_bytes * 13 / 10 / 1024;
    if need_kib > available {
        Some(format!(
            "RAM gate: pool{size} necesita ~{:.1} GiB (pesos × {size} × 1.3) \
             pero MemAvailable es {:.1} GiB — celda omitida para no OOM",
            need_kib as f64 / 1_048_576.0,
            available as f64 / 1_048_576.0
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Child: one cell (one config × one page-count) in a fresh process
// ---------------------------------------------------------------------------

/// Child worker: builds ONE engine, samples VmHWM post-load, runs `pages`
/// identical synthetic pages through `join_all(clean)` × [`REPS`], samples
/// VmHWM peak, prints one `P0_001_CELL …` line. Not `#[ignore]`: the parent
/// re-executes this binary with [`CHILD_ENV`] set; without it this is a no-op
/// pass so plain `cargo test` stays fast and offline-safe.
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
async fn p0_001_measure_child() {
    let spec = match std::env::var(CHILD_ENV) {
        Ok(spec) => spec,
        Err(_) => return,
    };
    let mut parts = spec.split('|');
    let (config_spec, variant_tag, pages_spec) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    let config = parse_config(config_spec).expect("config del hijo debe parsear");
    let variant = parse_variant(variant_tag).expect("variante del hijo debe ser 97m|311m");
    let pages: usize = pages_spec.parse().expect("pages del hijo debe ser entero");

    let Some((model_path, tokenizer_path)) = discover_assets(variant_tag) else {
        println!("{SKIP_PREFIX}reason=sin caché local para {variant_tag} (sin descargas)");
        return;
    };

    let model_config = ModelConfig::default()
        .with_model_variant(variant)
        .with_offline_mode(true);
    // Erased behind the `SemanticCleaner` trait: `Single` and `Pool` are
    // different concrete cleaners over the same pipeline, and only the
    // engine differs — which is exactly the variable under measurement.
    let cleaner: Arc<dyn SemanticCleaner> = match config {
        EngineConfig::Single => Arc::new(
            SemanticCleanerImpl::new(model_config)
                .await
                .expect("single offline debe construir"),
        ),
        EngineConfig::Pool { size } => {
            let engine = Arc::new(
                PooledInferenceEngine::open(&model_path, variant, size)
                    .expect("pool offline debe construir"),
            );
            let tokenizer = Arc::new(
                MiniLmTokenizer::from_file(&tokenizer_path)
                    .await
                    .expect("tokenizer en caché debe cargar"),
            );
            Arc::new(SemanticCleanerImpl::from_parts(
                engine,
                tokenizer,
                model_config,
            ))
        },
    };
    assert!(
        cleaner.is_ready(),
        "el cleaner del hijo debe reportar ready"
    );

    let hwm_load = peak_rss_kib().unwrap_or(0);

    // Reps: N identical pages through the same fan-out shape as
    // `export_flow::clean_all_pages` (`join_all` per page set).
    let html = synthetic_page();
    let mut times_s: Vec<f64> = Vec::with_capacity(REPS);
    let mut chunks_per_page = 0;
    for _ in 0..REPS {
        let urls: Vec<String> = (0..pages)
            .map(|i| format!("https://example.com/p0-001-c{i}"))
            .collect();
        let started = Instant::now();
        let results = join_all(urls.iter().map(|url| cleaner.clean(url.as_str(), &html))).await;
        times_s.push(started.elapsed().as_secs_f64());
        for (i, result) in results.iter().enumerate() {
            let chunks = result
                .as_ref()
                .unwrap_or_else(|e| panic!("clean de página {i} (N={pages}) falló: {e}"));
            assert!(
                !chunks.is_empty(),
                "la página sintética debe producir chunks (0 = el chunker/pruner se comió el fixture)"
            );
            chunks_per_page = chunks.len();
        }
    }

    let hwm_peak = peak_rss_kib().unwrap_or(0);
    let times = times_s
        .iter()
        .map(|t| format!("{t:.3}"))
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "{CELL_PREFIX}config={config_spec} variant={variant_tag} pages={pages} \
         chunks_per_page={chunks_per_page} times_s={times} \
         hwm_load_kib={hwm_load} hwm_peak_kib={hwm_peak} workers={CHILD_WORKERS}"
    );
}

// ---------------------------------------------------------------------------
// Parent: the sweep (#[ignore]-gated BENCH)
// ---------------------------------------------------------------------------

/// One finished cell: median time + RSS delta, both from a fresh process.
struct CellResult {
    config_spec: String,
    pages: usize,
    chunks_per_page: usize,
    median_s: f64,
    rss_delta_mib: f64,
}

/// Comma list from env, or the default.
fn env_list(key: &str, default: &str) -> Vec<String> {
    std::env::var(key)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Median of a non-empty sample.
fn median(mut sample: Vec<f64>) -> f64 {
    sample.sort_by(|a, b| a.total_cmp(b));
    sample[sample.len() / 2]
}

/// Run one cell in a fresh child process; parse its `P0_001_CELL` line.
/// Returns `None` when the child skips gracefully (cache absent).
fn run_cell(
    test_exe: &std::path::Path,
    config_spec: &str,
    variant_tag: &str,
    pages: usize,
) -> Option<CellResult> {
    let output = std::process::Command::new(test_exe)
        .args(["--exact", "p0_001_measure_child", "--nocapture"])
        .env(CHILD_ENV, format!("{config_spec}|{variant_tag}|{pages}"))
        .output()
        .expect("el binario de tests hijo debe ejecutarse");
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(line) = stdout.lines().find(|l| l.starts_with(CELL_PREFIX)) {
        let mut chunks = 0;
        let mut times: Vec<f64> = Vec::new();
        let mut load = 0u64;
        let mut peak = 0u64;
        for token in line[CELL_PREFIX.len()..].split_whitespace() {
            let (key, value) = token.split_once('=').unwrap_or(("", ""));
            match key {
                "chunks_per_page" => chunks = value.parse().unwrap_or(0),
                "times_s" => {
                    times = value.split(',').filter_map(|t| t.parse().ok()).collect();
                },
                "hwm_load_kib" => load = value.parse().unwrap_or(0),
                "hwm_peak_kib" => peak = value.parse().unwrap_or(0),
                _ => {},
            }
        }
        assert!(
            output.status.success() && !times.is_empty() && chunks > 0,
            "celda {config_spec}×{pages}p: hijo sin tiempos válidos.\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Some(CellResult {
            config_spec: config_spec.to_string(),
            pages,
            chunks_per_page: chunks,
            median_s: median(times),
            rss_delta_mib: peak.saturating_sub(load) as f64 / 1024.0,
        });
    }
    if stdout.lines().any(|l| l.starts_with(SKIP_PREFIX)) {
        eprintln!("  SKIP {config_spec}×{pages}p ({variant_tag}): sin caché local — sin descargas");
        return None;
    }
    panic!(
        "celda {config_spec}×{pages}p: el hijo no emitió resultado.\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// P0-001 MEASURE sweep: `{Single,Pool{2,4,8}(+Pool{15})}` × `{1,2,4,8}`
/// pages on the selected variant (default 97m; 311m via
/// `WEBFANG_P0_001_VARIANT=311m` as a spot check, not the full matrix).
///
/// Prints the full median table (time, speedup vs 1 page same-config, speedup
/// vs Single-1page, RSS delta). Evidence only — no numeric assertions beyond
/// child success, so numbers can never turn this BENCH red.
#[test]
#[ignore = "BENCH P0-001 (issue #1456): modelos reales en release, ~decenas de minutos"]
fn p0_001_measure_sweep() {
    let variant_tag = std::env::var("WEBFANG_P0_001_VARIANT").unwrap_or_else(|_| "97m".to_string());
    assert!(
        parse_variant(&variant_tag).is_some(),
        "WEBFANG_P0_001_VARIANT debe ser 97m|311m, fue {variant_tag:?}"
    );
    let configs = env_list("WEBFANG_P0_001_CONFIGS", "single,pool2,pool4,pool8,pool15");
    let pages: Vec<usize> = env_list("WEBFANG_P0_001_PAGES", "1,2,4,8")
        .iter()
        .map(|s| s.parse().expect("pages deben ser enteros"))
        .collect();
    for spec in &configs {
        assert!(
            parse_config(spec).is_some(),
            "config desconocida en WEBFANG_P0_001_CONFIGS: {spec:?} \
             (válidas: single, pool<N> — la misma sintaxis de WEBFANG_AI_ENGINE)"
        );
    }

    let test_exe = std::env::current_exe().expect("current_exe del harness");
    let model_bytes = discover_assets(&variant_tag)
        .and_then(|(model, _)| std::fs::metadata(&model).ok().map(|m| m.len()));

    eprintln!(
        "\nP0-001 MEASURE: variante={variant_tag} configs={configs:?} pages={pages:?} \
         reps={REPS} (mediana) workers={CHILD_WORKERS}"
    );

    // (config, pages) → cell, in matrix order. Pool{15} (y cualquier pool que
    // no quepa) se omite con su justificación impresa — nunca OOM.
    let mut cells: Vec<CellResult> = Vec::new();
    for spec in &configs {
        let config = parse_config(spec).expect("configs validadas arriba");
        if let Some(model_bytes) = model_bytes {
            if let Some(reason) = ram_gate(&config, model_bytes) {
                eprintln!("  SKIP {spec} (todas las páginas): {reason}");
                continue;
            }
        }
        for &npages in &pages {
            if let Some(cell) = run_cell(&test_exe, spec, &variant_tag, npages) {
                eprintln!(
                    "  celda {}×{}p: mediana {:.3}s (chunks/página {}) RSS-delta {:.0} MiB",
                    cell.config_spec,
                    cell.pages,
                    cell.median_s,
                    cell.chunks_per_page,
                    cell.rss_delta_mib
                );
                cells.push(cell);
            }
        }
    }
    assert!(
        !cells.is_empty(),
        "barrido vacío: ¿caché de modelos ausente? (sin descargas por diseño)"
    );

    // Median table. speedup_same(N) = (T1 × N) / TN (convención del issue:
    // serial ⇒ 1×, perfectamente paralelo ⇒ N×); speedup_base contra
    // Single-1page para comparar configuraciones entre sí.
    let single_1 = cells
        .iter()
        .find(|c| c.config_spec == "single" && c.pages == pages[0])
        .map(|c| c.median_s)
        .unwrap_or(f64::NAN);
    eprintln!("\n| config | pages | mediana AI | speedup vs 1p (misma cfg) | speedup vs Single-1p | RSS-delta |");
    eprintln!("|---|---|---|---|---|---|");
    for cell in &cells {
        let t1_same = cells
            .iter()
            .find(|c| c.config_spec == cell.config_spec && c.pages == pages[0])
            .map(|c| c.median_s)
            .unwrap_or(f64::NAN);
        let speedup_same = (t1_same * cell.pages as f64) / cell.median_s;
        let speedup_base = (single_1 * cell.pages as f64) / cell.median_s;
        eprintln!(
            "| {} | {} | {:.3}s | {:.2}x | {:.2}x | {:.0} MiB |",
            cell.config_spec,
            cell.pages,
            cell.median_s,
            speedup_same,
            speedup_base,
            cell.rss_delta_mib
        );
    }
    eprintln!(
        "\nchunks/página: {} · workers tokio/hijo: {CHILD_WORKERS} · \
         criterio del issue: mínimo N con speedup(8) >= 6.0x dentro del presupuesto RSS de ops",
        cells[0].chunks_per_page
    );
}

// ---------------------------------------------------------------------------
// Correctness: Single-vs-Pool parity on the same corpus
// ---------------------------------------------------------------------------

/// Correctness gate for the MEASURE decision (issue #1456): the SAME synthetic
/// page through baseline `Single` and each `Pool{N}` must yield the same chunk
/// count and per-chunk embeddings within [`PARITY_TOLERANCE`] (bit-identical is
/// reported, not required: `intra_threads > 1` may reduce in another order).
/// Runs on `WEBFANG_P0_001_VARIANT` (default 97m), offline, skipping gracefully
/// without cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "correctitud P0-001: modelos reales, mismo corpus que el barrido"]
async fn p0_001_engine_parity() {
    use webfang_core::domain::DocumentChunk;

    let variant_tag = std::env::var("WEBFANG_P0_001_VARIANT").unwrap_or_else(|_| "97m".to_string());
    let variant = parse_variant(&variant_tag).expect("variante debe ser 97m|311m");
    let pool_specs = env_list("WEBFANG_P0_001_CONFIGS", "pool2,pool4,pool8");
    assert!(
        !pool_specs.is_empty(),
        "WEBFANG_P0_001_CONFIGS no debe estar vacía para el parity"
    );

    let Some((model_path, tokenizer_path)) = discover_assets(&variant_tag) else {
        eprintln!("SKIP parity ({variant_tag}): sin caché local — sin descargas");
        return;
    };

    let model_config = || {
        ModelConfig::default()
            .with_model_variant(variant)
            .with_offline_mode(true)
    };
    let baseline = SemanticCleanerImpl::new(model_config())
        .await
        .expect("single offline debe construir");
    let html = synthetic_page();
    let url = "https://example.com/p0-001-parity";
    let base_chunks: Vec<DocumentChunk> = baseline
        .clean(url, &html)
        .await
        .expect("clean baseline debe funcionar");
    assert!(
        !base_chunks.is_empty(),
        "el corpus parity debe producir chunks"
    );
    eprintln!(
        "parity baseline single ({}): {} chunks",
        variant_tag,
        base_chunks.len()
    );

    for spec in &pool_specs {
        let Some(EngineConfig::Pool { size }) = parse_config(spec) else {
            continue;
        };
        let mut skip_reason: Option<String> = None;
        if let Ok(meta) = std::fs::metadata(&model_path) {
            skip_reason = ram_gate(&EngineConfig::Pool { size }, meta.len());
        }
        if let Some(reason) = skip_reason {
            eprintln!("  SKIP {spec}: {reason}");
            continue;
        }
        let engine = Arc::new(
            PooledInferenceEngine::open(&model_path, variant, size).expect("pool debe abrir"),
        );
        let tokenizer = Arc::new(
            MiniLmTokenizer::from_file(&tokenizer_path)
                .await
                .expect("tokenizer debe cargar"),
        );
        let pooled = SemanticCleanerImpl::from_parts(engine, tokenizer, model_config());
        let pool_chunks = pooled
            .clean(url, &html)
            .await
            .expect("clean pool debe funcionar");
        assert_eq!(
            pool_chunks.len(),
            base_chunks.len(),
            "{spec}: mismo corpus debe dar mismos chunks ({} vs {})",
            pool_chunks.len(),
            base_chunks.len()
        );

        let mut max_diff = 0.0f32;
        let mut bitwise = true;
        for (base, other) in base_chunks.iter().zip(pool_chunks.iter()) {
            let base_emb = base
                .embeddings
                .as_ref()
                .expect("baseline debe preservar embeddings");
            let other_emb = other
                .embeddings
                .as_ref()
                .expect("pool debe preservar embeddings");
            assert_eq!(base_emb.len(), 384, "embedding debe ser 384-dim");
            assert_eq!(other_emb.len(), base_emb.len(), "dims deben coincidir");
            for (a, b) in base_emb.iter().zip(other_emb.iter()) {
                max_diff = max_diff.max((a - b).abs());
                bitwise &= a.to_bits() == b.to_bits();
            }
        }
        eprintln!("  parity {spec} vs single: max-abs-diff={max_diff:.2e} bit-idéntico={bitwise}");
        assert!(
            max_diff <= PARITY_TOLERANCE,
            "{spec}: diff máxima {max_diff:.2e} supera la tolerancia {PARITY_TOLERANCE:.0e}"
        );
    }
}

//! Shared fixtures for the P0-001 harness tests (issue #1456).
//!
//! Single home for the setup every P0-001 test file needs — synthetic pages,
//! the in-memory tokenizer, ORT session construction, RSS sampling, probe
//! setup — so the three harness files share code instead of mirroring it.
//! Dependency-free and offline-safe: no model downloads, no network.
//!
//! Included from each harness file with `#[path = "p0_001_common.rs"]`; the
//! helpers that a given file does not use are covered by the `dead_code`
//! allowance at its include site.

#![cfg(feature = "ai")]

use std::path::Path;

use ort::session::{builder::GraphOptimizationLevel, Session};
use webfang_ai::infrastructure_ai::inference_engine::InputPlan;
use webfang_ai::infrastructure_ai::{AiModel, MiniLmTokenizer};

/// Paragraphs per synthetic page. The chunker packs ≤512 chars per chunk, so
/// ~400 × ~380-char paragraphs land near ~393 chunks — the issue's 153KB shape.
pub const PARAGRAPHS_PER_PAGE: usize = 400;

/// Exact read-only snapshot probed by the spike verdict (no downloads ever).
pub const CACHED_MODEL: &str = "/home/xavi/.cache/huggingface/hub/models--ibm-granite--granite-embedding-97m-multilingual-r2/snapshots/835ad14087e140460703cf0fae09f97d469d65c2/onnx/model.onnx";

/// One synthetic page: an article of identical paragraphs, each sized to fill
/// roughly one chunk (~380 chars < 512-char chunk cap).
pub fn synthetic_page() -> String {
    const SENTENCE: &str = "hello world hello world hello world hello world hello world. ";
    let mut html = String::from("<html><body><article>");
    for i in 0..PARAGRAPHS_PER_PAGE {
        html.push_str(&format!("<p>Párrafo {i}: {}</p>", SENTENCE.repeat(6)));
    }
    html.push_str("</article></body></html>");
    html
}

/// Build a minimal in-memory WordPiece tokenizer: no `tokenizer.json` file,
/// no network. Same pattern as the `EmbeddingAdapter` unit tests.
pub fn in_memory_tokenizer() -> MiniLmTokenizer {
    use tokenizers::models::wordpiece::WordPiece;

    const WORDS: [&str; 6] = ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "hello", "world"];
    const IDS: [u32; 6] = [0, 100, 101, 102, 5, 6];
    let vocab: [(String, u32); 6] = std::array::from_fn(|i| (WORDS[i].to_string(), IDS[i]));
    let model = WordPiece::builder()
        .unk_token("[UNK]".to_string())
        .vocab(vocab)
        .build()
        .expect("inline wordpiece vocab must build");
    MiniLmTokenizer::new(tokenizers::Tokenizer::new(model), 512)
}

/// Build a probe session exactly like production: Level3 over `intra_threads`.
/// The smoke probe passes 1 (batching isolation); the harness batch cell
/// passes the whole-machine budget.
pub fn build_ort_session(model_path: &Path, intra_threads: usize) -> Session {
    Session::builder()
        .expect("la construcción de la sesión ORT debe estar disponible")
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .expect("el nivel de optimización Level3 debe aceptarse")
        .with_intra_threads(intra_threads)
        .expect("intra_threads debe aceptarse")
        .commit_from_file(model_path)
        .expect("el modelo en caché debe cargar")
}

/// Current process peak RSS in KiB (`VmHWM` from `/proc/self/status`).
/// `None` off Linux — the caller degrades to "no RSS" instead of failing.
pub fn peak_rss_kib() -> Option<u64> {
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

/// Ready-to-run probe over the cached model, or `None` (after printing why)
/// when the cache is absent — the caller returns early, never downloads.
pub struct ProbeSetup {
    pub session: Session,
    pub plan: InputPlan,
    pub variant: AiModel,
}

/// Resolve the cached model read-only and build the single-session probe.
/// Prints the graceful skip with the caller's test name and returns `None`
/// when the cache is absent.
pub fn setup_probe(test_name: &str) -> Option<ProbeSetup> {
    let model_path = Path::new(CACHED_MODEL);
    if !model_path.is_file() {
        println!("SKIP {test_name}: modelo en caché ausente ({CACHED_MODEL}); sin descargas.");
        return None;
    }
    let variant = AiModel::Granite97M;
    let session = build_ort_session(model_path, 1);
    let plan = InputPlan::from_session(&session).expect("el plan debe resolverse");
    Some(ProbeSetup {
        session,
        plan,
        variant,
    })
}

//! Lightweight binary: the full `webfang` pipeline WITHOUT the `ai` feature.
//!
//! This target reuses the exact same entry point as the main binary
//! (`src/main.rs`) but is compiled with default features only, so the ONNX
//! stack (`tract-onnx`, `tokenizers`, `hf-hub`, SIMD embeddings) is excluded.
//! That keeps the artifact small (< 10 MB) for restricted/edge environments
//! while preserving crawling, readability extraction, TUI, and export.
//!
//! Build:
//! ```text
//! cargo build --profile core --bin rust_scraper_core
//! ```
//!
//! The full binary (with `ai` + SHA256-validated model) is built separately:
//! ```text
//! cargo build --release --features ai
//! ```

// Reuse the real CLI entry point verbatim. Because the whole codebase gates
// `ai` behind `#[cfg(feature = "ai")]`, compiling this bin without `--features
// ai` strips the ONNX code paths automatically — no code duplication.
#[path = "../main.rs"]
mod app;

fn main() -> impl std::process::Termination {
    app::main()
}

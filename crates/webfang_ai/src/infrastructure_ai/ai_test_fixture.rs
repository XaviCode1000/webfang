//! Shared AI test fixture — the ONE home for the in-memory WordPiece tokenizer
//! used by the AI test suites (#1575).
//!
//! # Why this file is included by `#[path]` from two crates
//!
//! The two consumers live on opposite sides of the lib boundary:
//!
//! - `#[cfg(test)] mod tests` inside
//!   [`embedding_adapter`](super::embedding_adapter) — a *unit* test, compiled
//!   as part of the `webfang_ai` library itself.
//! - `tests/erased_engine_ports_test.rs` — an *integration* test, compiled as a
//!   separate crate that links `webfang_ai` as an external dependency.
//!
//! Rust gives a library's own unit tests no way to see a `tests/` module, and
//! an integration test no way to see a `#[cfg(test)]` one. The only fully
//! general sharing mechanism is a dev-dependency crate, but the two obvious
//! homes both require a `Cargo.toml` change:
//!
//! - `webfang_test_utils` is already a dev-dependency of `webfang_ai`, but it
//!   depends on `webfang_core` only. Hosting this fixture would mean adding
//!   `webfang_ai` (and `tokenizers`) to its manifest — a new inter-crate edge.
//! - The fixture cannot become a public `webfang_ai` module either: this is
//!   test support, and shipping it in the library would put test scaffolding in
//!   production builds.
//!
//! So the single source of truth is this file, reached two ways:
//!
//! - The library's own unit tests get it from the `#[cfg(test)] mod
//!   ai_test_fixture;` declaration in `infrastructure_ai/mod.rs` — an ordinary
//!   module path, with no `#[path]` attribute.
//! - `tests/erased_engine_ports_test.rs` cannot see a `#[cfg(test)]` module (an
//!   integration test links a library compiled without `cfg(test)`), so it pulls
//!   the same file in with `#[path = "../src/infrastructure_ai/ai_test_fixture.rs"]`.
//!
//! Either way this file reaches **no production build**: the `mod` declaration
//! is `#[cfg(test)]`, and the `#[path]` include exists only in a test target.
//!
//! # The path-agnostic rule
//!
//! Because this file is compiled into two different crates, it must not name
//! either crate's items. `webfang_ai` has no `extern crate self as webfang_ai;`
//! alias, so `webfang_ai::…` resolves in the integration test but not in the
//! library's own unit tests, while `crate::…` does the exact opposite. Every
//! item below therefore refers only to the `tokenizers` crate, which is a
//! direct dependency of `webfang_ai` in both configurations. That constraint is
//! also why the *engine* half of the old fixture is not here — see the
//! `MockInferenceEngine` note in `embedding_adapter`'s own `fake_adapter`.

// Each consumer uses a different subset of this module, so items are unused in
// one of the two inclusions. Same allowance the shared `cli_harness` uses when
// it is pulled in by `#[path]`.
#![allow(dead_code)]

/// A model path that can never be loaded, for tests that need a real
/// `InferencePool` but no ONNX model on disk.
///
/// `InferencePool::new` succeeds for a path that does not exist: its worker
/// threads fail asynchronously while the pool still reports its configured
/// dimension. That makes it the right way to exercise the *concrete* engine —
/// which is exactly what the unsized-coercion test in
/// `tests/erased_engine_ports_test.rs` is about.
///
/// This constant is the single spelling of that path, so the "does not exist"
/// intent is asserted by one name instead of being restated per test.
pub const UNLOADABLE_MODEL_PATH: &str = "/nonexistent/webfang-fake-model.onnx";

/// Build a minimal in-memory WordPiece tokenizer — no `tokenizer.json` file
/// required.
///
/// The tokenizers only need to EXIST for component construction: both consumers
/// either never call `tokenize` (the `embedding_dim` unit tests) or drive a
/// mock engine that ignores the token ids entirely. Building one inline keeps
/// both suites free of any model download and fully deterministic.
///
/// # Panics
///
/// Panics if the WordPiece model cannot be built from the inline vocabulary.
/// The vocabulary is a fixed literal in this function, so a failure here means
/// a regression in the fixture itself, not a runtime condition.
#[must_use]
pub fn in_memory_wordpiece_tokenizer() -> tokenizers::Tokenizer {
    use tokenizers::models::wordpiece::WordPiece;
    // `WordPieceBuilder::vocab` accepts `Into<AHashMap>`; an array of tuples
    // converts directly (avoids a std HashMap → AHashMap mismatch).
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

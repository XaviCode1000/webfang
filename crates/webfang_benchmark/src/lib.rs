//! WebFang Stage 0 benchmark harness (`webfang_benchmark`).
//!
//! Decision instrument, not a shipped feature: measures WebFang across cost
//! per 1k pages, success rate against WAF challenge patterns, latency p50/p95,
//! and SPA-heavy crawl behavior — entirely in-process over a local simulated
//! corpus (Tier A). Zero production-code dependencies beyond the public core
//! API; this crate is a workspace leaf.

// Module declarations land with their tasks so every work-unit commit keeps
// `cargo check --workspace` green.

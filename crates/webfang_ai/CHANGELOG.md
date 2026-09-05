# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.0.0] - 2026-09-05


### 🎉 Added

- Add crossbeam-channel dependency for InferencePool
- Implement InferencePool with dedicated worker threads
- Complete InferencePool migration, remove legacy InferenceEngine
- Add GraniteDomInspector implementing SemanticInspectorPort
- Download progress, observability instrumentation, SHA256 test ([#394](https://github.com/XaviCode1000/webfang/pull/394)) ([#422](https://github.com/XaviCode1000/webfang/pull/422))
- Implement semantic vault search foundation ([#386](https://github.com/XaviCode1000/webfang/pull/386)) ([#436](https://github.com/XaviCode1000/webfang/pull/436))
- Add EmbeddingAdapter and wire vault-search ports ([#433](https://github.com/XaviCode1000/webfang/pull/433)) ([#480](https://github.com/XaviCode1000/webfang/pull/480))
- Add compat layer for WEBFANG_AI_MODEL_ID env var rename (persistencemode-5b #980) ([#987](https://github.com/XaviCode1000/webfang/pull/987))

### 🎨 Styling

- Apply cargo fmt
- Fix formatting in semantic_cleaner_pipeline.rs

### 🏗️ Architecture Improvements

- Complete product rename from rust_scraper to webfang
- Remove unused dependencies (serde, serde_json, thiserror from webfang_ai; rustls-webpki, time from webfang_core; bytes, thiserror from webfang_mcp)
- Remove dead pre-hf_hub model cache/download cluster ([#389](https://github.com/XaviCode1000/webfang/pull/389))
- Workspace lints + feature gate fix + scoped unwrap_used ([#405](https://github.com/XaviCode1000/webfang/pull/405))
- Wave 3 — resurrect ~40 dead tests with #[ignore] ([#412](https://github.com/XaviCode1000/webfang/pull/412))
- Complete #516 code-quality audit — clippy ratchets, dead deps, MCP coverage ([#541](https://github.com/XaviCode1000/webfang/pull/541))
- Test Suite Audit #239 remediation — deterministic, observable tests ([#546](https://github.com/XaviCode1000/webfang/pull/546))
- Remove silent-skips and message-string assertions ([#545](https://github.com/XaviCode1000/webfang/pull/545)) ([#557](https://github.com/XaviCode1000/webfang/pull/557))

### 📖 Documentation

- PR 0A — baseline snapshot + lint swap + dedup fix
- Annotate defensive error paths with LCOV_EXCL markers ([#530](https://github.com/XaviCode1000/webfang/pull/530))
- Fix stale cache path and align rate-limit defaults ([#729](https://github.com/XaviCode1000/webfang/pull/729))

### 🔧 CI/CD

- Activate deny(missing_docs) + doc quality CI jobs

### 🔧 Fixed

- Remove remaining rust_scraper references missed by PR #184 ([#195](https://github.com/XaviCode1000/webfang/pull/195))
- Normalize() returns None instead of panicking on zero-magnitude vectors
- Resolve 25 rustdoc errors + 71 doctest import paths
- Revert allow(missing_docs) speed-run, document items properly
- Remove unused MockPool and import in granite_dom_inspector tests
- Validate --threshold range at parse time instead of panic ([#347](https://github.com/XaviCode1000/webfang/pull/347))
- Wire hf_hub auto-download into SemanticCleanerImpl::new()
- Respect HF_HOME for offline model cache resolution
- Activate inert webfang_ai feature gate ([#399](https://github.com/XaviCode1000/webfang/pull/399)) ([#410](https://github.com/XaviCode1000/webfang/pull/410))
- Defuse production unwrap()/expect() — collector mem::take + documented invariants ([#466](https://github.com/XaviCode1000/webfang/pull/466))
- Resolve ONNX input mapping + elastic semaphore deadlock (#543 #544) ([#560](https://github.com/XaviCode1000/webfang/pull/560))
- --clean-ai exporta 0 chunks — enriquecer chunks AI con url/title ([#569](https://github.com/XaviCode1000/webfang/pull/569)) ([#572](https://github.com/XaviCode1000/webfang/pull/572))
- Robust centroid reference + cosine similarity correctness + filter warn ([#579](https://github.com/XaviCode1000/webfang/pull/579))
- Semantic cleaning — SHA256, shared session, link-density, z-score threshold ([#655](https://github.com/XaviCode1000/webfang/pull/655))
- Honest JS-only content errors + wire Tier 2 semantic ([#706](https://github.com/XaviCode1000/webfang/pull/706)) ([#730](https://github.com/XaviCode1000/webfang/pull/730))
- Fail loudly when AI_MODEL_ID is set to an unknown model ([#874](https://github.com/XaviCode1000/webfang/pull/874)) ([#888](https://github.com/XaviCode1000/webfang/pull/888))
- Enforcement rewiring + detector unification (Sprint 7-8 P1-conc, slice 2/5) ([#896](https://github.com/XaviCode1000/webfang/pull/896))
- Cluster F infra bugs — HybridRouter cancel token, Miri pin, AI env race, WafInspector DI ([#1041](https://github.com/XaviCode1000/webfang/pull/1041))
- Make InferencePool backpressure awaitable with tokio mpsc ([#1133](https://github.com/XaviCode1000/webfang/pull/1133))
- Remove InferencePool Clone so Drop cannot hang ([#1131](https://github.com/XaviCode1000/webfang/pull/1131))
- Remove reachable panics in startup, stdio, and data paths (#1123 #1108 #1109) ([#1152](https://github.com/XaviCode1000/webfang/pull/1152))

### 🔧 Other

- Resolve A/B/C before deny
- Deny clippy::expect_used + disallowed_types anyhow barrier ([#471](https://github.com/XaviCode1000/webfang/pull/471))

### 🧪 Testing

- Add 53 unit tests for TUI components
- Add SemanticCleaner pipeline integration tests
- Add 33 regression tests for 4 HTML cleaning pipelines
- Hermetic tests for vault_detector, args, and ai_integration ([#409](https://github.com/XaviCode1000/webfang/pull/409))
- Fix flaky test_execute_emits_pipeline_spans ([#418](https://github.com/XaviCode1000/webfang/pull/418))
- Re-enable full RAG pipeline integration tests (#542 phase 3)
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.0.0] - 2026-09-05


### ⚠️ Breaking Changes

- Remove OpenTelemetry (otel/otel-metrics features)

### 🎉 Added

- Add --ai-model flag with AI_MODEL_ID env var fallback
- Wire --adaptive-selectors to AdaptiveSelectorEngine ([#288](https://github.com/XaviCode1000/webfang/pull/288))
- Add positional URL argument and fix negative threshold parsing ([#426](https://github.com/XaviCode1000/webfang/pull/426))
- Add EmbeddingAdapter and wire vault-search ports ([#433](https://github.com/XaviCode1000/webfang/pull/433)) ([#480](https://github.com/XaviCode1000/webfang/pull/480))
- Comprehensive webfang features (#790+) ([#806](https://github.com/XaviCode1000/webfang/pull/806))

### 🏗️ Architecture Improvements

- Complete product rename from rust_scraper to webfang
- Workspace lints + feature gate fix + scoped unwrap_used ([#405](https://github.com/XaviCode1000/webfang/pull/405))
- Complete #516 code-quality audit — clippy ratchets, dead deps, MCP coverage ([#541](https://github.com/XaviCode1000/webfang/pull/541))
- Remove dead progress TUI, D1 spike debris, deprecated flags past window ([#880](https://github.com/XaviCode1000/webfang/pull/880)) ([#894](https://github.com/XaviCode1000/webfang/pull/894))

### 📦 Dependencies

- Eliminar dependencias sin usar (issue #353)

### 🔧 Fixed

- Remove remaining rust_scraper references missed by PR #184 ([#195](https://github.com/XaviCode1000/webfang/pull/195))
- Update max_tokens defaults from 512 to 32768 for Granite models
- Wire ai_integration test in webfang_ai crate instead of webfang_cli
- Update main.rs field accesses for args.rs modular split
- Propagate ai_model through ExportConfig and use opts.ai_config in SemanticCleanerImpl
- Validate --threshold range at parse time instead of panic ([#347](https://github.com/XaviCode1000/webfang/pull/347))
- Skip AI cleaner construction on --dry-run
- Propagate user-requested init errors instead of silent degradation ([#391](https://github.com/XaviCode1000/webfang/pull/391))
- Activate inert webfang_ai feature gate ([#399](https://github.com/XaviCode1000/webfang/pull/399)) ([#410](https://github.com/XaviCode1000/webfang/pull/410))
- Inject SemanticCleaner into ElasticIngestion for --output-vectors and --elastic ([#578](https://github.com/XaviCode1000/webfang/pull/578))
- Resolve issue #674 - Phase 2 integration, error message, dead code, missing features ([#679](https://github.com/XaviCode1000/webfang/pull/679))
- Gate build_crawler_config_from_json behind ui feature ([#719](https://github.com/XaviCode1000/webfang/pull/719))
- Chrome preflight exit 78 + MCP robots.txt enforcement (#685, #697) ([#722](https://github.com/XaviCode1000/webfang/pull/722))
- Honest JS-only content errors + wire Tier 2 semantic ([#706](https://github.com/XaviCode1000/webfang/pull/706)) ([#730](https://github.com/XaviCode1000/webfang/pull/730))
- Hallazgos menores auditoría CLI v2.0.0 ([#695](https://github.com/XaviCode1000/webfang/pull/695)) ([#754](https://github.com/XaviCode1000/webfang/pull/754))
- Wire chromium feature in CLI preflight + detect SPA by visible text ([#758](https://github.com/XaviCode1000/webfang/pull/758)) ([#771](https://github.com/XaviCode1000/webfang/pull/771))
- Resolve 5 CLI audit findings from #761 ([#774](https://github.com/XaviCode1000/webfang/pull/774))
- Obsidian: --vault redirige output + reparar artefacto byline vacío ([#762](https://github.com/XaviCode1000/webfang/pull/762)) ([#778](https://github.com/XaviCode1000/webfang/pull/778))
- Batch CLI and exporter reliability fixes ([#801](https://github.com/XaviCode1000/webfang/pull/801))
- Config provenance + single normalization pipeline (Sprint 6 P0-2, Gate 3) ([#870](https://github.com/XaviCode1000/webfang/pull/870))
- Fail loudly when AI_MODEL_ID is set to an unknown model ([#874](https://github.com/XaviCode1000/webfang/pull/874)) ([#888](https://github.com/XaviCode1000/webfang/pull/888))
- Enforcement rewiring + detector unification (Sprint 7-8 P1-conc, slice 2/5) ([#896](https://github.com/XaviCode1000/webfang/pull/896))
- Budget override plumbing — TOML/TUI concurrency reaches enforcement, burst-0 rejected everywhere ([#925](https://github.com/XaviCode1000/webfang/pull/925))
- Make invalid state unrepresentable in ExportState, CrawlerConfig and CategoryLimits ([#1132](https://github.com/XaviCode1000/webfang/pull/1132))

### 🔧 Other

- Resolve merge conflicts with main — preserve args.rs split and test extraction
- Deny clippy::expect_used + disallowed_types anyhow barrier ([#471](https://github.com/XaviCode1000/webfang/pull/471))
- Sprint 6 deferred follow-ups — trace provenance, rank guard, doc refresh ([#872](https://github.com/XaviCode1000/webfang/pull/872))
- Translate Spanish tracing messages to English ([#877](https://github.com/XaviCode1000/webfang/pull/877)) ([#878](https://github.com/XaviCode1000/webfang/pull/878))
- Budget override plumbing over current main — staged-wins crawl precedence ([#897](https://github.com/XaviCode1000/webfang/pull/897)) ([#936](https://github.com/XaviCode1000/webfang/pull/936))
- Eliminar crate webfang_tui (producto solo CLI/MCP/AI) ([#1180](https://github.com/XaviCode1000/webfang/pull/1180))

### 🧪 Testing

- Wire ai_integration tests and fix compilation
- SIGKILL crash matrix + SC7 cancellation + durability docs (Sprint 3-5 P0-1, PR5) ([#856](https://github.com/XaviCode1000/webfang/pull/856))
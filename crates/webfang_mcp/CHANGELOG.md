# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.0.0] - 2026-09-05


### 🎉 Added

- Integrate adaptive selector engine into scrape_with_config
- Integrate adaptive engine into scrape_with_config via select_sync_aware
- Implement real MCP export tools with honest errors (issue #343 slice 1) ([#383](https://github.com/XaviCode1000/webfang/pull/383))
- Real metrics in get_scrape_metrics (build ScrapeMetrics accumulator) ([#388](https://github.com/XaviCode1000/webfang/pull/388))
- Wire AI tools via Container cleaner port (issue #381 slice 2) ([#387](https://github.com/XaviCode1000/webfang/pull/387))
- Add error breakdown by category to crawl summary ([#374](https://github.com/XaviCode1000/webfang/pull/374)) ([#423](https://github.com/XaviCode1000/webfang/pull/423))
- Implement semantic vault search foundation ([#386](https://github.com/XaviCode1000/webfang/pull/386)) ([#436](https://github.com/XaviCode1000/webfang/pull/436))
- Cablear download_assets al AssetDownloader ([#452](https://github.com/XaviCode1000/webfang/pull/452)) ([#465](https://github.com/XaviCode1000/webfang/pull/465))
- Add EmbeddingAdapter and wire vault-search ports ([#433](https://github.com/XaviCode1000/webfang/pull/433)) ([#480](https://github.com/XaviCode1000/webfang/pull/480))
- Add eager vault indexing with staleness detection ([#435](https://github.com/XaviCode1000/webfang/pull/435)) ([#483](https://github.com/XaviCode1000/webfang/pull/483))
- Comprehensive webfang features (#790+) ([#806](https://github.com/XaviCode1000/webfang/pull/806))
- Extraction quality scoring + honest error hints (Slice A, #792) ([#804](https://github.com/XaviCode1000/webfang/pull/804))
- Dispatch compact vs playwright-mcp with yaml/json Content (T4 R6-R7)
- Wire observability and snapshot deltas for playwright-mcp (T4.1-T5 R6-R7)
- Unified COMMITTED-only resume gate with D3 commit protocol (Sprint 3-5 P0-1, PR3) ([#854](https://github.com/XaviCode1000/webfang/pull/854))
- Add compat layer for WEBFANG_AI_MODEL_ID env var rename (persistencemode-5b #980) ([#987](https://github.com/XaviCode1000/webfang/pull/987))

### 🏗️ Architecture Improvements

- Complete product rename from rust_scraper to webfang
- Remove unused dependencies (serde, serde_json, thiserror from webfang_ai; rustls-webpki, time from webfang_core; bytes, thiserror from webfang_mcp)
- Migrate discover_sitemap to crawl_with_sitemap, delete fetch_sitemap
- Centralize normalize_url with strip_www parameter
- Add acquire_semaphore! macro, eliminate ~210 LOC boilerplate
- Workspace lints + feature gate fix + scoped unwrap_used ([#405](https://github.com/XaviCode1000/webfang/pull/405))
- Wave 3 — resurrect ~40 dead tests with #[ignore] ([#412](https://github.com/XaviCode1000/webfang/pull/412))
- Complete #516 code-quality audit — clippy ratchets, dead deps, MCP coverage ([#541](https://github.com/XaviCode1000/webfang/pull/541))
- Test Suite Audit #239 remediation — deterministic, observable tests ([#546](https://github.com/XaviCode1000/webfang/pull/546))
- Remove silent-skips and message-string assertions ([#545](https://github.com/XaviCode1000/webfang/pull/545)) ([#557](https://github.com/XaviCode1000/webfang/pull/557))
- Wire OptionsSpec json_schema into MCP tool schemas + parity table (ADR-002 slice 4) ([#943](https://github.com/XaviCode1000/webfang/pull/943))
- Slice 4 review follow-ups (F4-F7 + bridge coverage) ([#969](https://github.com/XaviCode1000/webfang/pull/969))
- Slice 5a — SSOT completion (ai/obsidian/tui) (ADR-002) ([#983](https://github.com/XaviCode1000/webfang/pull/983))
- Followups from #983 review (parity + ai feature gate + threshold heading) ([#986](https://github.com/XaviCode1000/webfang/pull/986))
- Restore intra-crate direction + tighten allowlist to export narrow (ADR-0010 + ADR-0011) Closes #990 ([#993](https://github.com/XaviCode1000/webfang/pull/993))
- Collapse duplicate WAF VO families into canonical domain::waf ([#1049](https://github.com/XaviCode1000/webfang/pull/1049))
- Domain::ssrf_guard port; 3 allowlist entries closed (ADR-0012 3.C) ([#1059](https://github.com/XaviCode1000/webfang/pull/1059))
- Close the SSRF guard choke-point gap left by 3.C (ADR-0012 #1060) ([#1064](https://github.com/XaviCode1000/webfang/pull/1064))
- Port vault_search to domain::note_repository::VaultNoteReader (ADR-0012-B 3.I) ([#1073](https://github.com/XaviCode1000/webfang/pull/1073))
- Port RobotsFetcher to domain::crawler_port (ADR-0012-B post-narrow) ([#1089](https://github.com/XaviCode1000/webfang/pull/1089))
- Retire the infrastructure::config shim path and scrub ALIAS_NAMES ([#1128](https://github.com/XaviCode1000/webfang/pull/1128))

### 📖 Documentation

- Fix rustdoc warnings blocking GitHub Pages deploy
- Enforce missing docs lints across workspace ([#529](https://github.com/XaviCode1000/webfang/pull/529))
- Annotate defensive error paths with LCOV_EXCL markers ([#530](https://github.com/XaviCode1000/webfang/pull/530))
- Fix bare-urls rustdoc lint in require_safe_seed doc
- Fix stale cache path and align rate-limit defaults ([#729](https://github.com/XaviCode1000/webfang/pull/729))

### 📦 Dependencies

- Eliminar dependencias sin usar (issue #353)

### 🔧 Fixed

- Resolve merge conflicts and fix remaining rename issues
- Remove remaining rust_scraper references missed by PR #184 ([#195](https://github.com/XaviCode1000/webfang/pull/195))
- Use canonical crawler path + add auto-discovery wiremock test
- Resolve merge conflict + apply Finding 1 canonical path
- Resolve issue #218 — blocking syscalls, empty headers, exact host match
- MCP test wiring, try_send counter, Auto detection order ([#246](https://github.com/XaviCode1000/webfang/pull/246))
- Revert scraper_service integration, keep engine standalone
- Normalize seed_domain in is_internal_link to accept bare domains
- Detección WAF context-aware — elimina falsos positivos por mención de vendors ([#380](https://github.com/XaviCode1000/webfang/pull/380))
- Activate inert webfang_ai feature gate ([#399](https://github.com/XaviCode1000/webfang/pull/399)) ([#410](https://github.com/XaviCode1000/webfang/pull/410))
- Correct url_to_file_path example and download_assets schema docs ([#424](https://github.com/XaviCode1000/webfang/pull/424))
- Neutralize Obsidian URI command injection + harden path sanitization (#446 #447 #448) ([#453](https://github.com/XaviCode1000/webfang/pull/453))
- Defuse production unwrap()/expect() — collector mem::take + documented invariants ([#466](https://github.com/XaviCode1000/webfang/pull/466))
- Crawl_site max_depth now follows internal links ([#479](https://github.com/XaviCode1000/webfang/pull/479)) ([#481](https://github.com/XaviCode1000/webfang/pull/481))
- Trace events share one correlation ID per run — no per-page correlation in trace JSONL ([#506](https://github.com/XaviCode1000/webfang/pull/506))
- Business logic review — dedup normalization, nofollow, checkpoint queue ([#517](https://github.com/XaviCode1000/webfang/pull/517)) ([#525](https://github.com/XaviCode1000/webfang/pull/525))
- Include pure downloader submodules in Miri/TSan, replace unsafe expect paths ([#524](https://github.com/XaviCode1000/webfang/pull/524))
- Remove async-unsafe span.enter() guards across .await ([#519](https://github.com/XaviCode1000/webfang/pull/519)) ([#526](https://github.com/XaviCode1000/webfang/pull/526))
- Add deny_unknown_fields + validate() to *Params structs (#512, slice 1/2)
- Wire params.validate() into all 29 tool handlers (#512, slice 2/2)
- Accept bare base_domain and full-URL seed_domain (over-rejection fix, #512)
- Robust centroid reference + cosine similarity correctness + filter warn ([#579](https://github.com/XaviCode1000/webfang/pull/579))
- Resolve 10 bugs from comprehensive tool audit ([#590](https://github.com/XaviCode1000/webfang/pull/590)) ([#592](https://github.com/XaviCode1000/webfang/pull/592))
- Expose batch failures and honest Obsidian dispatch (issue #591) ([#594](https://github.com/XaviCode1000/webfang/pull/594))
- Reject zero concurrency and zero max_pages to prevent deadlock/panic ([#611](https://github.com/XaviCode1000/webfang/pull/611))
- Process_export_pipeline scrapea la url antes de exportar ([#617](https://github.com/XaviCode1000/webfang/pull/617))
- Generate_rich_metadata incluye language y content_type; reading_time 0 para vacío ([#616](https://github.com/XaviCode1000/webfang/pull/616))
- Classify Network{403} as Waf, Network{429} as RateLimit ([#630](https://github.com/XaviCode1000/webfang/pull/630))
- Deep crawl — preserva query strings, respeta --max-depth, final_url ([#661](https://github.com/XaviCode1000/webfang/pull/661))
- Prevent tracing callsite Interest poisoning in metrics test ([#665](https://github.com/XaviCode1000/webfang/pull/665))
- Export edge-case audit — query/fragment filenames, -o -, output-vectors, metadata v2.1.0, dedup ([#672](https://github.com/XaviCode1000/webfang/pull/672))
- Disable SSRF for tests, fix clippy issues ([#673](https://github.com/XaviCode1000/webfang/pull/673)) ([#677](https://github.com/XaviCode1000/webfang/pull/677))
- Block SSRF bypass via IPv4-mapped IPv6 addresses ([#710](https://github.com/XaviCode1000/webfang/pull/710))
- ScrapeEvent carries trace_id + correlation_id (#704 Paso 4, #698) ([#717](https://github.com/XaviCode1000/webfang/pull/717))
- Chrome preflight exit 78 + MCP robots.txt enforcement (#685, #697) ([#722](https://github.com/XaviCode1000/webfang/pull/722))
- Honest JS-only content errors + wire Tier 2 semantic ([#706](https://github.com/XaviCode1000/webfang/pull/706)) ([#730](https://github.com/XaviCode1000/webfang/pull/730))
- Fase 4 hardening — SSRF remanente, cota de métricas y auth fail-fast ([#707](https://github.com/XaviCode1000/webfang/pull/707)) ([#732](https://github.com/XaviCode1000/webfang/pull/732))
- Enforce export root-of-trust and per-domain batch metrics ([#696](https://github.com/XaviCode1000/webfang/pull/696)) ([#735](https://github.com/XaviCode1000/webfang/pull/735))
- Make Obsidian vault detection hermetic (injectable registry path) ([#726](https://github.com/XaviCode1000/webfang/pull/726))
- Enforce robots.txt uniformly across all direct-fetch tools ([#755](https://github.com/XaviCode1000/webfang/pull/755))
- Enforce export root-of-trust gate in export_file/export_jsonl/export_vector ([#756](https://github.com/XaviCode1000/webfang/pull/756)) ([#768](https://github.com/XaviCode1000/webfang/pull/768))
- Answer stdio initialize before resolving AI models ([#759](https://github.com/XaviCode1000/webfang/pull/759)) ([#772](https://github.com/XaviCode1000/webfang/pull/772))
- Align detect_spa with the scrape extraction chain ([#760](https://github.com/XaviCode1000/webfang/pull/760)) ([#773](https://github.com/XaviCode1000/webfang/pull/773))
- Address verify warnings W2-W3 (closes #812 verify)
- Fail loudly when AI_MODEL_ID is set to an unknown model ([#874](https://github.com/XaviCode1000/webfang/pull/874)) ([#888](https://github.com/XaviCode1000/webfang/pull/888))
- Close DNS rebinding TOCTOU + hostname-redirect SSRF via validating connect-time resolver ([#917](https://github.com/XaviCode1000/webfang/pull/917))
- Cluster F infra bugs — HybridRouter cancel token, Miri pin, AI env race, WafInspector DI ([#1041](https://github.com/XaviCode1000/webfang/pull/1041))
- Batch 1 security & lifecycle hardening (#1124 #1125 #1126 #1129) ([#1139](https://github.com/XaviCode1000/webfang/pull/1139))
- Remove reachable panics in startup, stdio, and data paths (#1123 #1108 #1109) ([#1152](https://github.com/XaviCode1000/webfang/pull/1152))
- Type the URL and hex boundaries with existing newtypes (#1116 #1117 #1118) ([#1158](https://github.com/XaviCode1000/webfang/pull/1158))

### 🔧 Other

- Update Cargo.lock and test tweaks for wiremock
- Deny clippy::expect_used + disallowed_types anyhow barrier ([#471](https://github.com/XaviCode1000/webfang/pull/471))
- Correcciones no bloqueantes de auditoría (validate_url, extract_domain, discover_urls, detect_waf, scrape_with_options) ([#618](https://github.com/XaviCode1000/webfang/pull/618))
- Consolidate 6 pending fixes (waf, url, crawler, audit, docs) ([#625](https://github.com/XaviCode1000/webfang/pull/625))
- Fix misconfigured Rust tooling (dead fuzz.toml, broken aliases, CI drift) ([#725](https://github.com/XaviCode1000/webfang/pull/725))
- Validate startup --output against --export-roots for process_export_pipeline consistency ([#769](https://github.com/XaviCode1000/webfang/pull/769)) ([#777](https://github.com/XaviCode1000/webfang/pull/777))
- Eliminar crate webfang_tui (producto solo CLI/MCP/AI) ([#1180](https://github.com/XaviCode1000/webfang/pull/1180))

### 🧪 Testing

- Add 53 unit tests for TUI components
- Add wiremock behavioral test for discover_sitemap
- Introduce AssetDownloaderPort trait to fix vacuous spy test ([#217](https://github.com/XaviCode1000/webfang/pull/217))
- Ampliar cobertura de tools MCP (issue #450) ([#470](https://github.com/XaviCode1000/webfang/pull/470))
- Add params rejection tests and use relative temp dirs (#512, slice 2/2)
- Add CLI+AI and MCP+AI integration tests (#542 phases 4-5)
- Live SSRF probe integration test with guard enabled ([#718](https://github.com/XaviCode1000/webfang/pull/718))
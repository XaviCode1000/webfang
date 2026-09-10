# Mode-D verification run — issue #1295 · AUDIT-02 AI columns + F-52

**Date:** 2026-09-10 · **Branch/worktree:** `chore/mode-d-verify` @ `~/Projects/Rust/webfang-worktrees/chore-mode-d-verify`
**Base:** `08eee306` (main, post F-52-a #1279 + F-52-c #1285; F-52-b still open as #1277)
**Mode:** measurement only — **zero behaviour changes** in this branch. Findings become child issues.

Full detail lives in [`PHASE7-VERIFY-1295.md`](./PHASE7-VERIFY-1295.md). This file is the report.

---

## 1. Build facts

| Item | Value |
| :--- | :--- |
| Toolchain | `rustc 1.88.0` (repo pin; reached with `env -u RUSTUP_TOOLCHAIN`) |
| Isolated build | `CARGO_TARGET_DIR=~/.cache/cargo-target/chore-mode-d-verify`, `CARGO_BUILD_JOBS=2`, `env -u RUSTC_WRAPPER -u RUSTUP_TOOLCHAIN`, `RUSTFLAGS=-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads` (#1267 recipe, BFD-only flags — see JS-RENDERING.md deviation note) |
| BIN2 | `cargo build -p webfang_cli --bin webfang --features ai,chromium,mcp` — **7m34s** cold |
| MCP+AI | `cargo build -p webfang_mcp --bin webfang-mcp --features ai,mcp` — **4m58s** cold |
| BIN1 (no `ai`) | `cargo build -p webfang_cli --bin webfang` — **2m16s**, used only for the honest-degradation rows |
| Model | **Granite-97M** (`ibm-granite/granite-embedding-97m-multilingual-r2`) from the **existing** hf_hub cache (397 MB). **No download happened** (verified with `find ~/.cache/huggingface -newermt …` → empty). Every AI row ran on a **warm** cache. |
| Fixture | `fixtures/moded/article.html` (8 topical paragraphs + nav/footer boilerplate) served by `python3 -m http.server 127.0.0.1:18991` with the three documented SSRF test hatches |

## 2. Per-cell verdicts

### CLI+AI (was BLOCKED in AUDIT-02 §14)

| Capability × input class | Verdict | Evidence |
| :--- | :--- | :--- |
| single-page × valid URL × `--clean-ai` | **PASS** | exit 0, 2.97 s, 6 chunks, boilerplate (nav/footer) 0 hits in the corpus |
| single-page determinism × warm cache | **PASS** | 2 runs byte-identical after `timestamp_utc` is stripped — the only diff is the clock |
| multi-page crawl × `--clean-ai` | **PASS** | 3-page loopback crawl → 5 chunks; MDN sitemap crawl → 37 chunks, exit 0 |
| batch × `--clean-ai` | **PASS** | 2 stdin URLs → 8 records, 6 + 2 per URL, no duplicates |
| export jsonl + md + `--format json` | **PASS** | `export.jsonl` + `127.0.0.1/article.html.md` + `results.json` |
| resume / checkpoint × `--clean-ai` | **PARTIAL** (shared with the plain path) | run 1 → 6 chunks; run 2 same state → **0 duplicates** (#946 holds), but run 2 exits 0 creating **no `export.jsonl` at all`. Identical without `--clean-ai` (1 record, then no file), so it is not an AI defect — it belongs to the resume lifecycle cluster **#1292** |
| robots × `--clean-ai` | **PARTIAL** (blocked correctly, message wrong) | google `/search` → exit 69 and the run stops, but it is reported as `WAF/CAPTCHA detectado … robots.txt` → **dupe of #1301** |
| SSRF × `--clean-ai` | **PASS** | with the hatches removed, loopback → `SSRF literal-IP target rejected at entry (no socket opened)` + exit 69, 0 records |
| AI cleaning (capability cell) | **PASS** | 384-dim embeddings, coherent paragraph chunks, relevance scores applied |
| AI failure: invalid `--ai-model` | **PASS** | exit 64, Spanish `Modelo AI inválido para --ai-model: Unknown AI model 'does-not-exist'. Valid values: granite-97m, granite-311m`, rejected **before** any fetch |
| AI failure: model absent from cache (`--offline`) | **PASS** | exit 78, Spanish `No se pudo inicializar el limpiador semántico AI: Modo offline: modelo '…' no está en caché`, **zero artefacts**, no panic |
| AI failure: `--threshold` out of range | **PASS** | `1.5` and `-0.1` → exit 64, Spanish `'1.5' está fuera de rango (rango válido: 0.0 a 1.0)` |
| AI failure: over-aggressive threshold | **PASS** | `--threshold 0.99` → exit 0, 1 chunk kept, WARN with `chunks_before/after/loss_ratio` (observability contract met) |
| AI failure: chunk over `--max-tokens` | **PASS** | exit 78, Spanish `Chunk chunk-0 excede límite de tokens: 88 > 8 (modelo: IBM Granite)`; per-page WARN fallback + total-failure ERROR (#543 holds) |
| offline (warm cache, `--offline`) | **PASS** | exit 0, same 6 chunks as online |
| cancellation × `--clean-ai` | **PASS** | SIGINT: 4 completed pages → 7 chunk records, exit 0. SIGTERM: 5 pages → 9 records, exit 0. Both log `received SIG… — draining in-flight work`, 0 panics, and the JSONL stays whole (every line parses, file ends on `\n` — no torn tail). Not AI-specific (identical without `--clean-ai`) |
| configuration: `WEBFANG_THRESHOLD` / `WEBFANG_AI_MODEL_ID` / legacy `AI_MODEL_ID` | **PASS** | env honoured (5 records at 0.5 vs 6 at 0.3); poisoned `AI_MODEL_ID=nonsense` → exit 64 naming **the env var**; poisoned env on an unrelated command → exit 0 (#827 holds) |
| observability × `--clean-ai` | **PASS** (was FAIL on the plain column) | trace JSONL carries `resolve_model_assets`, `download_model_assets`, `infer`, `prune_dom`, `export_batch`; structured fields, not string soup |
| `--output-vectors` | **FAIL** | → **MD-1** |
| memory footprint | **PASS** | RSS 1 152 MB peak on a ~1 MB wikipedia page (plan budget < 2 GB), 95 chunks, 8.8 s |
| export quality × real page | **PARTIAL** | 35/95 wikipedia chunks end mid-sentence → **MD-4** |
| feature-absent honesty (BIN1) | **PASS** | `--clean-ai` → exit 78 Spanish; `--output-vectors` → exit 78 Spanish; `--ai-model` → unknown flag exit 64; no `AI Settings` block in `--help` |

### MCP+AI (was BLOCKED)

| Row | Verdict | Evidence |
| :--- | :--- | :--- |
| `tools/list` with `--enable-ai` | **PASS** | 36 tools, `semantic_cleaner` + `search_obsidian` present |
| `semantic_cleaner` live call | **PASS** | `isError: false`, `chunks: 8`, `embedding_dim: 384`, every chunk carries a 384-float vector; doc keys `id,url,title,content,metadata,timestamp,embeddings` |
| honesty when compiled-with-ai but not enabled | **PASS** | `isError: true`, Spanish: `funcionalidad no disponible: limpieza semántica con IA. El binario tiene IA compilada pero no activa: reinicia el servidor con --enable-ai…` |
| `search_obsidian` without a vault | **PASS** | `isError: true`, Spanish `…no hay repositorio de notas configurado…` |
| SSRF guard on the AI tool | **PASS** | loopback with the hatches off → JSON-RPC `-32602 SSRF detectado: IP 127.0.0.1 prohibida` |
| robots guard on the AI tool | **PASS** | google `/search` → `isError: true` naming robots.txt |
| `scrape_url` smoke on the ai build | **PASS** | `isError: false`, full record envelope |
| strict params (P5-3) | **PASS** | `scrape_url {url, output_dir}` → rejected as `unknown field` |

### JS rendering (F-52, was BLOCKED on "no Chrome")

| Row | Verdict | Evidence |
| :--- | :--- | :--- |
| `static` on `quotes.toscrape.com/js/` | **PASS (honest failure)** | exit 65, Spanish `contenido insuficiente (5 caracteres) … requieren renderizado de JavaScript` |
| `full` crawl, 2 pages, real site | **PASS** | exit 0, `2 total, 2 succeeded`, 2 739 chars/page. Render proof: served bytes have **0** `class="quote"` nodes and the quote text exists **only** inside a `<script>` JSON blob |
| F-52-c (gate-certified launcher) | **PASS** | no `Permission denied (os error 13)`, no preflight/launched-binary mismatch (the #1285 symptom) |
| `full` × concurrency > 1 | **FAIL** | → **MD-3** |
| `full` × non-2xx response | **FAIL** | → **MD-2** |
| `full` × late-rendered DOM (F-52-b) | **NOT TESTED** | still open as #1277; the real late-render target `…/js/delayed/` now 404s upstream, which is what surfaced MD-2 |

## 3. Findings → child issues (no fixes)

| ID | Sev | Issue | One-line repro |
| :-- | :-- | :-- | :-- |
| **MD-1** | High | [#1310](https://github.com/XaviCode1000/webfang/issues/1310) | `--clean-ai --output-vectors` on the fixture: GETs 1 → **2**, corpus **6** records vs vectors **8**, one shared `sha256_hex`, no chunk ordinal |
| **MD-2** | Critical | [#1311](https://github.com/XaviCode1000/webfang/issues/1311) | `--js-strategy full` on the real 404 `quotes.toscrape.com/js/delayed/` → exit **0** + the 404 body in `export.jsonl`; same URL with `static` → exit 69 |
| **MD-3** | High | [#1312](https://github.com/XaviCode1000/webfang/issues/1312) | `--js-strategy full --max-pages 4` → 4× `SingletonLock: El fichero ya existe (17)` on `/tmp/chromiumoxide-runner`, exit 69, 1/4 pages; `--concurrency 1` → 0 failures |
| **MD-4** | Medium | [#1313](https://github.com/XaviCode1000/webfang/issues/1313) | `--clean-ai` on the Rust wikipedia page → 35/95 chunks end mid-sentence; chunk 6 ends `… (i.e., that all \nreferences`, chunk 7 resumes `point to valid memory) …` |
| **MD-5** | — | **not filed** — `--clean-ai --resume` with nothing new exits 0 without creating `export.jsonl` | two identical `--clean-ai --resume --state-dir …` runs → run 2 exit 0, file absent. **Reproduces identically without `--clean-ai`**, so it is not an AI-column defect; it is already inside the scope of #1292 (interruption/resume lifecycle). Recorded here so the evidence is not lost |

Cross-checked as **not** new: the robots→WAF misreport is #1301; the AI-failure honesty rows were already
claimed by #680/#681/#796/#575/#544 and all hold on this build; F-52-a/b/c are #1276/#1277/#1278.

## 4. Coverage-matrix delta to apply

See [`MODE-D-MATRIX-1295.md`](./MODE-D-MATRIX-1295.md) for the cell-by-cell table. Net: **no AI column stays
BLOCKED**; `CLI+AI` and `MCP+AI` close as PASS except the cells above, and the `JS rendering` row closes its
environment BLOCKED with execution evidence (two code defects left open as MD-2/MD-3).

## 5. Not tested here (and why)

- **Granite-311M** (`--ai-model granite-311m`, plan row 7.5) — needs a ~1.25 GB HF download; the mission
  requires explicit authorisation before downloading. The override *plumbing* is proven on 97M + the fail-fast path.
- **Cold-cache start with a real download** (plan 7.1) — same reason. Safe recipe if authorised:
  `HF_HOME=/tmp/<empty>` (hf-hub 0.5 reads **`HF_HOME` only**, not `HF_HUB_CACHE`) instead of clearing the shared cache.
- **Hard network cut** for 7.6/7.7 — approximated with `--offline` + an empty `HF_HOME`, which is the same
  code path (`SemanticError::OfflineMode`), rather than disabling the interface for the whole machine.
- **F-52-b late-render** — upstream fixture 404s; tracked by #1277.

## 6. Definition of done for this run

- [x] ai-featured builds, flags recorded verbatim
- [x] Phase-7 harness executed (11 of 12 rows; 7.1/7.5-partial deferred with reasons above)
- [x] MCP+AI smoke (7.11) + CLI+AI smoke + BIN1 honesty (7.10)
- [x] one real-page JS `full` crawl → F-52 environment BLOCKED lifted
- [x] coverage matrix updated (delta file)
- [x] failures captured as scoped child issues, **no fixes attempted**
- [x] no `CHANGELOG.md` touch, no push, no PR

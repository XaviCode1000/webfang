# Mode-D verification run — AUDIT-02 Phase 7 (AI) + F-52 JS rendering

**Issue:** #1295 · **Branch/worktree:** `chore/mode-d-verify` @ `~/Projects/Rust/webfang-worktrees/chore-mode-d-verify`
**Base SHA:** `08eee306` (`main`, includes F-52-a #1279 and F-52-c #1285; F-52-b still in review #1286)
**Date:** 2026-09-10 · **Method:** real execution, ai-featured builds. **Measurement only — zero behaviour changes.**

---

## 0. Build facts (reproducible)

Toolchain `rustc 1.88.0` (repo pin, reached with `-u RUSTUP_TOOLCHAIN`). Isolated target dir per #1267
(concurrent agents must not share the build cache):

```bash
env -u RUSTC_WRAPPER -u RUSTUP_TOOLCHAIN \
  CARGO_TARGET_DIR=~/.cache/cargo-target/chore-mode-d-verify CARGO_BUILD_JOBS=2 \
  RUSTFLAGS="-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads" \
  cargo build <...>
```

| Artifact | Features | Cold build | Notes |
| :--- | :--- | :--- | :--- |
| `webfang` (BIN2, AI) | `ai,chromium,mcp` | **7m34s** | `cargo build -p webfang_cli --bin webfang` |
| `webfang-mcp` (AI) | `ai,mcp` | **4m58s** | `cargo build -p webfang_mcp --bin webfang-mcp` |
| `webfang` (BIN1, no AI) | default | **2m16s** | rebuilt in the same dir for the 7.10 honest-degradation cell |

**Model:** Granite-97M (`ibm-granite/granite-embedding-97m-multilingual-r2`, 372 MB blob) from the **native hf_hub
cache** (`~/.cache/huggingface/hub/`, 397 MB, already present) for the initial pass. Granite-311M
(`…-311m-multilingual-r2`, **1.2 GB** blob) was pulled later under explicit authorisation; the 7.1 cold-start test
ran against an **isolated `HF_HOME`**, so the shared cache was never cleared.

**Loopback fixtures** are served by `python3 -m http.server 18991` with the three documented test hatches
(`WEBFANG_DISABLE_SSRF_ENTRY_GUARD/RESOLVER/REDIRECT_GUARD=1`, `domain/ssrf_guard.rs:58-82`), same method as
[`REVERIFICATION.md`](./REVERIFICATION.md) and [`JS-RENDERING.md`](./JS-RENDERING.md).

Fixture used for the AI cells: `fixtures/moded/article.html` (8 topical paragraphs + nav/footer boilerplate).
For the crawl cells: `many.html` + 12 generated sibling probes (`c0..c11.html`).

---

## 1. Phase 7 — AI semantic cleaning

| # | Test | Verdict | Evidence |
| :--- | :--- | :--- | :--- |
| 7.1 | Cold start (real download) | **PASS on outcome / PARTIAL on progress** → **#1316 (MD-7)** | fresh `HF_HOME` + network → 397 MB pulled in **1m48s**, exit 0, 6 chunks, RSS 1.28 GB, no panic. But the bar renders **only on a TTY**: piped/redirected runs emit **275 bytes** of stderr whose first line appears *after* the download; the same run under a pty emits 231 750 bytes including a `MiB` bar. The 311M cold pull (1.2 GB, **5m52s**) is equally silent when piped |
| 7.1b | Warm-cache cleaning | **PASS** | exit 0, 2.97 s, RSS 1 152 MB, 6 chunks, Spanish-clean stderr |
| 7.2 | Chunk quality (real web, S4 wikipedia) | **PARTIAL** | exit 0, 8.76 s, 95 chunks, RSS 1 152 MB. **35 of 95 chunks end mid-sentence** → **#1313 (MD-4)** |
| 7.3 | `--output-vectors <file>` | **FAIL** → **#1310 (MD-1)** | 384-dim vectors ✔, but **8 vector records vs 6 exported chunks** for the same page, and the run performs a **second HTTP GET per URL** |
| 7.4 | `--output-vectors -` (stdout) | **PASS** | 8 pure-JSONL lines on stdout, human log entirely on stderr, zero cross-pollution |
| 7.5 | Model override | **PASS** | `--ai-model granite-311m` → real 1.2 GB download; warm run exit 0 in 8.11 s, **384-dim** vectors (768→384 Matryoshka holds), same 6-chunk grouping, embeddings measurably different from 97M (**l2 distance 1.4911**) so the tier really switched; blob SHA `75f9f258…d541` = `DEFAULT_FALLBACK_MODEL_SHA256`. `WEBFANG_AI_MODEL_ID=granite-311m` alone → identical 311M vectors; flag beats env (`--ai-model granite-311m` + env 97m → 311M vectors). Invalid id → exit 64 + Spanish `Modelo AI inválido para --ai-model: Unknown AI model 'does-not-exist'. Valid values: granite-97m, granite-311m` before any fetch; `--ai-model ""` → falls back to default; `" GRANITE-97M "` → accepted (trim+case-insensitive). RSS price of the tier → **MD-6 / #1315** |
| 7.6 | Offline + warm cache (`--offline`) | **PASS** | exit 0, same 6 chunks as the online run — for **both** tiers (311M offline from cache: exit 0, 6 chunks) |
| 7.7 | Offline + cold cache | **PASS** | exit 78, Spanish `No se pudo inicializar el limpiador semántico AI: Modo offline: modelo '…97m…' no está en caché`, **zero artefacts written**, no panic. ⚠️ `hf_hub` 0.5 honours **`HF_HOME` only** — a first attempt using `HF_HUB_CACHE` was silently invalid (exit 0) |
| 7.8 | ChunkTooLarge | **PASS** | `--max-tokens 8` → exit 78, Spanish `Chunk chunk-0 excede límite de tokens: 88 > 8 (modelo: IBM Granite)`, WARN (per-page fallback) + ERROR (total failure) — never a silent fallback |
| 7.9 | AI + batch (2 URLs, stdin) | **PASS** | exit 0, 8 records (6 + 2), one file per URL, no duplicates |
| 7.10 | AI flags on BIN1 (no `ai`) | **PASS** | `--clean-ai` → exit 78 Spanish; `--output-vectors` → exit 78 Spanish; `--ai-model` → exit 64 (flag not compiled); `--help` shows no AI section at all |
| 7.11 | MCP AI tools | **PASS** | 36 tools; `semantic_cleaner` → `chunks: 8`, `embedding_dim: 384`, every chunk carries a real 384-float vector, `isError` absent; `search_obsidian` honest Spanish error without a vault; without `--enable-ai` both tools answer `funcionalidad no disponible … reinicia el servidor con --enable-ai`; **SSRF still enforced** (`127.0.0.1` → `-32602`) and **robots still enforced** (google `/search` → isError) |
| 7.12 | Memory footprint | **PASS (97M) / FAIL (311M)** → **#1315 (MD-6)** | warm, same command, `--ai-model` swapped: 97M **1 152 544 KB ≈ 1.10 GiB** (inside the plan's <2 GB budget); 311M **3 360 932 KB ≈ 3.20 GiB** — over budget, and steady state, not a download artifact (cold 311M was 3 423 952 KB) |
| 7.13 | Corrupted cache (integrity) | **PASS** | `model.onnx` bytes flipped in an isolated `HF_HOME` → exit 78, Spanish `Validación de caché falló para '…97m…': SHA256 inválido (esperado: 68e592b1…, obtenido: 106d89af…)`, **zero artefacts**, no panic — closes the plan's “cache corruption handling” bug class |

**Threshold failure modes (all PASS):** `--threshold 1.5` → exit 64 `'1.5' está fuera de rango (rango válido: 0.0 a 1.0)`;
`--threshold -0.1` → exit 64; `--threshold 0.99` → exit 0 with the over-aggressive-filtering WARN that carries
`chunks_before/after/loss_ratio`, and honestly 1 chunk kept.

---

## 2. F-52 — JS rendering with `--js-strategy full`, real page

**The BLOCKED verdict in AUDIT-02 §14 is now closed with execution evidence on a real public site.**

| Check | Verdict | Evidence |
| :--- | :--- | :--- |
| `static` on `https://quotes.toscrape.com/js/` (JS-only content) | **PASS (honest)** | exit 65, Spanish `… contenido insuficiente (5 caracteres) — la página devolvió muy poco contenido extraíble` / `requieren renderizado de JavaScript`. Zero artifacts — no silent empty success |
| `full` on the same URL, 2 pages | **PASS** | exit 0; `Finished: 2 total, 2 succeeded`; 2 739 chars/page. Render proof: served bytes contain `0` occurrences of `class="quote"` and the quote text appears **only** inside a `<script>` JSON blob → markdown with quote structure can only come from an executed render |
| F-52-c (pinned launcher) | **PASS** | no `Permission denied (os error 13)` and no preflight/launch mismatch reproduced in this build (was the #1285 symptom) |
| Concurrent launches | **FAIL** → **#1312 (MD-3)** | `--max-pages 4` (default concurrency) → **4 ×** `Chrome launch failed: … Failed to create /tmp/chromiumoxide-runner/SingletonLock: El fichero ya existe (17)`; identical run with `--concurrency 1` → **0** failures, 4 pages, 3 succeeded, 1 honestly failed |
| HTTP status on the browser path | **FAIL** → **#1311 (MD-2)** | `--js-strategy full` on a **real 404** (`quotes.toscrape.com/js/delayed/`, `curl` → 404) → **exit 0** + `export.jsonl` with 1 record whose content is the server's *“The requested URL was not found on the server…”*. Same URL with `static`/crawl → exit 69, 0 records |

---

## 3. Findings (each spawns its own issue; no fixes here)

- **MD-1 (#1310) — `--output-vectors` is a second pipeline, not a view of the corpus.** Per URL it re-fetches the
  page (`--clean-ai` alone: 1 GET; `+ --output-vectors`: **2 GETs**) and re-cleans the **raw bytes** through
  `ElasticIngestion::run` (`application/elastic_ingestion.rs:127-157`) while `--clean-ai` exports chunks built
  from `ScrapedContent.html` (`cli/export_flow.rs:158-176`). Same page → **6 exported chunks vs 8 vector
  records**. `VectorRecord` has no chunk ordinal (`infrastructure/stream/mod.rs:63-79`, `save_chunk` ignores
  `_chunk_index`) and `sha256_hex` is the **resource** digest, so all 8 rows share one key: the vectors cannot be
  joined back to the corpus. Cost: +1 fetch/URL (budget, WAF exposure), and a `vecs.jsonl` whose `count ≈ chunk
  count` expectation (plan 7.3) fails.
- **MD-2 (#1311) — the Chromium path exports non-2xx bodies as success.** Remote-triggerable, exit-code lie.
  Reproduces on a real site (see §2). Sibling of the unfiled “side observation” in
  [`JS-RENDERING.md`](./JS-RENDERING.md).
- **MD-3 (#1312) — `--js-strategy full` cannot run concurrently.** `ChromiumoxideDownloader` uses the fixed profile
  dir `/tmp/chromiumoxide-runner`; Chrome's `ProcessSingleton` rejects the second launch. The benchmark crate
  already documents this and works around it with a process-wide lock (`webfang_benchmark/src/runner.rs:41-58,
  125-140`), so the failure mode is known-but-unfixed in core; here it degrades a **normal crawl** (exit 69, 1
  of 4 pages lost at default concurrency). Fix needs a per-launch `user-data-dir`.
- **MD-4 (#1313) — chunk boundaries cut inside a sentence on real pages.** 35 of 95 chunks from the Wikipedia
  page end without terminal punctuation: chunk 6 ends
      `… Rust enforces memory safety (i.e., that all \nreferences` and chunk 7 begins
      `point to valid memory) without a conventional \ngarbage collector\n; ins…`. The cut sites coincide with the
      newlines markdown emits around inline links (`[text\n](url)`), so the chunker treats them as boundaries — the
      embedding then represents a fragment whose predicate lives in the neighbouring row. Plan 7.2 expects “not
      mid-sentence garbage”.
- **MD-6 (#1315) — the 311M tier keeps the model in RAM twice.** Warm steady-state RSS 3.20 GiB vs the plan's
  <2 GB budget (97M: 1.10 GiB). Cause: `resolve_model_assets` reads the whole file into `Arc<Vec<u8>>` for the
  SHA-256 check (`semantic_cleaner_impl.rs:713-725`), the pool holds it for its lifetime
  (`inference_engine.rs:255-265`), and the session is built with `commit_from_memory(bytes)`
  (`inference_engine.rs:373-387`) — so ORT takes its own copy. ~2 × 1.2 GB + overhead matches the measurement.
- **MD-7 (#1316) — the first download is invisible without a TTY.** `with_progress(true)`
  (`semantic_cleaner_impl.rs:682-694`) drives indicatif, which does not render when stderr is not a terminal: piped
  runs show **275 bytes** of stderr for a 1m48s / 5m52s stall, while the same run under a pty shows 231 750 bytes
  with a `MiB` bar. No tracing event names the pull at any verbosity, so CI/agent runs look hung and the audit's
  cold-start row cannot be verified headlessly.

**Environment/documentation observations (no issue):** (a) `hf_hub` 0.5 reads `HF_HOME` only — the audit plan's
“clear the hub cache” recipe must redirect `HF_HOME`, not delete the shared cache; (b) MCP `scrape_url` takes
`url` only (P5-3 strict params) — the plan's `output_dir` argument is correctly rejected; (c) the legacy
`AI_MODEL_ID` is honoured and distinguished from `WEBFANG_AI_MODEL_ID` in errors (both exit 64, Spanish), and a
poisoned value never breaks unrelated commands (#827 verified).

---

## 4. Cells that stay open (measurement limits of this run, not code blocks)

| Cell | Status | Why |
| :--- | :--- | :--- |
| CLI+AI × images/documents | NOT TESTED | outside Phase-7 scope |
| MCP+AI × real-web | PARTIAL | loopback + public google/robots only; no JS-rendered MCP cell exercised |
| MCP+AI × 311M | NOT TESTED | MCP server ran on the default 97M tier; the override was measured on the CLI |
| offline × hard network cut | PARTIAL | `--offline` (strict cache-only) verified for both tiers; a true link-down test would also break the fetch, so it was not isolated |

**Closed since the first draft of this file:** 7.1 cold start (real 397 MB pull under an isolated `HF_HOME`) and
7.5 `granite-311m` (real 1.2 GB pull) — both unblocked by the download authorisation. Nothing the issue listed as
*BLOCKED for lack of an ai build* remains unmeasured: **CLI+AI, MCP+AI, AI cleaning and AI failure columns are
CLOSED as BLOCKED** — see `MODE-D-MATRIX-1295.md` for the cell-by-cell flips.

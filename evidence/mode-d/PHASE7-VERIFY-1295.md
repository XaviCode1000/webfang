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

**Model:** Granite-97M (`ibm-granite/granite-embedding-97m-multilingual-r2`) from the **native hf_hub cache**
(`~/.cache/huggingface/hub/`, 397 MB, already present). **No download was performed** — verified by
`find ~/.cache/huggingface -newermt <start>` returning nothing. Every AI cell below ran against a **warm** cache.

**Loopback fixtures** are served by `python3 -m http.server 18991` with the three documented test hatches
(`WEBFANG_DISABLE_SSRF_ENTRY_GUARD/RESOLVER/REDIRECT_GUARD=1`, `domain/ssrf_guard.rs:58-82`), same method as
[`REVERIFICATION.md`](./REVERIFICATION.md) and [`JS-RENDERING.md`](./JS-RENDERING.md).

Fixture used for the AI cells: `fixtures/moded/article.html` (8 topical paragraphs + nav/footer boilerplate).
For the crawl cells: `many.html` + 12 generated sibling probes (`c0..c11.html`).

---

## 1. Phase 7 — AI semantic cleaning

| # | Test | Verdict | Evidence |
| :--- | :--- | :--- | :--- |
| 7.1 | Cold start (download) | **NOT TESTED** | Cache already warm; re-downloading 390 MB would require clearing a cache other runs share. Warm-cache path is PASS (below). Cold path is testable safely via `HF_HOME` redirect — needs an explicit download authorisation. |
| 7.1b | Warm-cache cleaning | **PASS** | exit 0, 2.97 s, RSS 1 152 MB, 6 chunks, Spanish-clean stderr |
| 7.2 | Chunk quality (real web, S4 wikipedia) | **PARTIAL** | exit 0, 8.76 s, 95 chunks, RSS 1 152 MB. **35 of 95 chunks end mid-sentence** → **#1313 (MD-4)** |
| 7.3 | `--output-vectors <file>` | **FAIL** → **#1310 (MD-1)** | 384-dim vectors ✔, but **8 vector records vs 6 exported chunks** for the same page, and the run performs a **second HTTP GET per URL** |
| 7.4 | `--output-vectors -` (stdout) | **PASS** | 8 pure-JSONL lines on stdout, human log entirely on stderr, zero cross-pollution |
| 7.5 | Model override | **PARTIAL** | `--ai-model granite-97m` explicit → exit 0, 6 chunks. Invalid id → exit 64 + Spanish `Modelo AI inválido para --ai-model: … Valid values: granite-97m, granite-311m` before any fetch. **`granite-311m` NOT TESTED** (≈1.25 GB download, unauthorised) |
| 7.6 | Offline + warm cache (`--offline`) | **PASS** | exit 0, same 6 chunks as the online run |
| 7.7 | Offline + cold cache | **PASS** | exit 78, Spanish `No se pudo inicializar el limpiador semántico AI: Modo offline: modelo '…97m…' no está en caché`, **zero artefacts written**, no panic. ⚠️ `hf_hub` 0.5 honours **`HF_HOME` only** — a first attempt using `HF_HUB_CACHE` was silently invalid (exit 0) |
| 7.8 | ChunkTooLarge | **PASS** | `--max-tokens 8` → exit 78, Spanish `Chunk chunk-0 excede límite de tokens: 88 > 8 (modelo: IBM Granite)`, WARN (per-page fallback) + ERROR (total failure) — never a silent fallback |
| 7.9 | AI + batch (2 URLs, stdin) | **PASS** | exit 0, 8 records (6 + 2), one file per URL, no duplicates |
| 7.10 | AI flags on BIN1 (no `ai`) | **PASS** | `--clean-ai` → exit 78 Spanish; `--output-vectors` → exit 78 Spanish; `--ai-model` → exit 64 (flag not compiled); `--help` shows no AI section at all |
| 7.11 | MCP AI tools | **PASS** | 36 tools; `semantic_cleaner` → `chunks: 8`, `embedding_dim: 384`, every chunk carries a real 384-float vector, `isError` absent; `search_obsidian` honest Spanish error without a vault; without `--enable-ai` both tools answer `funcionalidad no disponible … reinicia el servidor con --enable-ai`; **SSRF still enforced** (`127.0.0.1` → `-32602`) and **robots still enforced** (google `/search` → isError) |
| 7.12 | Memory footprint | **PASS** | RSS 1 152 MB on a ~1 MB wikipedia page (well under the ~2 GB budget); ONNX weights page in, no per-chunk reload |

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
  page end without terminal punctuation, and the next chunk resumes the same sentence: chunk 6 ends
  `… Rust enforces memory safety (i.e., that all \nreferences` and chunk 7 begins
  `point to valid memory) without a conventional \ngarbage collector\n; ins…`. The cut sites coincide with the
  newlines markdown emits around inline links (`[text\n](url)`), so the chunker treats them as boundaries — the
  embedding then represents a fragment whose predicate lives in the neighbouring row. Plan 7.2 expects “not
  mid-sentence garbage”.

**Environment/documentation observations (no issue):** (a) `hf_hub` 0.5 reads `HF_HOME` only — the audit plan's
“clear the hub cache” recipe must redirect `HF_HOME`, not delete the shared cache; (b) MCP `scrape_url` takes
`url` only (P5-3 strict params) — the plan's `output_dir` argument is correctly rejected; (c) the legacy
`AI_MODEL_ID` is honoured and distinguished from `WEBFANG_AI_MODEL_ID` in errors (both exit 64, Spanish), and a
poisoned value never breaks unrelated commands (#827 verified).

---

## 4. Cells that stay open (measurement limits of this run, not code blocks)

| Cell | Status | Why |
| :--- | :--- | :--- |
| CLI+AI × model 311m | NOT TESTED | 1.25 GB download, not authorised |
| CLI+AI × cold cache | NOT TESTED | same reason (needs a fresh 390 MB pull) |
| CLI+AI × images/documents | NOT TESTED | out of Phase-7 scope |
| MCP+AI × real-web | PARTIAL | loopback + public google/robots only; no JS-rendered MCP cell exercised |
| offline × hard network cut | PARTIAL | `--offline` (strict cache-only) verified; a true link-down test would also break the fetch, so it was not isolated |

Everything the issue listed as *BLOCKED for lack of an ai build* is now measured: **CLI+AI, MCP+AI, AI cleaning
and AI failure columns are CLOSED as BLOCKED** — see `MODE-D-MATRIX-1295.md` for the cell-by-cell flips.

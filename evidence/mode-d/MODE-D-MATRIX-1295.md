# AUDIT-02 Contract Matrix — Mode-D delta (issue #1295)

**Snapshot:** `2026-09-10` @ main `08eee306`, ai-featured builds (`ai,chromium,mcp` / `ai,mcp`),
Granite-97M from the warm hf_hub cache. Values below replace the `BLOCKED` / `NOT TESTED` entries
written when the audit had **no ai binary** (§14). Evidence: [`MODE-D-REPORT-1295.md`](./MODE-D-REPORT-1295.md),
[`PHASE7-VERIFY-1295.md`](./PHASE7-VERIFY-1295.md).

Legend as in `CONTRACT-MATRIX.md`: `PASS | FAIL | PARTIAL | BLOCKED | N/T | N/A`.

## Core Capability Matrix — changed rows only

| Capability | CLI | CLI+AI | MCP | MCP+AI | Real-Web |
| :--- | ---: | ---: | ---: | ---: | ---: |
| single-page | PASS | BLOCKED → **PASS** | PASS* | BLOCKED → **PASS** | PASS |
| multi-page crawl | PASS | BLOCKED → **PASS** | PASS* | BLOCKED → **PASS** | PASS |
| batch | PASS | BLOCKED → **PASS** | PARTIAL | BLOCKED → **N/A** (`--batch` is CLI-only; MCP has `scrape_batch`) | N/A |
| batch-file | PARTIAL | BLOCKED → **PARTIAL** (inherits the CLI row; not re-run with ai) | N/A | BLOCKED → **N/A** | N/A |
| sitemap | PASS | BLOCKED → **PASS** (MDN, 37 chunks) | FAIL | BLOCKED → **PASS** (`semantic_cleaner` fetches + embeds) | PARTIAL |
| resume | PARTIAL | BLOCKED → **PARTIAL** (no chunk duplication; "nothing new" writes no file — shared with the plain path, inside #1292) | N/A | BLOCKED → **N/T** | N/A |
| checkpoint | PARTIAL | BLOCKED → **N/T** (not re-run under ai) | N/A | BLOCKED → **N/T** | N/A |
| export | PASS | BLOCKED → **PASS** (jsonl + md + json) | FAIL | BLOCKED → **PARTIAL** (`semantic_cleaner` envelope carries inline vectors) | N/A |
| images | N/T | BLOCKED → **N/T** | N/A | BLOCKED → **N/T** | N/A |
| documents | N/A | BLOCKED → **N/T** | N/A | BLOCKED → **N/T** | N/A |
| robots | PASS | BLOCKED → **PARTIAL** (enforced, exit 69; message misclassified as WAF — **#1301**) | N/A | BLOCKED → **PASS** (AI tool names robots.txt before fetch) | PARTIAL |
| WAF | PARTIAL | BLOCKED → **PARTIAL** (inherits CLI classification; #1301 applies) | PASS | BLOCKED → **N/T** | N/A |
| retry | N/T | BLOCKED → **N/T** | N/T | BLOCKED → **N/T** | N/A |
| timeout | N/T | BLOCKED → **N/T** | N/T | BLOCKED → **N/T** | N/A |
| include/exclude | N/T | BLOCKED → **N/T** | N/T | BLOCKED → **N/T** | N/A |
| **JS rendering** | N/A → **PASS** | BLOCKED → **PARTIAL** | BLOCKED → **N/T** | BLOCKED → **N/T** | N/A → **PARTIAL** |
| **AI cleaning** | BLOCKED → **N/A** | BLOCKED → **PASS** | BLOCKED → **N/A** | BLOCKED → **PASS** | BLOCKED → **PARTIAL** (**MD-4**) |
| **AI failure** | BLOCKED → **PASS** (BIN1 honesty: exit 78 Spanish, hidden flags) | BLOCKED → **PASS** (5/5 modes) | PARTIAL | BLOCKED → **PASS** (honest `isError` when ai compiled but not enabled; no vault) | N/A |
| offline | N/T | BLOCKED → **PASS** (`--offline` warm = exit 0 for **both** tiers; cold = exit 78 Spanish, no silent fallback) | N/T | BLOCKED → **N/T** | N/A |
| cancellation | N/T | BLOCKED → **PASS** (SIGINT + SIGTERM drain, completed pages survive) | N/T | BLOCKED → **N/T** | N/A |
| configuration | PARTIAL | BLOCKED → **PASS** (`WEBFANG_THRESHOLD`, `WEBFANG_AI_MODEL_ID`, legacy `AI_MODEL_ID` all honoured and distinguished) | N/A | BLOCKED → **PASS** (`--enable-ai`, `WEBFANG_MCP_AI`) | N/A |
| SSRF | PASS | BLOCKED → **PASS** (pre-socket literal-IP rejection also on the ai build) | PASS | BLOCKED → **PASS** (`-32602` from `semantic_cleaner`) | N/A |
| observability | FAIL | BLOCKED → **PASS** (AI spans `resolve_model_assets`/`infer`/`prune_dom` in the trace JSONL) | N/A | BLOCKED → **N/T** | N/A |

**New row:** `AI cleaning × --output-vectors` — **FAIL** on both surfaces (**MD-1**: second fetch + a
different chunking, no join key between vectors and corpus).

## Input-class additions

### AI cleaning

| Input class | CLI+AI | MCP+AI | Result | Error |
| :--- | ---: | ---: | :--- | :--- |
| Static fixture page | PASS | PASS | 6–8 coherent chunks, 384-d | — |
| Real large page (wikipedia) | PARTIAL | N/T | chunks end mid-sentence (**MD-4 / #1313**) | — |
| Warm cache + `--offline` | PASS | N/T | identical output to online | — |
| Cold cache + `--offline` | PASS | N/T | refuses to run, writes nothing | exit 78, Spanish |
| Invalid model id (`--ai-model` / `AI_MODEL_ID` / `WEBFANG_AI_MODEL_ID`) | PASS | N/A | pre-fetch fail-fast | exit 64, Spanish, lists valid ids |
| `--ai-model ""` / `" GRANITE-97M "` | PASS | N/A | empty → documented default; whitespace+case trimmed | — |
| **`--ai-model granite-311m` (real 1.2 GB pull)** | **PASS** | N/T | 384-d kept (768→384), embeddings differ from 97M (l2 1.4911), flag beats env | — |
| **Cold start with a real download** | **PASS (outcome)** | N/T | 397 MB in 1m48s / 1.2 GB in 5m52s, exit 0 | — but **no progress without a TTY** (**MD-7 / #1316**) |
| **Corrupted cache (SHA mismatch)** | **PASS** | N/T | refuses to load, writes nothing | exit 78, Spanish, names expected/obtained digest |
| `--threshold` out of range | PASS | N/A | rejected at parse | exit 64, Spanish |
| `--threshold` over-aggressive | PASS | N/A | 1 chunk kept, visible WARN with loss ratio | — |
| `--max-tokens` under chunk size | PASS | N/A | honest failure, no silent fallback | exit 78, Spanish |
| `--clean-ai` on a non-ai build | PASS | N/A | preflight refusal | exit 78, Spanish |
| `--output-vectors` alongside `--clean-ai` | **FAIL** | N/A | vectors ≠ corpus chunks; duplicate fetch | — (**MD-1 / #1310**) |
| **Memory footprint per tier** | **PASS 97M / FAIL 311M** | N/T | 1.10 GiB vs **3.20 GiB** steady state (plan budget < 2 GB) | — (**MD-6 / #1315**) |

### JS rendering (F-52 family)

| Input class | CLI (`full`) | Result | Error |
| :--- | ---: | :--- | :--- |
| Real JS-only page (`quotes.toscrape.com/js/`) | PASS | DOM mutations absent from served bytes reach the markdown | — |
| Same page, `static` | PASS (honest) | run refuses instead of exporting an empty page | exit 65, Spanish |
| `--max-pages 4`, default concurrency | **FAIL** | Chrome `ProcessSingleton` collision on the fixed profile dir | exit 69 (**MD-3**) |
| `--max-pages 4 --concurrency 1` | PASS | 3/4 pages, 1 honest extraction failure | — |
| Real 404 with `full` | **FAIL** | 404 body exported as a successful page | exit 0 (**MD-2**) |

## Net effect on the audit's open questions

Opened by this run: **MD-1 → #1310**, **MD-2 → #1311**, **MD-3 → #1312**, **MD-4 → #1313**, **MD-6 → #1315**,
**MD-7 → #1316**. MD-5 (resume writes no file when nothing is new) was **not** filed: it reproduces identically
without `--clean-ai`, so it belongs to #1292.

- AUDIT-02 §14 "AI columns BLOCKED — binary lacks the ai feature": **closed**. No AI column remains `BLOCKED`.
- AUDIT-02 §14 "JS rendering BLOCKED — no Chrome": **closed** (F-52 environment premise already falsified in
  [`JS-RENDERING.md`](./JS-RENDERING.md); this run adds real-site crawl evidence on the F-52-c-pinned build).
- Newly opened scope for the remediation order: **MD-2 / [#1311]** (Critical, exit-code lie on the browser path)
  and **MD-1 / [#1310]** (High, vector/corpus desync) are the two that change what the product may claim about
  RAG exports; **MD-3 / [#1312]** makes `full` single-threaded in practice; **MD-4 / [#1313]** degrades retrieval
  quality on link-heavy pages.

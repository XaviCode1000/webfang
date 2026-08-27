# Real-World Manual Test Plan — WebFang

> **Purpose:** exercise webfang against REAL websites (not wiremock/hermetic fixtures)
> to find bugs that only appear in the wild: TLS fingerprinting, WAF challenges,
> malformed HTML, gzip sitemaps, JS-heavy SPAs, huge pages, hostile encodings.
>
> **Version under test:** post-v2.0.0 (commit 1f35b5d, 2026-08-17)
> **Methodology:** exploratory black-box. Every test = command + expected behavior +
> actual result + bug triage (severity, evidence, issue draft).
> **Rules:** only public sites; respect robots.txt (except tests marked `--ignore-robots`,
> against our own allowed targets); ≤2 concurrent requests per host; no authenticated
> private accounts; keep total crawl budgets tiny (`--max-pages`).

---

## 0. Environment facts (verified 2026-08-17)

| Fact | Value | Impact on plan |
| --- | --- | --- |
| Chrome/Chromium | **NOT installed** | `--js-strategy full` cannot run; Phase 4 limited to `static`/`hybrid`; preflight exit-78 IS testable |
| `obscura` binary | **NOT installed** | `--js-strategy hybrid` fallback to obscura untestable; static still works |
| HuggingFace cache | **empty** (`~/.cache/huggingface/hub/` doesn't exist) | First AI run downloads Granite-97M (~390 MB); Phase 7 includes offline-cache tests |
| Network to test targets | verified: example.com ✓, wikipedia ✓, httpbin ✓ (404 endpoint alive), quotes.toscrape.com ✓, books.toscrape.com ✓, badssl.com ✓, sannysoft.com ✓ | |
| quotes.toscrape.com/sitemap.xml | **404** (no sitemap) | use `books.toscrape.com/sitemap.xml` and wikipedia instead |
| Known open issues | #749 (MCP robots bypass — fix in flight), #724 (MCP follow-ups), #695 (minor CLI audit findings), #751 (CI-only) | retest findings against these before filing dupes |

### Builds required (do in this order, share `CARGO_TARGET_DIR`)

```bash
cargo build -p webfang_cli                                   # BIN1: plain (no ui/ai)
cargo build -p webfang_cli --features ui                     # BIN2: + TUI
cargo build -p webfang_cli --features ui,ai                  # BIN3: full (used for AI + final smoke)
cargo build -p webfang_mcp --bin mcp_server_http             # MCP server binary
# optional: cargo build -p webfang_mcp --features ai --bin mcp_server_http  (AI tools)
```

> BIN1 matters on purpose: some flags (`--clean-ai`, `--adaptive-selectors`) are
> feature-gated and must behave honestly (hidden/honest error) without the feature.

---

## 1. Target-site matrix

Each site exercises a distinct real-world failure class. Keep this matrix as the
source of truth for which site tests what.

| Code | Site | Real-world class | Health |
| --- | --- | --- | --- |
| S1 | `https://example.com` | minimal static, baseline sanity | 200 |
| S2 | `https://quotes.toscrape.com` | clean static, designed-scrape sandbox (login page present) | 200 |
| S3 | `https://books.toscrape.com` | static catalogue, HAS sitemap.xml, deep links | 200 |
| S4 | `https://en.wikipedia.org/wiki/Rust_(programming_language)` | huge page (~1 MB), complex tables, references, real sitemaps | 200 |
| S5 | `https://www.gutenberg.org/ebooks/65` | very large HTML, old-style markup | verify at test time |
| S6 | `https://news.ycombinator.com` | multi-page crawl w/ relative links, pagination | verify |
| S7 | `https://developer.mozilla.org/en-US/docs/Web/HTTP` | giant docs site, sitemap index, robots.txt rules | verify |
| S8 | `https://httpbin.org` | status codes: /status/404, /status/500, /redirect/5, /delay/10 | alive |
| S9 | `https://badssl.com` + subs | TLS pathologies: expired, self-signed, wrong.host, tls-v1-2-only | verify |
| S10 | `https://bot.sannysoft.com` | bot/WAF-detection content (JS-heavy, renders detection tables) | resolve ok |
| S11 | `https://www.wikipedia.org` | redirect + portal page, gzip, unicode titles | 200 |
| S12 | `https://quotes.toscrape.com/js/` | content rendered ONLY by JavaScript (JS-required case) | 200 |
| S13 | `https://httpbin.org/encoding/utf8` + latin-1 pages | charset edge cases | via S8 |
| S14 | sites with aggressive rate limiting / 429 (e.g. public APIs pages) — pick at test time | backpressure, retry/backoff realism | pick |
| S15 | `https://books.toscrape.com/catalogue/category/books/mystery/` | path-based include/exclude filtering | 200 |
| S16 | a page with large PDF/images (e.g. a gutenberg cover page, wikipedia images) | asset download realism | via S4/S5 |

---

## 2. Test phases

### Phase 1 — Build & smoke (no network)

| # | Test | Command | Expected |
|---|------|---------|----------|
| 1.1 | BIN1 builds | `cargo build -p webfang_cli` | exit 0, no warnings under CI lints |
| 1.2 | Help is complete & coherent | `./target/debug/webfang --help` | all documented flags present; feature-gated ones hidden in BIN1 |
| 1.3 | Version | `webfang --version` | matches workspace version |
| 1.4 | No-URL usage error | `webfang` (no args) | Spanish usage error, exit ≠ 0, no panic |
| 1.5 | Invalid URL | `webfang --url "notaurl"` | `Invalid URL: notaurl` usage error (exit 2), NOT a panic (CrawlOptions::from historically panicked) |
| 1.6 | Completions | `webfang completions bash` (and fish/zsh) | valid shell output, exit 0 |
| 1.7 | Flag validation | `webfang --url https://example.com --timeout-secs 0` ; `--download-concurrency 0` ; `--batch-concurrency 0` ; `--cpu-cores 0` ; `--ram-budget 0 ; --ram-budget nonsense` | Spanish validation error each, exit ≠ 0 |
| 1.8 | Legacy TUI flags on BIN1 | `webfang --tui` on BIN1 (no `ui` feature) | rejection/honest message, no panic |

**Bug classes:** panic on bad input, English leaks into user errors, exit codes wrong.

### Phase 2 — CLI core scraping (real sites)

| # | Test | Command | Expected |
|---|------|---------|----------|
| 2.1 | Baseline markdown | `webfang --url https://example.com -o out/2.1` | 1 file, valid Markdown containing "Example Domain"; exit 0 |
| 2.2 | Selector extraction | `webfang --url https://quotes.toscrape.com --selector "span.text" -o out/2.2` | only quote texts, no chrome/nav |
| 2.3 | Non-matching selector | `webfang --url https://quotes.toscrape.com --selector "div.does-not-exist" -o out/2.3` | graceful empty/warning result — NOT panic, NOT silent success (#695) |
| 2.4 | Bad selector syntax | `webfang --url https://quotes.toscrape.com --selector "[[[invalid"` | clear error, exit ≠ 0 |
| 2.5 | Huge page + table fidelity | `webfang --url https://en.wikipedia.org/wiki/Rust_(programming_language) -o out/2.5 -vv` | completes; Markdown tables non-garbled; unicode ok; memory sane |
| 2.6 | Output formats | repeat 2.1 with `-f text` and `-f json` | text = plain; json = valid parseable JSON |
| 2.7 | Quiet mode | `webfang --url https://example.com -q` | no info output; stderr clean |
| 2.8 | 404 handling | `webfang --url https://httpbin.org/status/404 -o out/2.8` | Spanish user error mentioning 404/status; exit code documented; no panic |
| 2.9 | 500 handling | `webfang --url https://httpbin.org/status/500 --max-retries 2` | retries visible at `-vv`, then clean failure |
| 2.10 | Redirect chain | `webfang --url https://httpbin.org/redirect/5 --timeout-secs 30` | follows redirects to final 200 page |
| 2.11 | Slow endpoint vs timeout | `webfang --url https://httpbin.org/delay/10 --timeout-secs 5` | clean timeout error in Spanish, exit ≠ 0, no hang |
| 2.12 | DNS failure | `webfang --url https://this-domain-definitely-does-not-exist-12345.com` | clear DNS error, exit ≠ 0 |
| 2.13 | Batch from stdin | `printf 'https://example.com\nhttps://quotes.toscrape.com\n' \| webfang --batch -o out/2.13` | 2 result files, per-URL error isolation |
| 2.14 | Batch file with bad line | file: valid URL + `not a url` + blank line | bad line reported, good lines processed, no abort |
| 2.15 | Batch concurrency 0 | `--batch --batch-concurrency 0` | validation error (covered by 1.7, confirm in batch path) |

**Bug classes:** selector edge crashes, panic-on-malformed-HTML, retry loop bugs, exit-code lies (success despite failure), markdown corruption on tables/unicode.

### Phase 3 — Discovery, crawling & sitemaps

| # | Test | Command | Expected |
|---|------|---------|----------|
| 3.1 | Dry-run discovery | `webfang --url https://quotes.toscrape.com --dry-run -n` | URLs listed, NO files written, no scraping |
| 3.2 | max_pages limit | `webfang --url https://books.toscrape.com --max-pages 3 --max-depth 2 -o out/3.2 -v` | exactly ≤3 pages scraped, stop announced |
| 3.3 | depth 0 = seed only | `webfang --url https://books.toscrape.com --max-depth 0 --single-page -o out/3.3` | only index page |
| 3.4 | include-pattern path | `webfang --url https://books.toscrape.com --include-pattern "/catalogue/category/books/mystery/*" --max-pages 5 --dry-run` | only mystery URLs listed |
| 3.5 | exclude-pattern wins | `webfang --url https://books.toscrape.com --exclude-pattern "*/page-*.html" --dry-run --max-pages 5` | no pagination URLs |
| 3.6 | sitemap discovery | `webfang --url https://books.toscrape.com --use-sitemap --dry-run` | discovers URLs from sitemap.xml (quotes.toscrape has NO sitemap — wrong-site trap) |
| 3.7 | explicit sitemap url | `webfang --url https://books.toscrape.com --sitemap-url https://books.toscrape.com/sitemap.xml --dry-run` | same, explicit path |
| 3.8 | missing sitemap fallback | `webfang --url https://quotes.toscrape.com --use-sitemap --dry-run` (#695 exit-code finding) | graceful: fallback to link discovery OR honest sitemap error + correct exit code |
| 3.9 | robots.txt respect | `webfang --url https://developer.mozilla.org --dry-run -v` | respects Disallow rules (log lines show filter), no disallowed URLs |
| 3.10 | resume mode | run 3.2, Ctrl-C mid-run, re-run with `--resume` | second run skips already-done URLs (StateStore), finishes remaining |
| 3.11 | checkpoint flags | `--checkpoint-interval 1` on 3.2 crawl | checkpoint saves logged; no crash |
| 3.12 | Ctrl-C mid-crawl | Ctrl-C during a 10-page crawl | clean shutdown, partial output intact, no corrupt half-written files, exit code sensible |

**Bug classes:** pattern-glob mismatches (#695 exit finding), sitemap-404 mishandling, resume state corruption, panic on mid-run interrupt, robots enforcement gaps (cf. #749 is MCP-only — verify CLI side is clean).

### Phase 4 — WAF evasion & JS rendering

> No Chrome installed → `full` strategy is **expected to fail the preflight (exit 78)**.
> That failure IS the test.

| # | Test | Command | Expected |
|---|------|---------|----------|
| 4.1 | TLS fingerprint baseline | `webfang --url https://bot.sannysoft.com -o out/4.1 -f markdown` | fetches without WAF challenge block (real fingerprint test) |
| 4.2 | TLS expired cert | `webfang --url https://expired.badssl.com` | clean TLS error in Spanish (no raw OpenSSL dump) |
| 4.3 | TLS wrong host | `webfang --url https://wrong.host.badssl.com` | hostname mismatch error, clean |
| 4.4 | h2 profile variation | `webfang --url https://example.com --h2-profile Chrome145` and bogus `--h2-profile Firefox999` | 1st works; 2nd clear validation error |
| 4.5 | JS-required page, static | `webfang --url https://quotes.toscrape.com/js/ --single-page -o out/4.5` | honest result: empty/minimal content + (ideal) warning that page looks JS-rendered — compare vs 4.6 |
| 4.6 | JS page with hybrid (no obscura) | `webfang --url https://quotes.toscrape.com/js/ --js-strategy hybrid -o out/4.6` | graceful degradation or honest error — NO hang, NO panic |
| 4.7 | full strategy preflight | `webfang --url https://example.com --js-strategy full` | **exit 78** with Spanish message that Chrome is missing (#685 preflight) — before any request |
| 4.8 | WAF challenge detection | target a Cloudflare-fronted public page known to challenge (pick at test time, e.g. some docs/CDN page) | detection triggers honest error/warning; `--ignore-waf` bypass documented in output |
| 4.9 | User-Agent override | `webfang --url https://httpbin.org/headers --user-agent "Mozilla/5.0 (TestBot)" -o out/4.9` | output shows the UA we sent |
| 4.10 | Custom headers/cookies | `webfang --url https://httpbin.org/headers --cookie "session=abc123" -H "X-Test: 1"` | httpbin response echoes both |

**Bug classes:** silent-empty-on-JS pages with no warning (worst UX bug), preflight gaps, TLS panic leakage, fingerprint regressions.

### Phase 5 — Exports, assets & Obsidian

| # | Test | Command | Expected |
|---|------|---------|----------|
| 5.1 | JSONL RAG export | `webfang --url https://quotes.toscrape.com --max-pages 3 --export-format jsonl -o out/5.1` | valid JSONL, one record per page/chunk |
| 5.2 | Output dir variants | `--output ./nested/deep/dir` (nonexistent) | created, or honest error; never panic |
| 5.3 | Image download | `webfang --url https://books.toscrape.com/index.html --download-images -o out/5.3` | images saved under assets dir, naming strategy applied |
| 5.4 | Asset naming strategies | repeat 5.3 with `--asset-naming hash` / `slug` / `content-disposition` | each produces its documented naming; collisions handled |
| 5.5 | Documents download | `webfang --url <page with a public PDF> --download-documents -o out/5.5` | PDF saved, size > 0, not HTML error page |
| 5.6 | max_file_size guard | `--max-file-size 1024` + 5.3 | large images SKIPPED with log line, small ones kept |
| 5.7 | Obsidian export | `webfang --url https://quotes.toscrape.com --max-pages 2 -o ~/obsidian-test-vault` (create an empty vault dir first) + `WEBFANG_VAULT` semantics | `[[wiki-links]]`, frontmatter, correct excerpt (#695 excerpt finding — retest!) |
| 5.8 | Obsidian vault detection edge | output into a non-vault dir that merely contains .md files | detection heuristic behaves per #745 (hermetic detection), report if it misfires |
| 5.9 | Obsidian + selector combined | `--selector "span.text"` + obsidian output | clean quote notes, no double-processing artifacts |

**Bug classes:** silent image download failures, path-join panics, excerpt duplication (#695), vault heuristics false positives.

### Phase 6 — MCP server (HTTP + stdio, 35 tools)

> Server: `./target/debug/mcp_server_http` (port 8080 default). Probe via `curl -X POST http://127.0.0.1:8080/mcp` JSON-RPC or an MCP client (OpenCode). The 8 handler categories: **scraping** (scrape_url, scrape_with_options, scrape_batch, crawl_site, crawl_with_sitemap, discover_urls, discover_sitemap, detect_spa), **content** (clean_html, convert_html_to_markdown, convert_wiki_links, extract_links, generate_frontmatter, generate_rich_metadata, highlight_code_blocks), **export** (export_file, export_jsonl, export_vector), **url_utils**, **security** (SSRF filter), **obsidian** (vault detect + search), **assets** (download_assets), **ai** (semantic_cleaner, search_obsidian).

| # | Test | Expected |
|---|------|----------|
| 6.1 | Boot loopback, no token | starts, warns "development mode" |
| 6.2 | External bind without token | `--bind 0.0.0.0:8080` → refused to start (REQ-06 fail-fast) |
| 6.3 | External bind with token | starts; requests without `Authorization: Bearer` → 401 |
| 6.4 | tools/list | exactly 35 tools, all categories present |
| 6.5 | scrape_url happy | `scrape_url https://quotes.toscrape.com` → real content back |
| 6.6 | scrape_url SSRF | loopback/private/169.254.169.254 targets → rejected (isError or invalid params) |
| 6.7 | scrape_batch multi-domain | batch example.com + books.toscrape.com → metrics attributed per real domain (regression #696) |
| 6.8 | scrape_batch zero limits | `max_pages: 0` → rejected (zero-limit validation) |
| 6.9 | discover_urls / detect_spa on S12 | detect_spa flags the JS page; discover returns honest result |
| 6.10 | crawl_with_sitemap | S3 sitemap; internal-only filtering holds |
| 6.11 | content tools | convert_html_to_markdown / clean_html / extract_links / generate_rich_metadata on real HTML from S4 — verify spanish detection, reading time, word count sane |
| 6.12 | export_file happy | `output_dir` relative → file written; read it back |
| 6.13 | export_file absolute (no roots) | `output_dir=/tmp/mcpexp` WITHOUT `--export-roots` → rejected fail-closed (#696 fix) |
| 6.14 | export_file absolute (with roots) | restart server `--export-roots /tmp/mcpexp` → allowed; sibling `/tmp/mcpexpX` and `/tmp/mcpexp/../etc` → rejected |
| 6.15 | filename traversal | `filename: "../../evil.jsonl"` → sanitized/rejected |
| 6.16 | download_assets | real page images → files land in `./downloads`, shared pool reused |
| 6.17 | rate limiting | burst 30 rapid requests → 429 after budget, then recovers |
| 6.18 | body limit | oversized JSON-RPC body (>10 MB) → clean 413-style error |
| 6.19 | malformed JSON-RPC | garbage body → JSON-RPC error object, server keeps running |
| 6.20 | robots.txt enforcement | crawl tool against a site with Disallow → blocked URLs skipped; **cross-check #749/#724** to see which of the 5 tools are fixed in THIS build |
| 6.21 | stdio transport | `mcp_server_stdio` spawned by a client (or manual stdin JSON-RPC echo): initialize → tools/list → one scrape works; stdout pure JSON-RPC, logs on stderr |
| 6.22 | obsidian tools | vault detect + search on a test vault (needs embeddings → without `--enable-ai` expect honest feature error, not crash) |
| 6.23 | concurrency hammer | 10 simultaneous scrape_url to S1 | all complete or rate-limited; no panics, no connection-pool deadlock |

**Bug classes:** auth fail-open, SSRF gaps, export-roots bypass (#696 regression), honest-error contracts broken, stdio/stdout pollution.

### Phase 7 — AI semantic cleaning (BIN3, feature `ai`)

> No HF cache → first run downloads Granite-97M. Test both the cold and warm path.

| # | Test | Command | Expected |
|---|------|---------|----------|
| 7.1 | Cold start | `webfang --url https://quotes.toscrape.com --clean-ai --max-pages 1 -o out/7.1` | model downloads to `~/.cache/huggingface/hub/`, honest progress, then chunks produced |
| 7.2 | Chunk quality | same on S4 (wikipedia) | chunks are semantically coherent paragraphs, not mid-sentence garbage; relevance scores present |
| 7.3 | Explicit output vectors | `--clean-ai --output-vectors ./vecs.jsonl` | JSONL with 384-dim vectors; count ≈ chunk count |
| 7.4 | AI via stdout vectors | `--output-vectors -` | vectors on stdout, human log on stderr, no cross-pollution |
| 7.5 | Model override | `--ai-model IBM/watsonx-granite-embedding-311m-multilingual` (or `WEBFANG_AI_MODEL_ID`, legacy `AI_MODEL_ID`) | downloads/uses 311M model, vectors 384-dim, honest about size |
| 7.6 | Offline + warm cache | disconnect network (or `HTTP_PROXY=http://127.0.0.1:9`) with cache present | cache-hit, works offline |
| 7.7 | Offline + cold cache | clear hub cache + no network | honest Spanish error: model unavailable offline; NO panic, NO silent fallback to garbage |
| 7.8 | ChunkTooLarge | feed a page with a single >32768-token text block (craft or find) | SemanticError::ChunkTooLarge honest message |
| 7.9 | AI + batch | 2 URLs batch + `--clean-ai` | both processed, vectors per URL |
| 7.10 | AI without feature (BIN1) | `webfang --clean-ai` on BIN1 | flag hidden/ignored honestly per cfg(placeholder) — verify no "unknown flag" crash either way |
| 7.11 | MCP AI tools | `mcp_server --enable-ai` → `semantic_cleaner` + `search_obsidian` over HTTP | real embeddings, honest when disabled |
| 7.12 | Memory footprint | `--clean-ai` on S4 with `/usr/bin/time -v` | RSS sane (<~2 GB), mmap used not loaded |

**Bug classes:** download-failure panics, offline-mode lies, dimension mismatches, memory blowups, cache corruption handling.

### Phase 8 — TUI / UI (BIN2+, feature `ui`)

> Interactive — run in a real terminal (tmux pane ok). Record with `script`/asciinema if useful.

| # | Test | Steps | Expected |
|---|------|-------|----------|
| 8.1 | Unified TUI launch | `webfang --tui`, enter a URL | config form renders (no garbled borders), navigable |
| 8.2 | URL selector widget | reach URL Selector phase (Space/Enter/A/D/q keys) | checkboxes toggle, counter updates, scroll works on long lists |
| 8.3 | Multi-select → batch | select 2+ URLs, Enter | temp batch file created (uuid v7), crawl proceeds for both |
| 8.4 | Cancel path | q at selector | "TUI cancelled.", exit 0, no partial state |
| 8.5 | Non-interactive terminal | `webfang --tui < /dev/null` (no TTY) | honest error, no panic/rendering garbage |
| 8.6 | Deprecated flags | `--config-tui`, `--interactive` | deprecation warning pointing to `--tui`, still works |
| 8.7 | TUI + bad URL | enter invalid URL in form | inline validation, no crash |
| 8.8 | TUI under slow network | crawl S5 via TUI | progress visible, no input lockup |
| 8.9 | Terminal resize | resize pane mid-render | re-renders, no panic |
| 8.10 | Chrome preflight in TUI | set js_strategy=full in config form (#724 preflight finding) | honest "no Chrome" message inside TUI, no hang |

**Bug classes:** ratatui panics on resize/bad UTF-8, key event deadlocks (#741-era bugs), temp-file leaks.

### Phase 9 — Observability & stress

| # | Test | Command | Expected |
|---|------|---------|----------|
| 9.1 | Trace file | `webfang --url https://books.toscrape.com --max-pages 5 --trace-file out/9.1.jsonl` | valid JSONL; every span carries `trace_id`; `jq 'select(.fields.trace_id==...)'` reconstructs the crawl |
| 9.2 | Error correlation | force an error mid-crawl + trace-file | `log_scrape_error` fields present (stage, url, correlation_id), no bare warn! |
| 9.3 | Verbosity levels | `-v` / `-vv` / `-vvv` | escalating detail; no secret/PII leakage at any level |
| 9.4 | Structured fields | check JSONL | no `format!` data soup inside messages; fields are structured |
| 9.5 | Concurrency stress | S3 with `--concurrency 8` (site allows it) + 9.1 trace | no deadlocks, all pages land, trace coherent |
| 9.6 | Timeout under load | S4 + `--timeout-secs 3` | per-page timeouts handled, run completes with partial results honestly reported |

---

## 3. Bug capture protocol

For every failure found, file one entry per row below (then promote real bugs to
GitHub issues with `type:bug` and one `priority:*` label):

| Field | Content |
| --- | --- |
| Test ID | e.g. `2.3` |
| Site | S-code + URL |
| Command | full reproducible command |
| Expected | from this plan |
| Actual | what happened (exit code, message, panic backtrace if any) |
| Evidence | file in `out/<test-id>/`, trace JSONL excerpt, screenshot (TUI) |
| Severity | Critical (panic/data-loss/lie) / High (broken feature) / Medium (bad UX) / Low (cosmetic) |
| Dupe check | cross-ref #695 / #724 / #749 / #751 before filing |
| Suggested title | `fix(<scope>): <spanish-or-english per repo convention>` |

**Triage rules:**
- Exit-code lies (claims success with broken output) = **Critical**.
- Panics on malformed remote HTML = **Critical** (remote-triggered).
- Silent empty output without warning (JS pages, selector miss) = **High**.
- English leaking into user-facing errors = **Medium** (repo policy: Spanish errors).
- Anything reproducible via MCP from an UNTRUSTED client = security, escalate (#696-class).

**Evidence policy:** keep `out/` gitignored; copy only redacted excerpts into issues
(no full paths, no tokens).

---

## 4. Execution order & priorities

1. **Phase 1** (smoke) then **Phase 2** (CLI core) — this is the product's spine.
2. **Phase 6** (MCP) before Phase 7, because MCP is the most attack-surface-heavy surface and #749/#724 findings are open.
3. **Phase 4** (WAF/JS) right after core — fingerprinting is webfang's differentiator.
4. **Phase 7** (AI) — needs ~400 MB download, run when bandwidth allows.
5. **Phase 3** (discovery), **5** (exports), **8** (TUI), **9** (stress) interleave as capacity allows.

Total: ~70 individual checks. Estimated wall time: 4–6 h excluding the AI model
download and any installed-Chrome follow-up (out of scope this environment).

## 5. Out of scope (documented, not forgotten)

- `--js-strategy full` real rendering (no Chrome installed) — retest when Chrome available.
- `hybrid` obscura layer (binary absent).
- Elastic ingestion (`--elastic`) real Elastic endpoint — needs a running Elastic; only flag-validation tested here.
- Authenticated crawls against private accounts — use `httpbin.org/headers` echo only.
- Windows/macOS behaviors — Linux-only environment.

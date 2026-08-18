# Explore — #793 Obscura L2: HTML dump + version contract + stateless scope

## Problem surface (from audit + code)

1. **Format mismatch** — `obscura_downloader.rs` fetches with `fetch --dump markdown`
   and stores the result in `FetchedPage.html`. Downstream consumers
   (`infrastructure/scraper/readability.rs`, `application/extraction.rs`
   `extract_with_selector`, `waf_engine.rs` `InspectionContext`) all assume HTML:
   CSS selectors cannot apply to markdown text, and Readability loses structure
   when fed `# heading` instead of `<h1>`. `FetchedPage.headers` is returned empty,
   so nothing downstream can tell markdown from HTML.
2. **No version contract** — `cli/preflight.rs` `check_obscura_binary` (#787)
   verifies the binary *exists* (path or PATH resolution) but never its version.
   An older obscura could change `--dump` semantics silently.
3. **Stateless L2** — module docs say "no cookies, no connection pool". L3
   (`chromiumoxide_downloader.rs`) injects `CookieBridge` cookies by domain.
   Session-gated pages therefore render empty at L2 and only resolve at L3.

## Existing seams to build on (#787, already merged in this worktree)

- `ObscuraDownloader::new(timeout_secs, binary)` — binary is a `PathBuf`; bare name
  resolves via PATH, explicit path invoked as-is. Test accessor `binary()`.
- `preflight.rs` pure helpers: `has_path_separator`, `resolve_executable_in_path`,
  `resolve_obscura_binary`, `check_obscura_binary(binary, path_value)` — PATH is
  injected as a value, no process-global env mutation in tests.
- `hybrid_router.rs` L2 branch: `Ok(page) if !page.html.is_empty()` → return (stop).

## Evidence gathered

- `obscura fetch --help` (obscura 0.2.0) confirms `--dump html` is supported.
- `InspectionContext::from_lowercase_headers` lifts a lowercase `content-type`
  header key — the honest marker for a subprocess downloader with no response
  headers is to set `content-type: text/html`.
- `FetchedPage.headers` doc explicitly permits subprocess downloaders to leave
  it empty; adding a known synthetic content-type is the simplest honest marker.

## Open questions resolved

- **Marker approach:** use the `headers` map with `content-type: text/html`.
  Adding a new `FetchedPage` field would ripple through every stub/test double
  (high blast radius) for no downstream benefit — both WAF engine and sniffing
  already key off `content-type`.
- **Version gating policy:** missing binary → hard ConfigError (exit 78, existing);
  `--version` fails/unparseable → warn + continue (best-effort, unknown builds);
  parsed < 0.2.0 → ConfigError exit 78 naming found vs required. Hard-gating on
  unparseable output would break exotic/custom obscura builds.
- **Cookies at L2:** decided as a documented scope boundary (see design.md §4),
  not wired here. Obscura's `--storage-dir` adds real complexity (profile format
  coupling with L1, lifecycle, size) for a best-effort middle layer.

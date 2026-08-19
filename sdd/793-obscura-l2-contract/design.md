# Design — #793 Obscura L2 contract

## 1. Layer 2 HTML dump + content-type marker

- `fetch_inner` arg change: `["fetch", "--dump", "markdown", &url]` →
  `["fetch", "--dump", "html", &url]`. Variable renamed `markdown` → `html`;
  `DownloadError::Internal` on non-zero exit unchanged.
- Marker: `headers: HashMap::from([("content-type".into(), "text/html; charset=utf-8".into())])`.
  Rationale: `FetchedPage.headers` keys are lowercase by contract
  (`InspectionContext::from_lowercase_headers`, `discovery.rs`
  `headers_to_header_map`); `is_html_content_type` accepts `text/html` with a
  `;` suffix. A subprocess downloader has no real response headers, so a
  synthetic content-type describing what we *asked obscura to dump* is the
  honest invariant — if a future obscura regresses to non-HTML, the version
  gate (REQ-OBS-02) is the fence, not the header.
- Rejected alternative: new `FetchedPage.format` field — touches every
  test-double constructor (`hybrid_router.rs`, `discovery.rs`) for zero
  downstream benefit; both consumers already key off `content-type`.

## 2. Version probe design (preflight.rs)

```text
check_obscura_binary(binary, path_value)          # #787 — unchanged contract
  └─ resolve_obscura_binary → Option<PathBuf>      # #787 — unchanged
       ├─ None  → ConfigError (exit 78)            # unchanged
       └─ Some(resolved)
            ├─ probe_obscura_version(&resolved) → Option<(u64,u64,u64)> + raw String
            │     std::process::Command::new(resolved).arg("--version").output()
            ├─ parse_obscura_version(raw) → Option<(u64,u64,u64)>   [pure]
            ├─ assess_obscura_version(opt) → VersionVerdict {Meets,TooOld,Unknown} [pure]
            ├─ TooOld  → ConfigError exit 78 (found vs required, ES)
            ├─ Unknown → tracing::warn! (EN fields) + continue
            └─ Meets   → tracing::info! obscura_dependency_checked
                          (adds field `version = "x.y.z"`, was: no version field)
```

- `MINIMUM_OBSCURA_VERSION = (0, 2, 0)` constant + `Display` helper.
- Parse rule: iterate whitespace-split tokens; for each, strip a leading `v`/
  `V`, split on `.`; first three dot-segments must be plain ASCII digits →
  semantic triple. Trailing `-alpha`/`+build` on the patch segment is cut off
  before digit parsing. First matching token wins (obscura prints
  `obscura 0.2.0`).
- Probe is a blocking `Command::output()` — acceptable per the module's
  existing contract ("Runs once, in a synchronous context, before crawl
  start — a brief blocking `--version` probe is acceptable there"; precedent:
  `binary_reports_version` for Full).
- No env mutation anywhere: the resolved path comes from the injected
  `path_value` (#787 pattern); tests create temp-dir fake binaries and pass
  explicit paths.

## 3. Error/observability contract

- User-facing errors: Spanish, exit 78, names found version, required version,
  `--obscura-binary`, `WEBFANG_OBSCURA_BINARY`.
- Tracing: English fields only. `tracing::warn!(binary, resolved, output,
  "obscura_version_unreadable — continuing best-effort")`; info event gains
  `version` field so `--trace-file` JSONL captures the contract.
- No new error variants needed: version mismatch is a config error — reuses
  `CliExit::ConfigError` (exit 78), consistent with missing-binary (#787).

## 4. Stateless L2 — scope decision (keep/drop justification)

**Decision: L2 remains sessionless. L3 (Chromiumoxide + `CookieBridge`) is
the cookie path.** Not wired: `--storage-dir`.

Tradeoffs considered:
1. **Profile-format coupling.** Obscura's storage dir is a headless-browser
   profile; `CookieBridge` currently exports a name/value/domain list consumed
   by `SetCookiesParams` (CDP). Bridging the two means either (a) converting
   `CookieBridge` into the profile on disk (fragile across obscura/Chromium
   versions, untested territory) or (b) running obscura with `--dump cookies`
   round-trips per crawl (N+1 subprocesses, race with L3's own session state).
2. **L2's value axis is memory (30 MB vs 200 MB), not completeness.** The
   issue's own framing: sessionless-but-correct-HTML L2 still resolves
   "JS-rendered public pages cheaply". Session-gated pages already escalate
   to L3 by design (empty-at-L2 is a correct escalation trigger, not a bug,
   once HTML is honest).
3. **Honest observability over silent gaps.** With `--dump html` + content-type
   marker + version gate, every L2 result is now verifiable; if keep/drop data
   (#788 AXTree roadmap) later shows session-gap pain on L2-eligible pages,
   `--storage-dir` can be designed against real numbers.

Documented in: module docs (obscura_downloader.rs), this file, and
`hybrid_router.rs` fetch-strategy doc (markdown → HTML correction).

## 5. Test design (deterministic; no real obscura, no network)

Fake-binary fixture: a temp-dir `obscura` shell script (`chmod +x` in setup):
`--version` → `obscura 0.2.0` (or a parametric variant); `fetch` echoes its
args + prints fixed HTML on stdout. Drives:

- Downloader passes `--dump html` (script writes args to a file; test asserts
  the captured argv, and stdout HTML reaches `FetchedPage.html`).
- Extraction survival: captured `FetchedPage.html` →
  `extract_with_selector(.., "article", None)` = `Matched`, and
  `readability::parse(..)` = Ok article containing the fixture text.
  (`scraper_service` path is reachable without Chrome but requires a full
  `scrape_with_config` wiring; the extraction functions ARE the router's
  downstream contract, so unit-level survival + S4.1 branch test covers it.)
- Version gate: fake binaries printing 0.2.0 / 0.1.9 / garbage / exiting 1 →
  pass / exit-78 / warn-pass / warn-pass. Pure parse tests cover
  v-prefix, suffix, malformed input.
- REQ-OBS-04: `hybrid_router.rs` stub test — L1 SPA shell, L2 non-empty HTML,
  L3 fails if touched → router returns L2 content.

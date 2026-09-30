# F-52 — JS rendering with `--js-strategy full`

**Item:** F-52, blocked in AUDIT-02 §14 with cause "no Chrome in this environment"
(repeated verbatim in [`REVERIFICATION.md`](./REVERIFICATION.md) row
"Chromium / `--js-strategy full` (F-52)").
**Verdict:** **NOT blocked on environment** — the premise is falsified and the positive
path is proven with a real browser. Verification surfaced three defects: one fixed
here (crawl path ignored the strategy entirely), two left open with root causes.

| Field | Value |
|---|---|
| Branch / worktree | `feat/f52-js-rendering` @ `~/Projects/Rust/webfang-worktrees/feat-f52-js-rendering` |
| Base SHA | `409379d58f18171e444bb8e52b088c6272326509` (`origin/main` after `git fetch`) |
| Rust | 1.88.0 (repo pin in `rust-toolchain.toml`) |
| Build | `cargo build -p webfang_cli --bin webfang --features chromium`, debug, **7m15s** cold, ~7s incremental |
| Chrome (preflight hit) | `Google Chrome for Testing 152.0.7977.42` — `~/.local/bin/google-chrome` |
| Chrome (actually launched) | `Google Chrome 152.0.7977.82` — `/usr/bin/google-chrome-stable` |
| Render proof | `UA=... HeadlessChrome/152.0.0.0 ... VENDOR=Google Inc. VIEWPORT=800x600` |

## Build recipe deviation (read this first)

The prescribed recipe as written **fails in 0.6 s** on this machine:

```text
rust-lld: error: unknown argument '--reduce-memory-overheads'
error: could not compile `proc-macro2` / `quote` (build script)
```

Cause: the harness exports `RUSTUP_TOOLCHAIN=stable` (1.97.1), which overrides the
`rust-toolchain.toml` pin to 1.88.0. rustc ≥ 1.90 defaults to `rust-lld`, and both
`--no-keep-memory` and `--reduce-memory-overheads` are BFD-only. Fix: add
`-u RUSTUP_TOOLCHAIN` to the existing `env -u RUSTC_WRAPPER`; the recipe then runs
verbatim. (Alternative on a modern toolchain: drop the flags, or `-C linker-features=-lld`.)
`env -u RUSTC_WRAPPER` was required **for this run** — `mise.toml` then declared
`[env] RUSTC_WRAPPER = "sccache"`. **Superseded 2026-09-29:** that declaration has since
been removed; `mise.toml`'s `[env]` block is now comments-only and deliberately leaves
`RUSTC_WRAPPER` unset. The wrapper's absence is the repository's current configuration — do
not restore it, and note `mise.toml` is byte-identical across main, every worktree and every
clone, so a declaration there could not be opted out of per tree. The unset is now a guard (a
shell can inherit the variable from its parent), not a fix for a live injection.

```bash
export CARGO_TARGET_DIR=~/.cache/cargo-target/feat-f52-js-rendering
export CARGO_BUILD_JOBS=2
export RUSTFLAGS="-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads"
env -u RUSTC_WRAPPER -u RUSTUP_TOOLCHAIN cargo build -p webfang_cli --bin webfang --features chromium
```

## Method

Fixtures are served by `python3 -m http.server 8731 --bind 127.0.0.1`; loopback needs the
three documented test hatches (`domain/ssrf_guard.rs:53-82`), same method as
`REVERIFICATION.md` — with guards on, loopback exits 69 with zero pages and proves nothing.

```bash
WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1 WEBFANG_DISABLE_SSRF_RESOLVER=1 \
WEBFANG_DISABLE_SSRF_REDIRECT_GUARD=1 \
webfang --js-strategy full --single-page --timeout-secs 40 -o . http://127.0.0.1:8731/sync.html
```

Fixtures in [`fixtures/f52/`](./fixtures/f52): `sync.html` and `d0.html` mutate the DOM
before load resolves, `d400.html` 400 ms after, `ua.html` writes `navigator.userAgent`.
Every JS-injected string is absent from the served bytes, so its presence in the
markdown can only come from an executed render.

## 1. Preflight — the §14 premise is false

`check_js_dependencies` (`cli/preflight.rs:850`, called from `webfang_cli/src/main.rs:165`)
**passes: exit 0, not 78.** `DEFAULT_CHROME_CANDIDATES` (`preflight.rs:799`) is
`["google-chrome", "google-chrome-stable", "chromium-browser", "chromium"]`, and both
first two resolve here. Negative control `PATH=/nonexistent-dir` → **exit 78**
`--js-strategy full requiere Google Chrome instalado`, so the gate is live, not vacuous.

## 2. Real render, scrape path — PASS

| strategy | served placeholder | JS-injected marker |
|---|---|---|
| `static` | present | absent |
| `full` | **gone** | **present** |

`full` on `ua.html` yields `HeadlessChrome/152.0.0.0` + `VENDOR=Google Inc.`; `static`
yields `RAW_SERVER_RESPONSE_HAS_NO_USER_AGENT_STRING`. A wrapper on `CHROME=` logged
`LAUNCH argv1=--disable-background-networking`, confirming a real Chrome child process.

## Defects found

### F-52-a — crawl mode ignored `--js-strategy` entirely — **FIXED here**

Same build, same URL, `--single-page` renders; a crawl does not:

| invocation (`--js-strategy full`, `d0.html`) | rendered |
|---|---|
| `--single-page` | yes |
| crawl, `--max-pages 1` (no checkpoint, no sink) | **no** — byte-identical to `static` |
| crawl, `--checkpoint-interval 1` | **no** |

`cli/url_discovery.rs` built `EngineOptions` with `..EngineOptions::default()`, and the
default `js_strategy` is `Static` — the operator's flag never reached the Engine. The
degradation is silent: exit 0, `Finished: 1 total, 1 succeeded`, because
`Engine::with_js_strategy` only *records* a strategy whose router it cannot build
(`engine.rs:369-377` documents exactly this trap). The scrape path was unaffected
because `cli/scrape_flow.rs:224` builds its router straight from `CrawlOptions` — which
is why the gap had gone unnoticed.

Causality was proven with a throwaway one-line probe (`rendered=1` flipped), then shipped
as `build_discovery_engine_options()` with two unit tests. Red-check confirmed: deleting
the propagation line fails `--js-strategy hybrid must reach EngineOptions`.
Post-fix crawl matrix: `static`/`d0` → placeholder; `full`/`d0` → rendered;
`full`/`d400` → placeholder (F-52-b).

Tests: `--lib cli::url_discovery` 4/4, `--lib cli::` 262/262, `--test behavioral` 141/141,
`discovery_capture_1229` / `discovery_parity_1232` / `discovery_determinism_1237` /
`engine_js_strategy_timeout_test` all pass; `cargo fmt --check` and
`cargo clippy -p webfang_core --features chromium` clean.
(First behavioral run failed 2 timeout-based batch tests under load; both pass in
isolation and in a quiet full run — load flakes, not this change.)

### F-52-b — no settle wait: post-load hydration is lost — **OPEN**

Delay sweep, 3 repeats each, `--js-strategy full`:

| `setTimeout` delay | 0 ms | 1 ms | 5 ms | 10–1200 ms |
|---|---|---|---|---|
| mutation captured | 3/3 | 2/3 | 0/3 | **0/3** |

`ChromiumoxideDownloader::fetch` navigates, then calls `page.content()` immediately
(`infrastructure/downloader/chromiumoxide_downloader.rs:160-172`). `NAV_TIMEOUT` (30 s)
is a ceiling, not a wait, and there is no network-idle, settle, or wait-for-selector
knob. So anything arriving after the load event — i.e. the fetch/XHR round trip that
defines a SPA — is invisible, and the module's own claim
"**Full** — always renders JS … handles all SPAs" (`domain/js_strategy.rs`) does not
hold. This is also why the mission's step 2 ("mute el DOM *tras cargar*") passes only for
mutations made before load resolves.

Not fixed: a correct fix is an API decision (`--js-settle-ms`, network-idle, or
wait-for-selector), not a patch. Recommend a focused issue.

### F-52-c — preflight validates a binary that is never launched — **OPEN**

webfang probes `google-chrome` first; chromiumoxide 0.7's `detection::get_by_name`
searches `chrome`, `chrome-browser`, `google-chrome-stable`, `chromium`,
`chromium-browser`, `msedge*` — **`google-chrome` is not in that list**.

Consequences, both reproduced:

1. *Different binary.* With a logging wrapper named `google-chrome-stable` earlier on
   `PATH`, the wrapper is what executes. So here preflight certifies CfT 152.0.7977.42
   while the crawl runs `/usr/bin/google-chrome-stable` 152.0.7977.82 — a version check
   against the wrong artifact.
2. *Green gate, dead crawl.* `PATH` containing **only** `google-chrome`: preflight
   passes (no 78), every page then fails mid-crawl with
   `Chrome launch failed: Permission denied (os error 13)`, exit 3, zero files.
   Detection falls through `get_by_name` to `get_by_path`, whose
   `Path::new("/opt/google/chrome").exists()` is true for that **directory** on this
   box, so `Command::spawn` hits EACCES. This is the exact mid-crawl failure #758 exists
   to prevent. Setting `CHROME=<absolute binary>` fixes it, confirming the order.

Not fixed — needs a design choice: either mirror chromiumoxide's candidate list in
`DEFAULT_CHROME_CANDIDATES`, or stop relying on auto-detection and resolve the binary in
webfang, passing it via `BrowserConfig::builder().chrome_executable(..)`. The latter also
removes the `/opt/google/chrome` directory trap. Recommend an issue; a CI harness that
installs only `google-chrome` would hit it.

Side observation, outside F-52: a `404` from the fixture server was crawled and exported
as a success page ("Nothing matches the given URI"), i.e. HTTP status does not gate
export on this path. Not investigated.

## Verdict

**F-52 is CLOSED as an environment blocker and CLOSED for the code path that exists.**
Chrome is installed, the exit-78 gate passes, and a real HeadlessChrome 152 renders DOM
mutations that the served bytes do not contain — on both the scrape and (after F-52-a)
the crawl path.

It is **not** closed as a capability. `full` renders only JS that has already run by the
load event (F-52-b), so it does not do the one job the flag advertises, and the gate can
certify a browser the crawler never launches (F-52-c). F-52-b and F-52-c should be filed
as defects with the reproductions above; neither is environmental.

# Feature: #1808 — Windows console lifecycle plumbing (XP-S-02, XP-K-03, XP-P-07 doc)

**Issue:** #1808 — `fix(platform): Windows console lifecycle plumbing — XP-S-02, XP-K-03 and the \\?\ long-path decision`
**Split from:** #1608 (closed, scope verified complete)
**Labels:** `type:bug`, `technical-debt`, `status:approved`
**Branch:** `fix/windows-console-lifecycle`
**Worktree:** `~/Projects/Rust/webfang-worktrees/fix-windows-console-lifecycle`
**Base:** `origin/main` @ `99748f5f`

## Decisions taken by the maintainer (2026-09-30)

| Decision | Choice | Consequence |
| :--- | :--- | :--- |
| Reaching `SetConsoleCtrlHandler` / `SetConsoleOutputCP` | **`windows-sys` as a direct `[target.'cfg(windows)'.dependencies]` of `webfang_core`** | Already in `Cargo.lock` transitively (0.60.2) → no new crate enters the graph. Hand-rolled `extern "system"` rejected. |
| How the Windows-only code gets compiled | **the CI `test-crossplatform` lane (`windows-latest`) only** | `rustup target add x86_64-pc-windows-msvc` was **declined**. Local `cargo check`/`clippy`/`nextest` therefore do **not** compile the `#[cfg(windows)]` arms. First type-check of the Win32 code happens in CI. |
| XP-P-07 remainder (`\\?\` extended-length paths) | **document as a known limitation** + record the `LongPathsEnabled` registry workaround | Satisfies acceptance criterion "implemented **or** documented". The 255-byte per-component cap already landed in #1627. |

## Premise (verified against main @ 99748f5f)

Three independent shutdown authorities exist, each with its own Windows arm. All of them
ended at `tokio::signal::ctrl_c()`, which on Windows covers **CTRL_C_EVENT and
CTRL_BREAK_EVENT only** — so `CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT` and
`CTRL_SHUTDOWN_EVENT` are observed by none of them:

| # | site | previous Windows arm |
| :--- | :--- | :--- |
| 1 | `crates/webfang_core/src/cli/shutdown.rs` (`next_termination_signal`) | `tokio::signal::ctrl_c().await.ok()` |
| 2 | `crates/webfang_core/src/application/crawler/engine.rs` (`spawn_signal_handler`) | `tokio::signal::ctrl_c().await.ok()` |
| 3 | `crates/webfang_mcp/src/mcp_server/server.rs` (`shutdown_signal`) | `tokio::signal::ctrl_c().await` |

**Premise correction (verified while implementing):** the plan originally listed a fourth
site, `engine.rs:200`. That line is inside `first_termination_signal`, which is
`#[cfg(unix)]` — the engine has exactly ONE Windows arm, the one in `spawn_signal_handler`.
Three sites, not four; the MCP server is the third.

Design consequence: **one** process-wide `SetConsoleCtrlHandler` registration (guarded by
`OnceLock`, so it is installed once no matter how many sites await it), fanning out through a
`tokio::sync::broadcast` channel that carries the **event name** so each site keeps its
existing "Received {name}" log line. Three independent handler registrations would each
install a console handler and each would have to duplicate the same fallback behaviour.

**API note found in windows-sys 0.60.2 while implementing (not guessable):** `CP_UTF8` lives
in `Win32::Globalization`, NOT in `Win32::System::Console`, so the dependency needs the
`Win32_Globalization` feature alongside `Win32_Foundation` and `Win32_System_Console`.
`SetConsoleCP`, `SetConsoleOutputCP` and `PHANDLER_ROUTINE` are all in
`Win32::System::Console`.

Shutdown authority stays exactly where ADR-0016 put it: one `CancellationToken` per run, one
decision. The console handler only *fires* the existing authority; it never owns a second one.

| Task | Status |
| :--- | :--- |
| 1. `windows-sys` dep + `infrastructure::platform` console module | done |
| 2. Wire the console source into all shutdown sites | done |
| 3. XP-K-03 — UTF-8 console codepage at CLI startup | done |
| 4. Docs — ADR-0016, `persistence-resume.md`, `troubleshooting.md` (XP-P-07 `\\?\` recorded as a known limitation with the `LongPathsEnabled` workaround) | done |
| 5. Verify + PR | in progress |

## Non-goals

- Not re-litigating #1608's landed validation work.
- Not XP-F-06 (codesign/notarization) — needs Apple credentials, out of scope.
- Not a rewrite of the four shutdown sites: each keeps its existing shape.

## Commits

| sha | subject |
| :--- | :--- |
| _pending_ | |

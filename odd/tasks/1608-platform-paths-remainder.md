# Feature: #1608 remainder — platform path/filename findings still open on main

**Issue:** #1608 — `fix(platform): path and filename validation is POSIX-only and wrong on NTFS/APFS`
**Labels:** `type:bug`, `status:approved`, `technical-debt`
**Branch:** `fix/platform-paths`
**Worktree:** `~/Projects/Rust/webfang-worktrees/fix-platform-paths`

## Premise correction (verified against main @ 7c383739)

The mission text treats **XP-C-01 (CI matrix) as slice 1 to do first**. It is already
landed and needs nothing from this branch:

| already merged | what it closed |
| --- | --- |
| `e6f3028f` (#1623) | XP-C-01 — `test-crossplatform` matrix job, `windows-latest` + `macos-latest`, `ci.yml:520-543` |
| `70003509` (#1627) | XP-P-04/05/06/07-partial + G-12/G-14 in `mcp_server/validation.rs` |
| `dc6c7d95` (#1628) | XP-F-01 (Windows `write_0600_atomic` arm), XP-S-04 (PATHEXT + per-OS Chrome candidates), XP-S-05 (kill-on-drop), XP-S-06, XP-F-02/03/04, XP-K-02/04, XP-C-04 |
| `4d4c39be` (#1629) | XP-P-01 + XP-P-02 (`path_gate.rs`, platform-aware `classify`, case-insensitive containment) |
| `17145e1b` (#1630) | XP-F-05 shared `domain::paths` helper + the last two cache-path call sites |
| `4646fa8d` | `WEBFANG_CONFIG` + duplicate resolver removal |
| `b85e4695` (#1636) | XP-C-03-adjacent signal-lifecycle gating in the crossplatform lane |

## What is genuinely still open, and what this branch does about it

| finding | state after this branch |
| --- | --- |
| XP-P-05 `:` → NTFS ADS in the **crawler/export download path** | **FIXED here** — `sanitize_filename_component` never had the ADS rule; only the MCP validator got it |
| XP-P-06 trailing dot/space in the download path | **FIXED here** — same gap |
| Windows-invalid set `< > " \| ? *` in the download path | **FIXED here** — same gap, not separately named in the issue |
| XP-C-03 ungated `#!/bin/sh` preflight fixtures | **FIXED here** |
| XP-S-02 Windows CTRL_CLOSE/LOGOFF/SHUTDOWN | **BLOCKED** — needs a Win32 dependency (see below) |
| XP-K-03 Windows console codepage | **BLOCKED** — same reason |
| XP-F-06 codesign / notarization | **BLOCKED** — needs Apple credentials + a maintainer secret |
| G-8 relative `output_dir` exempt from the root gate | deliberately untouched — it is the documented #696 contract, and #1588's design is out of bounds here |
| G-9 `process_export_pipeline` bypasses the gate | deliberately untouched — already surfaced by the #769 startup warn; closing it is a design change |
| `autotuning.rs` `~/.webfang/crawl.db` | deliberately untouched — `.webfang` is a *legacy layout*, not a divergence from `domain::paths`; routing it through the helper relocates a user's database |

## Why the three BLOCKED rows are blocked and not "attempted anyway"

All three need something this worker is forbidden to add:

- **XP-S-02** needs `SetConsoleCtrlHandler`; tokio's `ctrl_c()` covers CTRL_C/CTRL_BREAK only.
- **XP-K-03** needs `SetConsoleOutputCP`.
- Neither has a pure-`std` route. The only dependency-free option is a hand-rolled
  `extern "system"` kernel32 binding — and this environment has **only
  `x86_64-unknown-linux-gnu` installed**, so that FFI could not be type-checked, let
  alone run. Shipping an uncompilable, unrunnable shutdown path is worse than leaving
  the finding open. The standard route is `windows-sys` as a direct dependency, which
  is already in `Cargo.lock` transitively (0.52 / 0.60 / 0.62) — a one-line
  `Cargo.toml` addition that needs maintainer approval.
- **XP-F-06** needs `xcrun notarytool` plus an Apple ID / app-specific password /
  team id. No such secret exists here and creating one is out of bounds.

## Tasks

1. Harden `sanitize_filename_component` (ADS, Windows-invalid set, trailing dot/space).
2. `cfg(unix)`-gate the shell-script preflight fixtures.
3. Verify: `cargo check`, strict clippy, `fmt --check`, `cargo doc`, focused nextest.

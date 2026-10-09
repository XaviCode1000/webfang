# Feature: build-perf-experiments

## Objective
Reduce local build time, memory and disk consumption for webfang, measuring each candidate change separately and landing only what shows a reproducible improvement.

## Problem
Issue #1952. Local builds are the bottleneck for parallel agent worktrees: each new worktree pays a full cold build, the machine is under memory pressure (zram 5.5/8 GiB, no global build lock), and ~/.cache/cargo-target holds 21 target dirs. Prior work already settled sccache (#1447) and dev debuginfo (#944).

## Scope
In: measurement and configuration of toolchain, linker, dev profile, compiler caches (ccache/kache/mbx), split-debuginfo, and a build lock.
Out: `panic = "abort"` (#1219 correctness), `wreq` vs reqwest, release `lto`/`codegen-units` (already justified), `debug = "line-tables-only"` (#944).

## Constraints
- Worktree `chore-build-perf-experiments`, branch `chore/build-perf-experiments`, target dir `~/.cache/cargo-target/chore-build-perf-experiments`.
- mold is local-only and never wired into a config CI reads (mise.toml documents the reason).
- No sccache re-enable (#1447: net negative, hard failure with CARGO_INCREMENTAL).
- All A/B runs are cold builds in virgin target dirs; same machine, same source revision.

## Tasks
- [ ] T1 — Baseline: cold `cargo build --workspace --timings` and cold `--all-targets`; record wall, peak RSS, unit breakdown.
- [ ] T2 — ccache `max_size` 10G -> 50G.
- [ ] T3 — `CFLAGS=-O0` for C/C++ build scripts.
- [ ] T4 — toolchain 1.88.0 -> 1.99.0 (rust-lld default on Linux since 1.90).
- [ ] T5 — `[profile.dev.package."*"]` opt-level 3 -> 0, and add `[profile.dev.build-override] opt-level = 3`.
- [ ] T6 — mold as local-only rustc linker.
- [ ] T7 — `split-debuginfo`.
- [ ] T8 — kache as RUSTC_WRAPPER.
- [ ] T9 — mr-boxington (mbx) via mise.
- [ ] T10 — global build lock (flock) for build/test commands.

## Measured results
| # | Change | Cold wall | Peak RSS | Verdict |
|---|---|---|---|---|
| - | Baseline (worktree dir) | 194.3 s | 2.36 GiB | - |
| T2 | ccache max_size 50G (treeB) | 193.2 s | 2.38 GiB | no effect on cold wall |
| T3 | CFLAGS=-O0 (treeC) | 190.0 s | - | -4.4 s only |
| - | ccache behaviour, same dir | 36.7 s | - | 604/605 direct hits |

### Findings so far
1. The two C build scripts (libgit2-sys 113.1 s, btls-sys 70.8 s) are NOT the critical path: they run concurrently with 1958 s of rustc work. With CFALGS=-O0 libgit2-sys dropped to 68.8 s but total wall moved only 4.4 s.
2. ccache works per target dir: 604/605 direct hits when rebuilding the same dir, but zero useful hits in a virgin dir, because the compile command embeds the target-dir path in `-I .../debug/build/libgit2-sys-<hash>/out/include`. Every new worktree therefore recompiles all C from scratch.
3. ccache contributes 675 cacheable C calls per full build (96 hits on a virgin dir).
4. Cold build parallelism: 2152 s CPU / 194 s wall = 11x on 16 cores.

## Acceptance
Per issue #1952.

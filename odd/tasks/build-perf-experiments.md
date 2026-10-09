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
- [x] T5 — `[profile.dev.package."*"]` opt-level 3 -> 0, and add `[profile.dev.build-override] opt-level = 3`.
- [x] T6 — mold as local-only rustc linker.
- [ ] T7 — `split-debuginfo`.
- [ ] T8 — kache as RUSTC_WRAPPER.
- [x] T9 — mr-boxington (mbx) via mise.
- [ ] T10 — global build lock (flock) for build/test commands.

## Measured results
Cold `cargo build --workspace`, virgin target dir each time, same revision. Load is recorded because this machine is shared with other agent worktrees and contention dominates.

| # | Change | Cold wall | Load | Verdict |
|---|---|---|---|---|
| - | Baseline, `[profile.dev.package."*"] opt-level = 3` | 194.3 s | ~1 | - |
| - | Same baseline, repeated | 251.1 s | 16.6 | +29% from contention alone |
| T2 | ccache `max_size` 10G -> 50G | 193.2 s | ~4 | no effect on cold wall |
| T3 | `CFLAGS=-O0` / `CXXFLAGS=-O0` | 190.0 s | ~8 | -2%, not the critical path |
| T4 | toolchain 1.88.0 -> 1.99.0 | 218.0 s | 20.5 | inconclusive, rerun needed at low load |
| T5 | `[profile.dev.package."*"] opt-level = 0`, paired with baseline | **99.2 s** | 16.3 | **ACCEPTED** (251.1 -> 99.2, -60%) |
| T6 | mold 3.0.0 linker, pair order 1 (mold won at the worse load) | 89.9 s vs 106.2 s | 8.5-13.4 | **-15.3%** |
| T6 | mold, pair order 2 (reversed) | 91.9 s vs 98.9 s | 11.6-12.2 | **-7.1%** |
| T9 | mbx 1.22.0, second virgin target dir (fake new worktree) | **70.7 s** | 29.7 | **722 cache hits — first tool that hits across dirs** |

T6 applied as `RUSTFLAGS="-C link-arg=-fuse-ld=mold"` with mold's `bin` on PATH, per-invocation only. mold 3.0.0 came from the GitHub release tarball, not `cargo install` (the crates.io crate named `mold` is an unrelated DI library). `-fuse-ld=<absolute path>` is rejected by gcc: the linker must be found by name, so `ld.mold` has to be on PATH.

Unit CPU total: 2152 s at opt-level 3 -> 712 s at opt-level 0 (-67%). Peak RSS ~2.4 GiB in both.
Commit `04dc5ce7` lands T5.

### Findings so far
1. T5 is the dominant lever by a wide margin. Everything else measured is within contention noise or smaller.
2. Contention is a first-order cost: the identical build went 194.3 s -> 251.1 s (+29%) purely because another agent was building on the same machine. That is the case for T10 (build lock).
3. The C build scripts are NOT the critical path, so fixing C build time does not help. At opt-level 3 they are libgit2-sys 113.1 s + btls-sys 70.8 s; at `-O0` libgit2-sys drops to 68.8 s, yet total wall moves only 4.4 s. They run concurrently with 1958 s of rustc work on 16 cores.
4. ccache is effective per target dir (604 of 605 direct hits) and useless across dirs. Root cause proven from `CCACHE_LOGFILE`: the compile command embeds the absolute target dir in an include flag, e.g. `cc -O3 ... -I <target-dir>/debug/build/libgit2-sys-<hash>/out/include ...`, so the cache key changes with the tree. Every new worktree recompiles all 675 C files.
5. After T5 the new critical path is `chromiumoxide_cdp` (47.4 s) plus the two C build scripts (btls-sys 41.1 s, libgit2-sys 28.6 s) and 4 binary links (~40 s of unit time). The link share is what T6 (mold) has to attack, and it only becomes dominant for `--all-targets`, which links 89 test binaries of ~270 MB each.
6. A profile change invalidates every unit hash, so a worktree that lands T5 pays one full rebuild.
7. **T6 (mold) is real but modest for `--workspace`**: -7% to -15%, two of two orderings. It only links 4 binaries here; the win should be far larger for `--all-targets`, which links 89 test binaries of ~270 MB and is what `cargo nextest` needs. That measurement is still open.
8. **T9 (mbx) is the first tool on this machine that produces cross-target-dir cache hits.** First virgin build 199.0 s / 0 hits / 2.8 GiB stored; second virgin build in a *different* dir 70.7 s with **722 hits and 209 misses**, at load 29.7. For comparison, ccache is proven ineffective across dirs (finding 4) and sccache was measured net-negative in #1447. `mbx doctor`: 0 failures, 1 warning — plain `cargo` bypasses it until `mbx setup` installs the shim, so the win requires a machine-global decision, not a repo change. Its defaults also attack the disk problem directly: 45 GiB budget, automatic GC, managed targets under `~/.cache/mbx/targets`. mise.toml was reverted: enabling it for plain cargo is a separate call.
9. `mise use --tool-option mr_boxington=true rust mr-boxington` rewrites `rust = "1.88"` into `rust = { version = "latest", ... }`, which silently breaks the MSRV pin in the same file. Restore the pin immediately after.

## Verification
`cargo check --workspace` green with the new profile (69.9 s at load 6.2). The full pre-push gate (`scripts/ci_fast_gate.sh`) has not been run yet.

## Acceptance
Per issue #1952.

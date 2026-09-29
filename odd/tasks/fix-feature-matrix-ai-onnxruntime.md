# fix-feature-matrix-ai-onnxruntime

Unblock GitHub issue #1639 — the `ai` combo of the compatibility harness cannot
find the native `onnxruntime` static library, turning `Feature matrix` red on
`main` and blocking every Release PR (including #1554).

## Objective

Make `bash scripts/check_compatibility.sh --ci-required` pass 6/6 combos in the
`feature-matrix` job, deterministically, and make a recurrence diagnosable from
the job log alone.

## Problem

`Feature matrix` fails twice per run (`[compile] ai` and `[start] ai`) with:

```
Compiling ort-sys v2.0.0-rc.12
error: could not find native static library `onnxruntime`, perhaps an -L flag is missing?
error: could not compile `ort-sys` (lib) due to 1 previous error
FAIL ai start
```

## Why — established root cause (cache-vs-provisioning interaction)

This is **not** missing provisioning. It is a split artifact:

1. `ci.yml:907` runs `cargo hack check --each-feature --workspace --no-dev-deps`
   before the harness. In `check` mode the `ort-sys` build script **does** run and
   records its fingerprint + `output` under `target/debug/build/ort-sys-*/`, but
   `check` produces only an **rmeta** for `ort-sys` — never the **rlib**.
2. A main-branch run of that era went green, and `Swatinem/rust-cache`
   (`cache-targets: true`, `save-if: github.ref == 'refs/heads/main'`) saved a
   `target/` holding the build-script fingerprint **without** the `ort_sys` rlib.
3. The fail-closed harness (f6a0db21, #1634) now runs
   `cargo build -p webfang_cli --features ai`. The rlib is missing, so cargo
   prints `Compiling ort-sys (lib)` — but the **build-script unit is `Fresh`**,
   so it never re-runs and never downloads anything.
4. The ONNX Runtime static library is written to `$ORT_CACHE_DIR`, else
   `${XDG_CACHE_HOME:-$HOME/.cache}/ort.pyke.io`
   (`ort-sys-2.0.0-rc.12` `build/main.rs:97-101`, `src/internal/dirs.rs:196-198`) —
   **outside `target/` and outside every path rust-cache saves**. On a fresh runner
   that directory is empty while the replayed `-L` still names it. Link dies in
   ~0.95 s. The state is self-perpetuating.

Confirmed by reproduction (5 builds, throwaway crate): a *consistent* warm restore
does **not** fail, because rustc bundles the static lib **into** the 92 MB
`libort_sys-*.rlib`. Failure needs the **rlib rebuilt while the fingerprint is
fresh** — an inconsistent restore. That is why `Tests (all features)` survives
(zero `ort-sys` lines in its log, rlib fresh, green) and `feature-matrix` does not.

## Scope

In scope:

- `feature-matrix` job only: co-locate the ORT cache dir inside `target/`, and
  invalidate the stale `ort-sys` build state before the harness.
- A visible preflight diagnostic in the harness so the next recurrence is
  diagnosable from the job log alone (issue AC #3).
- Record the invariant in a comment so a future edit does not silently undo it.

Explicitly out of scope (and why):

- `test-ai` — must stay green with its no-rust-cache posture (#1171). Untouched.
- `clippy` / `code-quality` / `doc-quality` — they run `clippy`/`doc`, which never
  link the native lib, so they cannot produce this error. Untouched to keep the
  diff reviewable.
- `test-full` / `mcp` — they link and are currently green, but their caches are
  self-consistent (they contain the rlib). Adding `cargo clean -p ort-sys` there
  would force a ~90 MB re-download on every run for no present benefit. The
  co-location env is what makes them structurally safe; recorded here so the
  decision is visible, not accidental.
- `CHANGAGELOG.md` — repo policy: release-plz owns it (AGENTS.md).

## Constraints

- `ORT_CACHE_DIR` must be **absolute**. `ort-sys` records the value verbatim and
  resolves it against the crate root at build time, so a relative path is fragile.
- `ORT_CACHE_DIR` is **not** a declared rerun input of the `ort-sys` build script
  (its 7 declared inputs are `ORT_LIB_PATH`, `ORT_LIB_LOCATION`,
  `CARGO_NET_OFFLINE`, `ORT_SKIP_DOWNLOAD`, `ORT_OFFLINE`, `ORT_CXX_STDLIB`,
  `CXXSTDLIB`). Changing it against a warm target silently replays the old `-L`.
  Therefore it must be **byte-identical** in the cache-producing job and every
  consumer of that same cache.
- `cargo clean -p ort-sys` removes 20 files / 101.5 MiB and does **not** work
  offline. It must run while the co-located lib is still present, so no
  re-download is triggered.
- No `git stash`, no `git checkout`/`switch` (shared stash across worktrees).
- Errors/logs in English; the repo policy for user-facing CLI text is unaffected.

## Tasks

- [x] T1 Establish the root cause and reproduce it locally — **done** before this
      document; see "Why" and the verification evidence below.
- [x] T2 Create the feature document (this file) before the first source write.
- [x] T3 Co-locate `ORT_CACHE_DIR` inside `target/` in the `feature-matrix` job
      and add the `cargo clean -p ort-sys` pre-harness step, with an invariant
      comment. — **done in #1642** (`d38970b5`), but the `cargo clean -p ort-sys`
      half of it was **insufficient on its own**; see the correction below.
- [x] T4 Add a harness preflight that reports the missing native library
      explicitly, so a recurrence is readable from the job log. — **done in
      #1642** (`preflight_ort_native_lib`).
- [ ] T5 Verify: `actionlint` + `zizmor` clean, `bash scripts/ci_fast_gate.sh`
      green, and the harness's shell syntax checked.
- [ ] T6 Post the diagnosis into #1639 (AC #1) and open the PR against the
      approved issue.

## Correction (2026-09-29): T3 shipped, and the lane stayed red anyway

#1642 landed both the co-located `ORT_CACHE_DIR` and the
`cargo clean -p ort-sys` step, and the `Feature matrix` lane **kept failing** on
every run afterwards. The step was the wrong remedy, and the preflight shipped
with it advertised that wrong remedy to whoever hit the next recurrence.

**Mechanism.** The download is gated on `!bin_extract_dir.exists()` in
`ort-sys` `build/download/mod.rs:102`. A cache restore that brings the extract
directory back *without* the `libonnxruntime.a` inside it therefore closes the
gate: the build script skips the download, and nothing retries it because the
fingerprint is then `Fresh`. `cargo clean -p ort-sys` drops the build-script
unit but does not touch `$ORT_CACHE_DIR`, so it re-establishes a `Fresh`
fingerprint around the same missing library.

**Verified locally**, from the exact broken state (`.a` absent, extract dir
present):

| Sequence | Result |
| --- | --- |
| `cargo clean -p ort-sys` + `cargo build` | `EXIT=101`, *could not find native static library `onnxruntime`*, nothing downloaded |
| clear the extract dir + `cargo clean -p ort-sys` + `cargo build` | `EXIT=0`, `libonnxruntime.a` downloaded |

The first row is exactly what #1642 shipped, and it is the row that has been
failing.

**The fix** clears the half-restored `dfbin/` extract dir so the download gate
re-opens, in both the CI step and the harness preflight, and adds one real
`cargo build -p webfang_cli --features ai` so the library is present before the
combos run and a genuine download failure is reported under this job's name
instead of surfacing as a link error twice inside the harness.

**What this does NOT establish.** The end-to-end harness run could not complete
locally: `help_check` hardcodes `./target/debug/webfang`, and #1267 forbids
pointing the run at the shared target dir, so `--help` exits 127 on a missing
relative path. The core mechanism is proven directly (table above); the
`6/6` result is CI's to confirm.

## Authorized scope

- Edit: `.github/workflows/ci.yml` (the `feature-matrix` job only).
- Edit: `scripts/check_compatibility.sh`.
- Add: nothing outside the two files above plus this document.

## Acceptance criteria (mirrors #1639)

- [ ] AC1 Cause established and written down with the proving log line / job
      setting. Satisfied by the "Why" section; must be posted to #1639 in T6.
- [ ] AC2 `check_compatibility.sh --ci-required` passes 6/6 in `feature-matrix`,
      with `ai` reaching its runtime probes.
- [ ] AC3 A recurrence is diagnosable from the job log alone (T4).
- [ ] AC4 `Tests (AI integration)` stays green, no-rust-cache posture preserved.
      No change is made to that job.
- [ ] AC5 `bash scripts/ci_fast_gate.sh` green; no regression in
      `Tests (all features)`. The diff touches no Rust source, so the compile
      lanes are skipped by path classification.

## Applicable checks

```bash
actionlint .github/workflows/ci.yml
zizmor --persona=auditor .github/workflows/ci.yml   # informational; report, do not autofix
bash -n scripts/check_compatibility.sh
bash scripts/ci_fast_gate.sh
```

No `cargo` lane applies: the diff is CI-config and shell only.

## Route declaration

| Task | Route | Trigger evidence |
| :--- | :--- | :--- |
| T1 | delegated direct (explore + general) | 4+ files to understand (`ci.yml`, `check_compatibility.sh`, `ort-sys/build/*`, `ort-sys/src/internal/dirs.rs`) → mapping trigger; the reproduction is a build action → fresh worker |
| T2 | direct inline | One mechanical document, no research left to do |
| T3, T4 | delegated direct (one writer) | 2 non-trivial files with subtle cache-invariance reasoning → writer trigger |
| T5 | delegated direct (fresh worker) | Build/verify action |
| T6 | direct inline | `git`/`gh` state commands |

## Verification evidence

- Reproduction, 5 builds, throwaway crate under a temp dir, real ORT cache and
  repo untouched: deleting the ORT cache dir while keeping the `ort_sys` rlib
  absent reproduces the CI error verbatim
  (``could not find native static library `onnxruntime` ``); keeping both
  succeeds.
- Co-locating `ORT_CACHE_DIR` inside the target dir: a re-run with both present
  succeeds (`Fresh ort-sys`, links).
- Recorded on this machine: `target/debug/build/ort-sys-*/output` contains
  `link-search=native=/home/xavi/.cache/ort.pyke.io/dfbin/...` while the `.a`
  lives at that path — the split, observed directly.

## Progress

- T1, T2 done. T3 next.

## Next step

T3 + T4 via one writer, then T5.

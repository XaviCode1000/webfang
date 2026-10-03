# #1697: the un-gated sibling — `batch_empty_stdin_exits_64`

## Goal
Close #1697 for real. The fix it shipped landed on **one** of the two tests
that carry the flake; the other one kept hanging and kept the
`Tests (windows-latest)` lane red on every PR touching `webfang_core`.

## What #1697 actually left behind
The issue reports `batch_empty_file_exits_64`: `TRY 1`/`TRY 2` each ran past
nextest's 180 s `terminate-after` as TMT, `TRY 3` failed — **3 de 3**. The fix
(`SPAWN_LATENCY_BUDGET` + `assert_spawn_within`, `cli_harness.rs:133` / `:153`)
was applied to that test and only that test.

Its sibling in the same module is the same shape — spawn the binary, feed it an
empty batch, assert exit 64 and the Spanish message — and it kept the raw
`cmd().assert()` path. It hangs for the same reason, because **the flake is a
property of SPAWNING on `windows-latest`, not of which input path feeds the
process.**

Reproduced on this repo's own CI while reviewing PR #1793 (run `37126785645`,
job `111213658025`):

```
TRY 1 SLOW >60s → >120s → TRMNTG >180s → TMT 180.207s
RETRY 2/3 → identical
RETRY 3/3 → TRY 3 FAIL 6.248s
1013 tests run: 1012 passed (1 leaky), 1 failed, 16 skipped
```

Counter-evidence that it is not caused by any recent diff: the very same test
**passed in 25m36s** on PR #1794's run `37139628727` with a near-identical tree.
Intermittent, exactly as the issue describes.

The unit lib suite on that same Windows run was clean — `3043 tests, 3043
passed, 4 skipped` — so the failure is a behavioural test in `webfang_core`,
not the MCP change under review.

## Fix
Convert `batch_empty_stdin_exits_64` to `assert_spawn_within(
SPAWN_LATENCY_BUDGET, …)`, mirroring the sibling verbatim in structure: the
assertion content is **unchanged** (exit code 64 AND the `No URLs provided`
message), only its failure *transport* changes from an unbounded
`.assert()` to a budgeted, retried worker with an abandoned overrun.

Net: `43 insertions, 7 deletions`, one test function. No production code.

## Why the audit did not convert ~20 other tests
The suite has **116** `.assert()` spawn sites and only **5** use
`assert_spawn_within`. A grep-based classifier over "tests asserting an exit
code without wiremock" produces ~20 candidates — but that heuristic carries
`net` state between adjacent functions, so it mislabels tests that do use
`wiremock` (e.g. `harness_403_counts.rs`). Converting 20 tests on a classifier
this unreliable would be a large behavioural diff with no reproduced evidence
behind any individual change.

**Scope discipline: fix what was reproduced.** The correct follow-up method is
empirical, not lexical — the `Tests (windows-latest)` lane's own TMT list names
exactly which spawn sites actually overrun, and that list is the input to a
follow-up issue. Recorded there rather than guessed at here.

## Evidence

| Check | Result |
| :--- | :--- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy -p webfang_core --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | clean (`predicates` import not left dangling) |
| `cargo nextest run -E '<both empty-batch tests>'` | `2 run: 2 passed` |
| **Guard still detects a real regression** | injected `!= Some(65)` instead of `Some(64)` → `TRY 1/2/3 FAIL`, first at **0.287 s** |

That last row is the one that matters. A budget can be implemented so that a
timeout is reported as a pass; proving that an *injected wrong exit code* still
fails, and fast, is what distinguishes a guard from a lid.

## Acceptance
- [x] `batch_empty_stdin_exits_64` runs under `assert_spawn_within` with the
      shared budget.
- [x] Assertion content unchanged: exit code 64 **and** the Spanish message.
- [x] The guard can still fail — proven by an injected regression.
- [x] The sibling stays green.
- [ ] `Tests (windows-latest)` green on the PR.

## Also fixed
The issue **title** names `batch_empty_file_exits_64`, which is the half that
was already fixed — which is why the issue reads as open while the test it
describes is green. It now names the sibling that was actually un-gated, so
the tracker state matches reality.

## Out of scope
- Not touching the other ~20 candidate spawn sites (see above).
- Not changing `SPAWN_LATENCY_BUDGET` (20 s, ~6x the measured 3.05 s healthy
  run) — it is load-bearing for the sibling and shared.
- Not touching `refs/stash`, which is shared across worktrees.
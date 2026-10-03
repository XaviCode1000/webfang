# #1734: Miri aborts on `copy_file_range(326)` from `preserve_pre_migration_backup`

## Goal
Stop a documented Miri syscall limit from **aborting whole lanes**. `Miri (core)`
and `Miri (infra-slow)` both died mid-run in the #1423 manual dispatch because
one un-gated test each reached `std::fs::copy`, whose Linux fast path is the
kernel `copy_file_range(2)` call Miri does not implement.

## Root cause
`std::fs::copy` on Linux is specialized to `copy_file_range` (syscall 326) for
regular files. Miri does not implement syscall 326, and a Miri
unsupported-operation **aborts the whole process** — so one test kills its entire
lane instead of failing one case.

The single site is `preserve_pre_migration_backup`
(`crates/webfang_core/src/application/resume.rs:144`, `fs::copy` at `:149`), which
has exactly **2 callers**:

| caller | reached by |
| :--- | :--- |
| `application/crawler/checkpoint.rs:398` (`accept_version`) | `Miri (core)` — filter `-- adapters:: application:: cli:: config:: di:: error::` |
| `infrastructure/export/state_store.rs:194` (`load_or_default`) | `Miri (infra-slow)` — filter includes `infrastructure::export::` |

That maps exactly onto the #1423 run `37038367607`, where those two lanes failed
and `domain` / `infra-fast` were green.

## This is a gap, not a new class
`copy_file_range(326)` is already a **documented, gated** Miri limit in this repo.
`infrastructure/export/record_store.rs` carries six
`#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]` flags
(`:879, :928, :965, :978, :1024, :1075`) on the tests that reach its own
`fs::copy` (`record_store.rs:589`). Two sibling checkpoint tests
(`prop_roundtrip_save_load`, `prop_corruption_tamper_crc32`, `checkpoint.rs:963`
and `:974`) are already `#[cfg_attr(miri, ignore)]`.

`test_old_format_json_loads` is the test in the same module that was **missed**.

## The three un-gated tests (intelligence gate, 2026-10-03)

| # | test | line | lane |
| :--- | :--- | :--- | :--- |
| 1 | `application::crawler::checkpoint::tests::test_old_format_json_loads` | `checkpoint.rs:822` | `Miri (core)` |
| 2 | `infrastructure::export::state_store::tests::test_load_or_default_discards_stale_version_zero` | `state_store.rs:341` | `Miri (infra-slow)` |
| 3 | `infrastructure::export::state_store::tests::test_load_or_default_stale_version_preserves_bak` | `state_store.rs:373` | `Miri (infra-slow)` |

Both `state_store` tests write `version: 0`, so `load_or_default` takes the
`version != CURRENT` branch and calls `preserve_pre_migration_backup`.

Ruled out by reading each remaining test in both modules — none reaches the
backup: `test_load_or_default_existing`, `test_load_or_default_new`,
`test_load_or_default_keeps_current_version_one` (all `CURRENT` version or
`NotFound`), `test_load_or_default_corrupt_propagates_error` and
`test_load_or_default_unknown_version_propagates_error` (propagate before the
branch), `test_load_does_not_discard_stale_version` (calls `load()`, not
`load_or_default`).

## Fix
Apply the **same** durable per-test flag already used by the record-store sibling:

```rust
#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
```

Nothing else changes. Three attribute lines.

## Out of scope — explicitly
- **Do NOT replace the production copy with a byte loop.** The FFI/syscall
  boundary is a documented Miri limit; the flag is the honest gate. A byte loop
  would trade real `copy_file_range` performance in production for a green
  advisory lane.
- **Do NOT re-add a `--skip` to `ci.yml`.** The durable-flag contract (PR #1417,
  landed in #1425) is exactly what these lanes are meant to be proving; a
  `--skip` would restore the blind window #1423 exists to close.
- No production-code change of any kind.

## Tasks
1. [x] Intelligence gate: enumerate every test reaching `std::fs::copy` through
   `preserve_pre_migration_backup` (3 found, 8 ruled out by reading).
2. [x] Confirm the lane mapping against `ci.yml`'s actual filters.
3. [x] Create this doc.
4. [x] Bootstrap worktree `fix/miri-copy-file-range-trap`, isolated
   `CARGO_TARGET_DIR`.
5. [x] Apply the three flags (each pattern matched **exactly once**, verified
   programmatically — the edit script fails closed on 0 or >1 matches).
6. [x] **Verified with a real Miri run** on the pinned toolchain
   (`nightly-2026-08-27`) with `ci.yml`'s exact `MIRIFLAGS`. A `cargo check`
   proves nothing here; the interpreter is the whole point.
7. [x] Pre-commit gate: check, strict clippy, `fmt --check`, rustdoc, nextest.
8. [ ] Native review of the committed range, then work-unit commit and PR.

## Evidence — real Miri A/B, same command both sides

```bash
RUSTUP_TOOLCHAIN=nightly-2026-08-27 \
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-disable-isolation -Zmiri-ignore-leaks \
          -Zmiri-seed=42 -Zmiri-permissive-provenance" \
cargo miri test -p webfang_core --lib -- \
  application::crawler::checkpoint::tests::test_old_format_json_loads \
  infrastructure::export::state_store::tests::test_load_or_default_discards_stale_version_zero \
  infrastructure::export::state_store::tests::test_load_or_default_stale_version_preserves_bak
```

| Variant | Result |
| :--- | :--- |
| **B** — flags removed | `error: unsupported operation: syscall: unsupported syscall number 326` on `test_old_format_json_loads`; `error: aborting due to 1 previous error`; **exit 1** |
| **A** — flags applied | all three `ignored, copy_file_range(326) unsupported by Miri`; `0 passed; 0 failed; 3 ignored; 2255 filtered out`; **exit 0** |

The detail that matters is in variant B: the process **aborted on the first
test**. The two `state_store` tests never ran at all. That is the issue's
central claim — one test kills the whole lane mid-run rather than failing a
case — reproduced locally rather than inferred from a CI log.

## Gates

| Gate | Result |
| :--- | :--- |
| `cargo check -p webfang_core --all-targets --all-features` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | clean |
| `cargo fmt --all -- --check` | exit 0 |
| `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps` | exit 0 |
| `cargo nextest run -p webfang_core -E '<the three>'` | **3 passed; 0 failed** |

That last row is what separates a gate from a deletion: outside Miri the three
tests still run and still pass, so the `cfg_attr` gates the **interpreter**, not
the behaviour.

## Acceptance
- [x] `cargo miri test` reports `ignored` (not a process abort) with the flags
      applied, for all three tests.
- [x] The same command **without** the flags reproduces
      `unsupported syscall number 326` and aborts the process.
- [x] Non-Miri suite unaffected: all three still run and pass under `nextest`.
- [ ] `Miri (core)` and `Miri (infra-slow)` green on CI.
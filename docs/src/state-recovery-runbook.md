# State Recovery Runbook

What to do when webfang's persisted state looks wrong.

This page covers the **SQLite/WAL vault database** (`~/.webfang/crawl.db`) —
the artifact that had no recovery procedure anywhere — and routes the other
persisted artifacts (the crawl checkpoint and the JSON resume state) to their
own contract.

Every behaviour described here is **pinned by a test that already exists** in
the repository, and the test is named next to the claim. A step marked
**not covered by a test** is either SQLite's own behaviour or an operator
instruction: it is never a promise webfang makes.

> The durability **contract** — what is guaranteed and what is explicitly not
> — lives in `docs/persistence-resume.md`. This page is the **procedure**:
> what to check, what to run, what to delete.
>
> Where the files live and how to remove them: [installation.md](installation.md).
> Symptoms that are not state at all: [troubleshooting.md](troubleshooting.md).

---

## Triage — which artifact is broken?

| Symptom | Artifact | Section |
| :--- | :--- | :--- |
| A run fails with `base de datos con versión de esquema …` | vault DB | [The vault database](#the-vault-database) |
| A run fails with `vector BLOB corrupto: longitud N no es múltiplo de 4` | vault DB | [A corrupt embedding](#a-corrupt-embedding-blob) |
| A run with `--elastic` fails before it crawls | vault DB | [The schema-version gate](#the-schema-version-gate-refuse-never-migrate) |
| `--resume` re-scrapes pages it had already done | JSON state | [The JSON state](#the-json-state) |
| A `<domain>.json.bak` appeared next to the state file | JSON state | [The JSON state](#the-json-state) |
| A `<domain>.json.lock` is still there after the run | — | Nothing is broken: the sentinel is permanent by design — see [troubleshooting.md](troubleshooting.md#leftover-lock-files-in-the-state-directory--is-my-state-corrupted) |

---

## The vault database

### Where the database lives

`~/.webfang/crawl.db` by default — **absolute, under `$HOME`, not under the
webfang cache base**. `--db-path` (or `WEBFANG_DB_PATH`) moves it.

| Behaviour | Pinned by |
| :--- | :--- |
| The default path is `<home>/.webfang/crawl.db` | `test_default_db_path_under_home` — `crates/webfang_core/src/infrastructure/autotuning.rs` |
| The CLI flag wins over the env var, which wins over the default | `test_resolve_db_path_priority` — `crates/webfang_core/src/infrastructure/autotuning.rs` |
| The file is created **lazily**: the pool is built first, the file appears when the schema is initialised — that is the fail-fast point if the path is not writable | `test_db_file_created_when_absent` — `crates/webfang_core/src/infrastructure/persistence/sqlite.rs` |
| The database only exists in builds compiled with the `persistence` feature. The release binary is built with `--features "mcp"` only, so a release install never creates it | *not covered by a test* — the feature gate is `crates/webfang_core/Cargo.toml` (`persistence = ["dep:rusqlite", "dep:deadpool-sqlite", "dep:sys-info"]`) and the release command is `.github/workflows/release.yml`; the same fact is stated for operators in [installation.md](installation.md) |

### How it is opened

| Behaviour | Pinned by |
| :--- | :--- |
| Every pooled connection runs `journal_mode=WAL`, `synchronous=NORMAL`, `cache_size=-4000` (the first is persistent in the DB header, the other two are per-connection and are re-applied by a `post_create` hook) | `test_wal_pragmas_applied_to_connection` — `crates/webfang_core/src/infrastructure/persistence/sqlite.rs` |
| The schema DDL is idempotent (`CREATE … IF NOT EXISTS`), so re-initialising an existing database is a no-op | `test_setup_schema_is_idempotent` — same file |

### The schema-version gate: refuse, never migrate

The database carries its schema version in SQLite's own on-disk header slot
(`PRAGMA user_version`). webfang writes `1` and opens only `1`.

| Behaviour | Pinned by |
| :--- | :--- |
| A fresh database is stamped with the current version, and the stamp survives a reopen; a second initialisation neither fails nor re-stamps | `setup_schema_stamps_the_schema_version_marker` — `crates/webfang_core/src/infrastructure/persistence/sqlite.rs` |
| A database stamped with **another** version is **refused before any DDL runs** — the refused file is never opened for writing, and it keeps its own marker | `setup_schema_refuses_a_foreign_schema_version_without_writing` — same file |
| A database stamped `0` (written before the marker existed, or brand new) is **adopted**, not refused | `setup_schema_adopts_an_unversioned_database` — same file |
| The refusal message names **both** versions, in Spanish: `base de datos con versión de esquema {found}, incompatible con la versión {SCHEMA_VERSION} de webfang; actualizá webfang o usá una base de datos distinta (nunca se modifica una base desconocida)` | `setup_schema_refuses_a_foreign_schema_version_without_writing` — same file (it asserts both numbers appear in the message) |

There is **no migration mechanism**: a bump of the marker without a migration
would turn every existing database into a refusal, so the gate refuses by
design. The behaviour that pins this is the refusal itself
(`setup_schema_refuses_a_foreign_schema_version_without_writing`): a foreign
version is refused, never converted.

### Recovery procedure

1. **Stop every webfang process.** The database is only safe to touch when no
   run holds it open.
2. **Keep the evidence before you touch anything:**

   ```bash
   cp ~/.webfang/crawl.db ~/.webfang/crawl.db.broken.$(date +%s)
   ```

3. **Read the marker** (SQLite's own tool — *not covered by a test*):

   ```bash
   sqlite3 ~/.webfang/crawl.db 'PRAGMA user_version;'
   ```

   `1` (today's `SCHEMA_VERSION`) is a database this build owns. `0` is an
   unversioned one, which webfang adopts. **Anything else is a database
   webfang refuses** — it was written by a build with a different column
   layout.

4. **Check the file's integrity** (SQLite's own tool — *not covered by a
   test*; webfang never runs it):

   ```bash
   sqlite3 ~/.webfang/crawl.db 'PRAGMA integrity_check;'
   ```

5. **Choose the recovery:**

   | Situation | What to do |
   | :--- | :--- |
   | The marker is foreign (`webfang` refuses the file) | Either update webfang so the version matches, or point `--db-path` / `WEBFANG_DB_PATH` at a different file. This is what the refusal message itself asks for. Note that `--db-path` only has an effect together with `--elastic` — on its own it is rejected with exit 78 (*not covered by a test*; the guard is `crates/webfang_core/src/cli/preflight.rs::check_db_path_requires_elastic`). |
   | The marker is foreign and the ingested vectors are not needed | Delete `crawl.db`, `crawl.db-wal` and `crawl.db-shm` **together**, with no run active, and let the next run create a fresh database. The DDL is idempotent, so the new file is created and stamped on the spot. |
   | The marker is `1` and the file passes `integrity_check` | Nothing to recover — the refusal came from somewhere else. Re-read the error text; the two webfang-raised failures are the two rows below. |
   | The file is corrupt and the run fails on a vector read | See [A corrupt embedding](#a-corrupt-embedding-blob). |

> **What is lost when the database is deleted.** The database holds the
> ingestion tables — `resources`, `chunks`, `notes`, `note_chunks` and
> `extraction_fingerprints`. Deleting it discards the ingested vectors, the
> stored chunks and the failure fingerprints. It does **not** touch the
> Markdown/JSONL the run wrote to the output directory: those are separate
> files, written by the exporters.
>
> **Not covered by a test:** the sidecars `-wal` and `-shm` are SQLite's own
> artifacts. webfang never creates, truncates or deletes them by name, and it
> never runs `PRAGMA wal_checkpoint`. Deleting the three files together with
> no process attached is SQLite's own recovery practice, not a webfang
> guarantee.

### A corrupt embedding blob

| Behaviour | Pinned by |
| :--- | :--- |
| A stored embedding whose length is not a multiple of 4 is refused with a Spanish error — `vector BLOB corrupto: longitud N no es múltiplo de 4` — instead of being decoded into garbage | `test_bytes_to_f32_vec_rejects_invalid_blob_length` — `crates/webfang_core/src/infrastructure/persistence/sqlite.rs` |
| The same failure surfaces through the public read path (`get_vector`) as an error, never as a panic | `test_corrupt_blob_returns_error` — same file |

A corrupt blob surfaces as an error on the read of that row. Whether the
rest of the database keeps working is SQLite's own behaviour — *not covered
by a test*. The recovery is the same as above: the file itself is not
damaged.

### Retention

| Behaviour | Pinned by |
| :--- | :--- |
| `--retention-days N` prunes rows older than the cutoff from all four tables, children before parents, and runs `VACUUM` afterwards | `vault_db_rows_older_than_cutoff_are_pruned_children_first` — `crates/webfang_core/src/cli/retention.rs` (the test asserts the row counts; the `VACUUM` is in the code path, not asserted) |
| Rows inside the cutoff survive the prune | same test (it asserts the fresh rows are still there) |

Without `--retention-days` nothing is ever pruned and the run prints no
retention summary — `default_retention_is_disabled_and_never_prunes` —
`crates/webfang_core/tests/retention_test.rs`. Rows whose
`created_at` is `NULL` or unparseable are never pruned — *the SQL condition
is in `prune_older_than`
(`crates/webfang_core/src/infrastructure/persistence/sqlite.rs`); the test seeds
only dated rows, so the `NULL` case is **not covered by a test***.

---

## The JSON state

The crawl checkpoint and the JSON resume state are covered by the durability
contract in `docs/persistence-resume.md`. This section only lists what the
recovery procedure needs.

| Behaviour | Pinned by |
| :--- | :--- |
| The record store writes a temp file and renames it over the live file; **`rename(2)` is the commit point** — a crash before it leaves the previous complete state, never a torn file | `crash_between_saves_leaves_exported_state_reloadable_and_completable` and `crash_after_committed_rename_sees_committed_on_reload` — `crates/webfang_core/src/infrastructure/export/record_store.rs` |
| A crash at any of the pinned crash points, followed by a `--resume` rerun, recovers with **zero lost URLs and no duplicated committed line** | every row of `crates/webfang_core/tests/crash_matrix_test.rs` (for example `crash_mid_state_file_write_truncated_tmp_is_gced`, `crash_tmp_written_pre_rename_discards_tmp_and_recovers`, `crash_while_holding_lock_releases_and_recovers`) |
| A stale `.tmp-*` sibling left by a crash is garbage-collected on the next load and is never read as data | `stale_tmp_files_are_garbage_collected_on_load_and_never_read` — `crates/webfang_core/src/infrastructure/export/record_store.rs` |
| A discarded state file leaves a `<file>.bak` sibling carrying the exact pre-migration bytes; an existing backup is never overwritten, so the *first* bytes win | `test_load_or_default_stale_version_preserves_bak` — `crates/webfang_core/src/infrastructure/export/state_store.rs`; `preserve_abandoned_bytes_writes_rejected_bytes_once` — `crates/webfang_core/src/application/resume.rs` |
| The `.bak` extends the artifact's own extension (`state.json` → `state.json.bak`) | `backup_sibling_extends_the_existing_extension` — `crates/webfang_core/src/application/resume.rs` |
| The crawl checkpoint is `[4-byte CRC32][JSON payload]`; a checksum mismatch refuses the file and leaves the exact rejected bytes in a `.bak` | `checksum_is_first_four_bytes` and `crc32_mismatch_preserves_bak_before_the_rewrite` — `crates/webfang_core/src/application/crawler/checkpoint.rs` |
| A checkpoint whose schema version is superseded is discarded (never resumed half-understood) and its pre-migration file is preserved | `stale_checkpoint_version_is_discarded_not_resumed` — `crates/webfang_core/tests/checkpoint_store_integration.rs` |
| The `<state>.lock` sentinel is permanent: the lock is released when the process exits, the file is never unlinked | `dropping_the_store_lock_releases_but_never_unlinks_the_sentinel` — `crates/webfang_core/src/infrastructure/export/record_store.rs` |

**Recovery in one line:** if a state file looks wrong, the `.bak` sibling next
to it holds the bytes webfang refused. Copy it back (with no run active) if
you need the old state; otherwise delete it and let the next `--resume` start
from scratch.

---

## What this runbook does not promise

- **Power loss.** Every atomic write in the persistence layer is
  temp-file + `rename(2)` with **no `fsync`**: it is atomic against process
  death, not against power loss. The contract is `docs/persistence-resume.md`.
- **An integrity check on the database.** webfang never runs
  `PRAGMA integrity_check` and never checkpoints the WAL. Both commands in the
  recovery procedure above are SQLite's own tools, invoked by the operator.
- **A migration for a foreign database.** The gate refuses; it does not
  convert.
- **Automatic repair.** Nothing in this repository repairs a damaged file on
  its own. The repairs this page lists are operator actions.

---

## Where the code lives

| Component | Path |
| :--- | :--- |
| SQLite pool, pragmas, schema, version gate | `crates/webfang_core/src/infrastructure/persistence/sqlite.rs` |
| Fingerprint table (same pool) | `crates/webfang_core/src/infrastructure/persistence/fingerprint.rs` |
| Retention prune | `crates/webfang_core/src/cli/retention.rs` |
| DB path resolution | `crates/webfang_core/src/infrastructure/autotuning.rs` |
| JSON state store and record store | `crates/webfang_core/src/infrastructure/export/{state_store,record_store}.rs` |
| Crawl checkpoint | `crates/webfang_core/src/application/crawler/checkpoint.rs` |
| `.bak` policy (one policy for every destructive rewrite) | `crates/webfang_core/src/application/resume.rs` |
| Crash-injection harness | `crates/webfang_core/src/cli/crash_points.rs` |

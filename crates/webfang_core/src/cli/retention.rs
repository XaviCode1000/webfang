//! End-of-run retention pruning (#1827 slice B).
//!
//! `--retention-days <N>` (default `0` = disabled) prunes, after a run that
//! exported:
//!
//! - files under the export output root whose mtime is older than the
//!   cutoff (now-empty directories are removed too; the root itself never
//!   is), and
//! - vault-DB rows (`chunks` → `resources`, `note_chunks` → `notes`) whose
//!   `created_at` is older than the cutoff — `persistence` feature only.
//!   In builds without the feature the DB cannot exist, so the step is a
//!   silent no-op.
//!
//! Retention is best-effort by design: individual failures log a warning
//! and the run continues (same contract as extraction fingerprinting) — a
//! cleanup step must never turn a successful scrape into a failed run.

use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};

use crate::application::crawl_options::CrawlOptions;

/// What one retention pass removed. `files_removed` counts pruned files
/// (directories are not counted); `db_rows_removed` counts deleted rows
/// across all vault-DB tables (`0` without the `persistence` feature).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RetentionOutcome {
    pub files_removed: u64,
    pub db_rows_removed: u64,
}

impl RetentionOutcome {
    fn is_empty(self) -> bool {
        self.files_removed == 0 && self.db_rows_removed == 0
    }
}

/// Apply `--retention-days` at the end of a run. Disabled (`0`) returns
/// immediately and performs no I/O — the default must be zero-cost.
pub(crate) async fn apply_retention(opts: &CrawlOptions) -> RetentionOutcome {
    let days = u64::from(opts.export.retention_days);
    if days == 0 {
        return RetentionOutcome::default();
    }
    let cutoff = Utc::now() - Duration::days(i64::try_from(days).unwrap_or(i64::MAX / 86_400));

    let files_removed = prune_exports(&opts.export.output_dir, cutoff).await;
    let db_rows_removed = prune_vault_db(opts, cutoff).await;
    let outcome = RetentionOutcome {
        files_removed,
        db_rows_removed,
    };

    if outcome.is_empty() {
        tracing::debug!(retention_days = days, "retention: nothing older than the cutoff");
        return outcome;
    }
    tracing::info!(
        files_removed = outcome.files_removed,
        db_rows_removed = outcome.db_rows_removed,
        retention_days = days,
        "retention prune complete"
    );
    if !opts.export.quiet {
        println!(
            "Retention: {} archivo(s) y {} fila(s) de la vault DB purgados (> {} días).",
            outcome.files_removed, outcome.db_rows_removed, days
        );
    }
    outcome
}

/// Prune files under `root` (recursive) whose mtime is older than `cutoff`,
/// then prune directories left empty (the root itself is never removed).
async fn prune_exports(root: &Path, cutoff: DateTime<Utc>) -> u64 {
    if !root.exists() {
        return 0;
    }
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut removed = 0_u64;
        prune_dir(&root, cutoff, &mut removed, true);
        removed
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "retention: export prune join failed; skipping");
        0
    })
}

/// Depth-first walk: children first (so an emptied parent can be pruned),
/// files by mtime, directories by emptiness. `is_root` guards the export
/// root itself — it always survives.
fn prune_dir(dir: &Path, cutoff: DateTime<Utc>, removed: &mut u64, is_root: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::warn!(dir = %dir.display(), "retention: output dir unreadable; skipping subtree");
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            prune_dir(&path, cutoff, removed, false);
        } else if meta.modified().is_ok_and(|m| m < SystemTime::from(cutoff)) {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    *removed += 1;
                    tracing::debug!(file = %path.display(), "retention: pruned export file");
                },
                Err(e) => {
                    tracing::warn!(error = %e, file = %path.display(), "retention: could not prune file");
                },
            }
        }
    }
    if !is_root {
        // Succeeds only when the directory is empty — either it was pruned
        // empty just now or it was already empty and stale. A `NotEmpty`
        // error is the normal case and is deliberately ignored.
        let _ = std::fs::remove_dir(dir);
    }
}

/// Prune vault-DB rows older than the cutoff. The `persistence` feature
/// compiles the SQLite stack; without it no DB can exist and this is a
/// silent `0` (the release binary ships without `persistence`).
#[cfg(feature = "persistence")]
async fn prune_vault_db(opts: &CrawlOptions, cutoff: DateTime<Utc>) -> u64 {
    use crate::infrastructure::autotuning::{env_db_path, resolve_db_path};

    let db_path = resolve_db_path(opts.elastic.db_path.as_deref(), env_db_path());
    if !db_path.exists() {
        return 0;
    }
    let pool = match crate::infrastructure::persistence::create_pool(&db_path, 1) {
        Ok(pool) => pool,
        Err(e) => {
            tracing::warn!(error = %e, db = %db_path.display(), "retention: vault DB pool unavailable; skipping DB prune");
            return 0;
        },
    };
    match crate::infrastructure::persistence::prune_older_than(&pool, &cutoff.to_rfc3339()).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, db = %db_path.display(), "retention: vault DB prune failed; continuing");
            0
        },
    }
}

#[cfg(not(feature = "persistence"))]
async fn prune_vault_db(_opts: &CrawlOptions, _cutoff: DateTime<Utc>) -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::Duration;

    /// Age a file's mtime into the past via std (`File::set_times`,
    /// stable since 1.75) — no extra dev-dependency needed.
    fn age_file(path: &Path, days_ago: u64) {
        let f = File::options().append(true).open(path).expect("open for times");
        let past = SystemTime::now() - Duration::from_secs(days_ago * 86_400);
        f.set_times(std::fs::FileTimes::new().set_modified(past))
            .expect("set mtime");
    }

    fn cutoff_days_ago(days_ago: u64) -> DateTime<Utc> {
        Utc::now() - chrono::Duration::days(days_ago as i64)
    }

    #[test]
    fn old_files_pruned_recent_kept_root_survives() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path().join("out");
        std::fs::create_dir_all(root.join("run-1")).expect("dir");
        std::fs::write(root.join("run-1/old.md"), "old").expect("old");
        std::fs::write(root.join("run-1/fresh.md"), "fresh").expect("fresh");
        age_file(&root.join("run-1/old.md"), 30);

        let removed = prune_dir_and_count(&root, cutoff_days_ago(7));
        assert_eq!(removed, 1, "only the aged file is pruned");
        assert!(!root.join("run-1/old.md").exists());
        assert!(root.join("run-1/fresh.md").exists(), "recent file survives");
        assert!(root.exists(), "the export root itself is never removed");
    }

    /// Drive [`prune_exports`] synchronously through its blocking body.
    fn prune_dir_and_count(root: &Path, cutoff: DateTime<Utc>) -> u64 {
        let mut removed = 0_u64;
        prune_dir(root, cutoff, &mut removed, true);
        removed
    }

    #[test]
    fn emptied_directories_are_removed_but_never_the_root() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path().join("out");
        std::fs::create_dir_all(root.join("stale")).expect("dir");
        std::fs::write(root.join("stale/a.md"), "a").expect("file");
        age_file(&root.join("stale/a.md"), 30);

        let removed = prune_dir_and_count(&root, cutoff_days_ago(7));
        assert_eq!(removed, 1);
        assert!(!root.join("stale").exists(), "emptied dir is removed");
        assert!(root.exists());
    }

    #[test]
    fn missing_output_root_is_a_noop() {
        let removed = prune_dir_and_count(Path::new("/nonexistent/webfang-retention"), cutoff_days_ago(7));
        assert_eq!(removed, 0);
    }

    #[cfg(feature = "persistence")]
    #[tokio::test]
    async fn vault_db_rows_older_than_cutoff_are_pruned_children_first() {
        // `_tmp` holds the TempDir alive: the DB file lives as long as the test.
        let (_tmp, pool) = test_pool_with_rows().await;

        let removed = crate::infrastructure::persistence::prune_older_than(
            &pool,
            &cutoff_days_ago(7).to_rfc3339(),
        )
        .await
        .expect("prune");

        // 1 resource + 1 chunk + 1 note + 1 note_chunk, all aged.
        assert_eq!(removed, 4, "aged rows across all four tables");
        let remaining = count_rows(&pool).await;
        assert_eq!(remaining, 2, "fresh resource and fresh note survive");
    }

    #[cfg(feature = "persistence")]
    async fn test_pool_with_rows() -> (tempfile::TempDir, deadpool_sqlite::Pool) {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let db_path = tmp.path().join("crawl.db");
        let pool = crate::infrastructure::persistence::create_pool(&db_path, 1).expect("pool");
        crate::infrastructure::persistence::setup_schema(&pool)
            .await
            .expect("schema");

        pool.get()
            .await
            .expect("conn")
            .interact(|c| {
                c.execute_batch("PRAGMA foreign_keys = ON;")?;
                c.execute_batch(
                    "INSERT INTO resources (url, title, status, created_at, updated_at) VALUES \
                     ('https://old.example/a', 'old', 'ok', '2020-01-01T00:00:00+00:00', '2020-01-01T00:00:00+00:00'),\
                     ('https://new.example/b', 'new', 'ok', datetime('now'), datetime('now'));\
                     INSERT INTO chunks (id, resource_url, chunk_index, content, created_at) VALUES \
                     ('old-chunk', 'https://old.example/a', 0, 'x', '2020-01-01T00:00:00+00:00'),\
                     ('new-chunk', 'https://new.example/b', 0, 'y', datetime('now'));\
                     INSERT INTO notes (path, content_hash, mtime_secs, created_at, updated_at) VALUES \
                     ('/old.md', 'h1', 1, '2020-01-01T00:00:00+00:00', '2020-01-01T00:00:00+00:00'),\
                     ('/new.md', 'h2', 2, datetime('now'), datetime('now'));\
                     INSERT INTO note_chunks (note_id, chunk_index, content, created_at) VALUES \
                     (1, 0, 'x', '2020-01-01T00:00:00+00:00'),\
                     (2, 0, 'y', datetime('now'));",
                )
            })
            .await
            .expect("interact")
            .expect("seed rows");
        (tmp, pool)
    }

    #[cfg(feature = "persistence")]
    async fn count_rows(pool: &deadpool_sqlite::Pool) -> u64 {
        pool.get()
            .await
            .expect("conn")
            .interact(|c| {
                Ok::<_, rusqlite::Error>(
                    c.query_row(
                        "SELECT (SELECT count(*) FROM resources) + \
                         (SELECT count(*) FROM chunks) + \
                         (SELECT count(*) FROM notes) + \
                         (SELECT count(*) FROM note_chunks)",
                        [],
                        |r| r.get::<_, u64>(0),
                    )?,
                )
            })
            .await
            .expect("interact")
            .expect("count")
    }
}

//! SQLite-backed fingerprint repository (#792 Slice B).
//!
//! Persists per-site extraction failure fingerprints in a dedicated
//! `extraction_fingerprints` table inside the same WAL-mode pool used by the
//! elastic ingestion pipeline. Upserts on the
//! `(site_base_url, selector_signature)` pair so repeated failures accumulate
//! a count instead of duplicating rows.

use std::future::Future;
use std::pin::Pin;

use deadpool_sqlite::Pool;

use crate::domain::extraction_quality::FingerprintRecord;
use crate::domain::fingerprint_repository::FingerprintRepository;
use crate::error::ScraperError;

/// Forward-only schema for the fingerprint table (`CREATE ... IF NOT EXISTS`).
const FINGERPRINT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS extraction_fingerprints (\
    site_base_url TEXT NOT NULL,\
    selector_signature TEXT NOT NULL,\
    score_at_failure REAL NOT NULL,\
    failure_count INTEGER NOT NULL DEFAULT 1,\
    last_seen INTEGER NOT NULL,\
    last_note TEXT,\
    PRIMARY KEY (site_base_url, selector_signature)\
);";

/// SQLite-backed [`FingerprintRepository`].
///
/// Reuses the WAL-mode pool from
/// [`crate::infrastructure::persistence::sqlite::create_pool`]. Schema creation
/// is explicit — [`SqliteFingerprintRepository::setup_schema`] MUST be called
/// at startup, matching the frozen decision #1 pattern of
/// [`crate::infrastructure::persistence::sqlite::SqliteVectorRepository`].
#[derive(Debug, Clone)]
pub struct SqliteFingerprintRepository {
    pool: Pool,
}

impl SqliteFingerprintRepository {
    /// Wrap an existing WAL-mode pool.
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Create the `extraction_fingerprints` table if missing.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] if the DDL batch fails.
    pub async fn setup_schema(&self) -> Result<(), ScraperError> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| ScraperError::persistence(format!("obtener conexión SQLite: {e}")))?;
        conn.interact(|c| c.execute_batch(FINGERPRINT_DDL))
            .await
            .map_err(|e| ScraperError::persistence(format!("ddl fingerprint (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("ddl fingerprint: {e}")))?;
        Ok(())
    }
}

impl FingerprintRepository for SqliteFingerprintRepository {
    fn record_failure<'a>(
        &'a self,
        record: &'a FingerprintRecord,
    ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            // Own all borrowed inputs: the `interact` closure must be `Send + 'static`.
            let site = record.site_base_url.clone();
            let signature = record.selector_signature.clone();
            let score = record.score_at_failure;
            let last_seen = record.last_seen;
            let note = record.last_note.clone();

            let conn =
                self.pool.get().await.map_err(|e| {
                    ScraperError::persistence(format!("obtener conexión SQLite: {e}"))
                })?;
            let count = conn
                    .interact(move |c| {
                        c.execute(
                            "INSERT INTO extraction_fingerprints \
                     (site_base_url, selector_signature, score_at_failure, failure_count, last_seen, last_note) \
                         VALUES (?1, ?2, ?3, 1, ?4, ?5) \
                         ON CONFLICT(site_base_url, selector_signature) DO UPDATE SET \
                         score_at_failure = excluded.score_at_failure, \
                         failure_count = failure_count + 1, \
                         last_seen = excluded.last_seen, \
                         last_note = excluded.last_note",
                            rusqlite::params![site, signature, score, last_seen, note],
                        )?;
                        c.query_row(
                            "SELECT failure_count FROM extraction_fingerprints \
                             WHERE site_base_url = ?1 AND selector_signature = ?2",
                            rusqlite::params![site, signature],
                            |row| row.get(0),
                        )
                    })
            .await
            .map_err(|e| ScraperError::persistence(format!("record_failure (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("record_failure: {e}")))?;
            Ok(count)
        })
    }

    fn get_failure_count<'a>(
        &'a self,
        site_base_url: &'a str,
        selector_signature: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let site = site_base_url.to_owned();
            let signature = selector_signature.to_owned();

            let conn =
                self.pool.get().await.map_err(|e| {
                    ScraperError::persistence(format!("obtener conexión SQLite: {e}"))
                })?;
            let count = conn
                .interact(move |c| {
                    c.query_row(
                        "SELECT failure_count FROM extraction_fingerprints \
                             WHERE site_base_url = ?1 AND selector_signature = ?2",
                        rusqlite::params![site, signature],
                        |row| row.get(0),
                    )
                    .or_else(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => Ok(0),
                        other => Err(other),
                    })
                })
                .await
                .map_err(|e| {
                    ScraperError::persistence(format!("get_failure_count (interact): {e}"))
                })?
                .map_err(|e| ScraperError::persistence(format!("get_failure_count: {e}")))?;
            Ok(count)
        })
    }
}

//! Integration tests for FingerprintRepository — real I/O with temp dirs (#792 Slice B).
//!
//! Exercises upsert semantics, failure counting, and the no-op backend.
//! SQLite tests require the `persistence` feature; the no-op test always runs.

use webfang_core::domain::extraction_quality::FingerprintRecord;
use webfang_core::domain::fingerprint_repository::FingerprintRepository;
use webfang_core::infrastructure::fingerprint::NoopFingerprintRepository;

fn record(site: &str, signature: &str, score: f64) -> FingerprintRecord {
    FingerprintRecord {
        site_base_url: site.to_string(),
        selector_signature: signature.to_string(),
        score_at_failure: score,
        failure_count: 1,
        last_seen: 1_700_000_000,
        last_note: Some("selector matched 0 nodes".to_string()),
    }
}

// ===== NO-OP BACKEND (always compiled) =====

/// Noop records nothing and always reports a failure count of 0.
#[tokio::test]
async fn test_noop_record_failure_returns_zero() {
    let repo = NoopFingerprintRepository;

    let count = repo
        .record_failure(&record("https://example.com", "article|.body", 42.0))
        .await
        .unwrap();
    assert_eq!(count, 0, "Noop must always return 0");

    let count = repo
        .get_failure_count("https://example.com", "article|.body")
        .await
        .unwrap();
    assert_eq!(count, 0, "Noop must always report 0 failures");
}

// ===== SQLITE BACKEND (feature-gated) =====

#[cfg(feature = "persistence")]
mod sqlite {
    use super::record;
    use tempfile::TempDir;
    use webfang_core::domain::fingerprint_repository::FingerprintRepository;
    use webfang_core::infrastructure::persistence::{create_pool, SqliteFingerprintRepository};

    async fn repo_in(tmp: &TempDir) -> SqliteFingerprintRepository {
        let pool = create_pool(&tmp.path().join("fingerprints.db"), 1).unwrap();
        let repo = SqliteFingerprintRepository::new(pool);
        repo.setup_schema().await.unwrap();
        repo
    }

    /// First record inserts with count 1; repeats upsert and increment.
    #[tokio::test]
    async fn test_sqlite_upsert_increments_failure_count() {
        let tmp = TempDir::new().unwrap();
        let repo = repo_in(&tmp).await;

        let count = repo
            .record_failure(&record("https://example.com", "article|.body", 42.0))
            .await
            .unwrap();
        assert_eq!(count, 1, "first occurrence must report count 1");

        let count = repo
            .record_failure(&record("https://example.com", "article|.body", 38.0))
            .await
            .unwrap();
        assert_eq!(count, 2, "second occurrence must increment to 2");

        let count = repo
            .record_failure(&record("https://example.com", "article|.body", 35.0))
            .await
            .unwrap();
        assert_eq!(count, 3, "third occurrence must increment to 3");
    }

    /// Distinct site/selector pairs are tracked independently.
    #[tokio::test]
    async fn test_sqlite_distinct_pairs_are_independent() {
        let tmp = TempDir::new().unwrap();
        let repo = repo_in(&tmp).await;

        repo.record_failure(&record("https://a.com", "article|.body", 40.0))
            .await
            .unwrap();
        repo.record_failure(&record("https://b.com", "article|.body", 40.0))
            .await
            .unwrap();
        repo.record_failure(&record("https://a.com", "main|.content", 40.0))
            .await
            .unwrap();

        assert_eq!(
            repo.get_failure_count("https://a.com", "article|.body")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            repo.get_failure_count("https://b.com", "article|.body")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            repo.get_failure_count("https://a.com", "main|.content")
                .await
                .unwrap(),
            1
        );
    }

    /// Unknown pairs report 0 instead of erroring.
    #[tokio::test]
    async fn test_sqlite_unknown_pair_returns_zero() {
        let tmp = TempDir::new().unwrap();
        let repo = repo_in(&tmp).await;

        let count = repo
            .get_failure_count("https://never-seen.com", "article|.body")
            .await
            .unwrap();
        assert_eq!(count, 0, "unknown pair must report 0");
    }

    /// Counts survive a repository drop + reopen (real persistence, not cache).
    #[tokio::test]
    async fn test_sqlite_counts_persist_across_reopen() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("fingerprints.db");

        {
            let pool = create_pool(&db_path, 1).unwrap();
            let repo = SqliteFingerprintRepository::new(pool);
            repo.setup_schema().await.unwrap();
            repo.record_failure(&record("https://example.com", "article|.body", 42.0))
                .await
                .unwrap();
            repo.record_failure(&record("https://example.com", "article|.body", 40.0))
                .await
                .unwrap();
        }

        let pool = create_pool(&db_path, 1).unwrap();
        let repo = SqliteFingerprintRepository::new(pool);
        // No setup_schema on reopen: the table already exists.
        let count = repo
            .get_failure_count("https://example.com", "article|.body")
            .await
            .unwrap();
        assert_eq!(count, 2, "counts must survive reopen");
    }

    /// setup_schema is idempotent — calling it twice doesn't fail.
    #[tokio::test]
    async fn test_sqlite_setup_schema_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let pool = create_pool(&tmp.path().join("fingerprints.db"), 1).unwrap();
        let repo = SqliteFingerprintRepository::new(pool);
        repo.setup_schema().await.unwrap();
        repo.setup_schema().await.unwrap();
    }
}

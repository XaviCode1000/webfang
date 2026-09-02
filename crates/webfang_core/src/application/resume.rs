//! The single resume gate shared by the scrape and batch paths (D5, PR3).
//!
//! Every `--resume` skip decision flows through [`filter_committed`]: a URL
//! is skipped ONLY when its persisted record reconciles into
//! `Stateful<_, Committed>` through the typed boundary — the type-level gate
//! demanded by SC2. Any other status (or no record at all) re-drives from its
//! recorded position.
//!
//! Fresh-run semantics (A2/E10): every run — with or without `--resume` —
//! issues a fresh [`RunId`] and opens the store WITHOUT discarding prior
//! records. A run without `--resume` simply never consults the skip branch;
//! old records stay on disk and remain queryable.

use url::Url;

use crate::domain::exporter::{DomainRecords, RawRecord, RecordStorePort};
use crate::domain::page_state::Committed;
use crate::domain::page_state::Stateful;
use crate::domain::url_validation::{normalize_url, NormalizeConfig};

/// Canonical dedup form: strip fragments/queries, unify `www.` with apex.
/// This is THE key every record is stored and looked up under.
pub(crate) fn canonical_key(url: &str) -> String {
    normalize_url(
        url,
        &NormalizeConfig {
            strip_www: true,
            query_policy: url_normalize::RemoveQueryParameters::All,
        },
    )
}

/// Normalize a host into its record-store domain key (D2): lowercase →
/// strip trailing `.` → strip leading `www.`. Scrape path keys by root-host;
/// batch path keys by seed-URL host; both converge here.
#[must_use]
pub fn normalize_domain_key(host: &str) -> String {
    let lowered = host.trim().to_ascii_lowercase();
    let stripped = lowered.strip_suffix('.').unwrap_or(&lowered);
    stripped
        .strip_prefix("www.")
        .unwrap_or(stripped)
        .to_string()
}

/// Identity of the run that last touched a record (A2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunId(String);

impl RunId {
    /// A fresh uuid v4 for this run.
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// The id as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Load this store's records without ever discarding prior history (A2):
/// unreadable files degrade to an empty in-memory view WITH a named-path
/// warning (`RecordStorePort::load_or_init`) — the original bytes stay on disk.
pub(crate) fn load_preserving(store: &dyn RecordStorePort) -> DomainRecords {
    store.load_or_init()
}

/// The single resume gate: drop URLs whose record is proven `COMMITTED`.
///
/// Returns the URLs still to drive plus a fresh [`RunId`] for the run this
/// decision belongs to. The skip branch is reached ONLY from records that
/// reconcile into `Stateful<RawRecord, Committed>` — never from a string
/// compare on status.
///
/// Records are looked up by canonical URL, so `www.`/apex variants of a
/// committed page share one entry (spec www/apex scenario).
pub fn filter_committed(
    urls: impl IntoIterator<Item = Url>,
    store: &dyn RecordStorePort,
) -> (Vec<Url>, RunId) {
    let records = load_preserving(store);
    let original = urls.into_iter().collect::<Vec<_>>();
    let pending: Vec<Url> = original
        .iter()
        .filter(|url| !is_committed_proven(url.as_str(), &records))
        .cloned()
        .collect();
    let skipped = original.len() - pending.len();
    if skipped > 0 {
        tracing::info!(
            skipped,
            pending = pending.len(),
            "resume gate: skipping COMMITTED records only"
        );
    }
    (pending, RunId::new())
}

/// Type-level skip decision: true iff the record for `url` reconstructs as
/// `Stateful<_, Committed>` (status match + D2 invariant table pass).
fn is_committed_proven(url: &str, records: &DomainRecords) -> bool {
    records
        .get(&canonical_key(url))
        .cloned()
        .map(|record| Stateful::<RawRecord, Committed>::reconcile(record).is_ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::infrastructure::export::RecordStore;

    #[test]
    fn canonical_key_unifies_www_and_strips_queries() {
        assert_eq!(
            canonical_key("https://WWW.Example.com/a?x=1#frag"),
            "https://example.com/a"
        );
    }

    #[test]
    fn run_id_display_matches_inner() {
        let id = RunId::new();
        assert_eq!(id.to_string(), id.as_str());
    }

    // --- triangulation: randomized status mix through the gate -----------
    // SC2: skip-set == exactly the Committed-typed subset, regardless of mix.

    mod proptest_mix {
        use super::*;
        use crate::domain::page_state::PageStatus;
        use std::collections::BTreeMap;

        /// Deterministic pseudo-random status mixes (xorshift); asserts the
        /// gate's skip set equals exactly the Committed entries for every mix.
        #[test]
        fn skip_set_equals_committed_subset_across_random_mixes() {
            const ALL_STATUSES: [PageStatus; 8] = [
                PageStatus::Discovered,
                PageStatus::Queued,
                PageStatus::Fetching,
                PageStatus::Fetched,
                PageStatus::Extracted,
                PageStatus::Processed,
                PageStatus::Exported,
                PageStatus::Committed,
            ];
            let mut seed: u64 = 0x9E3779B97F4A7C15;
            for mix in 0..32 {
                let dir = tempfile::TempDir::new().unwrap();
                let store = RecordStore::new("mix.test").with_state_dir(dir.path().to_path_buf());
                let mut expected_skipped = Vec::new();
                let mut records = BTreeMap::new();
                let mut candidates = Vec::new();
                for i in 0..8u64 {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    let status = ALL_STATUSES[(seed % 8) as usize];
                    let url = format!("https://mix.test/p{i}");
                    candidates.push(url.clone());
                    if status == PageStatus::Committed {
                        expected_skipped.push(url.clone());
                    }
                    records.insert(
                        url.clone(),
                        RawRecord {
                            url: url.clone(),
                            canonical_url: url,
                            run_id: "r".to_string(),
                            content_hash: Some("h".to_string()),
                            attempts: 1,
                            status,
                            last_error: None,
                            output_location: Some("o".to_string()),
                            updated_at: 1,
                        },
                    );
                }
                store.save(&records).unwrap();

                let candidate_urls: Vec<Url> =
                    candidates.iter().map(|u| Url::parse(u).unwrap()).collect();
                let (pending, _) = filter_committed(candidate_urls, &store);

                let pending_strs: Vec<String> = pending.iter().map(Url::to_string).collect();
                assert_eq!(
                    pending_strs.len() + expected_skipped.len(),
                    8,
                    "mix {mix}: partition must be exact"
                );
                for skipped in &expected_skipped {
                    assert!(
                        !pending_strs.contains(skipped),
                        "mix {mix}: committed {skipped} must not re-drive"
                    );
                }
            }
        }
    }
}

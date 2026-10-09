//! Elastic vector ingestion pipeline wiring.
//!
//! Builds and runs the optional vector ingestion pipeline (Elasticsearch-backed
//! or dependency-free JSONL stream) that the orchestrator drives after a scrape.

use std::future::Future;

use tokio::task::JoinSet;
use tracing::{debug, warn, Instrument};

use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;
use crate::domain::config::ScraperConfig;
use crate::domain::repository::DynVectorRepository;
use crate::error::ScraperError;
use crate::infrastructure::observability::log_scrape_error;
use crate::CrawlerConfig;

/// Run the elastic ingestion pipeline on all scraped results.
///
/// Each URL is processed concurrently via a bounded `JoinSet` with
/// concurrency limited by the elastic config's CPU core count.
///
/// Fail-fast (frozen Decision 3 + D2): the first ingestion failure propagates
/// immediately and aborts the crawl, rather than being swallowed as a warning.
/// "Failure" covers all three completion shapes a spawned task can have —
/// including a task that panicked or was cancelled, which used to be dropped
/// by the final drain and reported as success (#1941).
///
/// Attribution: every spawn records which URL its task is ingesting, so a task
/// that never returns — panicked or cancelled — is still reported against the
/// page whose vectors it never ingested, in the structured log and in the
/// operator-facing error text (#1941 follow-up).
pub(super) async fn run_elastic_ingestion(
    ingestion: &std::sync::Arc<
        crate::application::elastic_ingestion::ElasticIngestion<DynVectorRepository>,
    >,
    results: &[crate::domain::ScrapedContent],
) -> Result<(), ScraperError> {
    if results.is_empty() {
        return Ok(());
    }

    let mut tasks = IngestTasks::new();
    // Bounded concurrency derives from the budget model's Operation.elastic
    // tier (task 2.5e) — the canonical detector seam replaces the second
    // `num_cpus` counter; the frozen decision #12 env overrides stay layered
    // in the autotuning path that sizes the ingestion itself.
    let concurrency = crate::domain::budget::BudgetModel::build(
        crate::domain::budget::BudgetOverrides::default(),
        &crate::domain::budget::detector::SystemDetector,
    )
    .elastic()
    .get();

    for result in results {
        let ing = std::sync::Arc::clone(ingestion);
        let url = result.url.clone();

        while tasks.len() >= concurrency {
            // `reap_next` classifies all three completion shapes exactly as the
            // final drain does, so the two sites can never drift apart again.
            match tasks.reap_next().await {
                Some(Ok(())) => {},            // success
                Some(Err(e)) => return Err(e), // ingestion error / join failure (D2 fail-fast)
                None => break,
            }
        }

        let url_str = url.to_string();
        let task_url = url_str.clone();
        tasks.spawn(
            &url_str,
            async move { ing.run(&task_url).await }.in_current_span(),
        );
    }

    // Await remaining tasks (propagate the first error — D2 fail-fast).
    drain_remaining(&mut tasks).await
}

/// Spawned ingestion work plus the URL each task is bound to.
///
/// A task that panics or is aborted never returns its own output — the
/// `JoinSet` hands back a bare `JoinError` — so a URL carried in the task's
/// result would be unavailable on exactly the path this type exists to fix.
/// The URL is therefore recorded here, at spawn time, under the task's own
/// identity, and read back when that task fails to join (#1941).
struct IngestTasks {
    tasks: JoinSet<Result<(), ScraperError>>,
    /// URL per in-flight task, keyed by the id `JoinError::id()` reports.
    url_by_task: std::collections::HashMap<tokio::task::Id, String>,
}

impl IngestTasks {
    fn new() -> Self {
        Self {
            tasks: JoinSet::new(),
            url_by_task: std::collections::HashMap::new(),
        }
    }

    /// Number of tasks still in flight (the throttle window's budget).
    fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Spawn one ingestion task, recording which URL it is ingesting.
    ///
    /// The returned handle is the task's own abort handle (tests cancel with
    /// it); production lets the drain reap the task instead.
    fn spawn<F>(&mut self, url: &str, ingest: F) -> tokio::task::AbortHandle
    where
        F: Future<Output = Result<(), ScraperError>> + Send + 'static,
    {
        let handle = self.tasks.spawn(ingest);
        self.url_by_task.insert(handle.id(), url.to_string());
        handle
    }

    /// Reap one finished task, mapping every completion shape onto `Result`.
    ///
    /// Mirrors [`handle_crawl_result`](crate::application::crawler::crawl_task::handle_crawl_result):
    ///
    /// - `Ok(Ok(()))` — success.
    /// - `Ok(Err(e))` — an application error, propagated verbatim (D2 fail-fast,
    ///   unchanged behaviour).
    /// - `Err(join_err)` — the task never produced a value. The two causes are
    ///   told apart for the operator (cancellation is a control signal, a panic
    ///   is a defect) and **both** are terminal: swallowing either would
    ///   silently drop that URL's vectors while reporting the run as
    ///   successful.
    ///
    /// `None` means the set is drained; `Some(Err(..))` is terminal (D2
    /// fail-fast). Every failure carries the URL of the task that produced it,
    /// resolved here by task identity — a panicking task returns nothing, so
    /// the URL cannot come from the task's own result (#1941).
    async fn reap_next(&mut self) -> Option<Result<(), ScraperError>> {
        let joined = self.tasks.join_next_with_id().await?;
        // `join_next_with_id` hands the id back inside the `Ok` arm, so the
        // `Err` arm is keyed by the id the join error itself reports — both
        // name the same task, so the URL lookup works either way.
        let task_id = match &joined {
            Ok((task_id, _)) => *task_id,
            Err(join_err) => join_err.id(),
        };
        let url = self
            .url_by_task
            .remove(&task_id)
            // LCOV_EXCL_LINE defensive: spawn-records-every-task — every task is recorded at spawn and removed here, so a miss is an invariant break
            .unwrap_or_else(|| UNKNOWN_URL.to_string());
        let outcome = match joined {
            Ok((_task_id, result)) => result,
            Err(join_err) => Err(join_failure(url.as_str(), &join_err)),
        };
        Some(outcome)
    }
}

/// Placeholder identity for a reaped task with no recorded URL.
const UNKNOWN_URL: &str = "unknown";

/// Await every remaining ingestion task, propagating the first failure.
async fn drain_remaining(tasks: &mut IngestTasks) -> Result<(), ScraperError> {
    while let Some(result) = tasks.reap_next().await {
        result?;
    }
    Ok(())
}

/// Render a task that never produced a value as an operator-facing error,
/// logging the panic through the crate's shared error-logging contract.
///
/// `url` is the page that task was ingesting, resolved by the caller from the
/// task's identity — a panicking task returns nothing, so the URL cannot come
/// from the task itself.
fn join_failure(url: &str, join_err: &tokio::task::JoinError) -> ScraperError {
    if join_err.is_cancelled() {
        debug!(url = %url, "elastic ingestion task cancelled before completion");
        ScraperError::ingestion(format!("tarea de ingesta elástica cancelada para {url}"))
    } else {
        // `correlation_id: None` — the run root never reaches this function:
        // its signature is pinned by the orchestrator and batch call sites, so
        // the panic carries the task's own identity only. Threading a run
        // correlation through is a follow-up on those callers, not a local
        // change.
        log_scrape_error(
            join_err,
            url,
            "elastic_ingestion",
            None,
            "elastic ingestion task panicked",
        );
        // The URL rides in the operator-facing text as well: the panic is a
        // `InternalFatal` data-loss defect, and "a task died" is not an
        // actionable report of WHICH page lost its vectors.
        ScraperError::ingestion(format!(
            "tarea de ingesta elástica entró en pánico para {url}"
        ))
    }
}

/// Build the elastic ingestion pipeline for the run.
///
/// `--elastic` and `--output-vectors` are orthogonal vector *destinations*
/// (issue #636), so both can be active at once:
///
/// - `persistence` ON + `--elastic` → SQLite-backed `SqliteVectorRepository`.
/// - `--output-vectors <path|->` → dependency-free `StreamRepository` JSONL sink
///   (available in every build, including the lightweight core binary).
/// - both → a single `ElasticIngestion` over a `MultiVectorRepository` fan-out,
///   persisting to SQLite **and** streaming JSONL in the same run.
/// - otherwise → `None` (no ingestion).
///
/// `vault_ports` (#433) carries the optional vault-search AI ports assembled by
/// the binary layer; whichever are present are injected into the container so
/// the ingestion's `Container` is complete. An empty bundle wires nothing.
pub(super) async fn build_elastic_ingestion(
    opts: &CrawlOptions,
    vault_ports: crate::application::container::VaultAiPorts,
) -> Result<
    Option<
        std::sync::Arc<
            crate::application::elastic_ingestion::ElasticIngestion<DynVectorRepository>,
        >,
    >,
    CliExit,
> {
    let container = match crate::application::container::Container::new(
        CrawlerConfig::new(opts.url.as_url().clone()),
        ScraperConfig::default(),
    )
    .await
    {
        Ok(c) => c.with_vault_ports(vault_ports),
        Err(e) => {
            if opts.elastic.enabled || opts.elastic.output_vectors.is_some() {
                return Err(CliExit::IoError(format!(
                    "no se pudo crear el contenedor para ingesta elástica: {e}"
                )));
            }
            warn!("failed to create container for elastic ingestion: {e}");
            return Ok(None);
        },
    };

    // Wire every active sink (`--elastic` AND/OR `--output-vectors`) into a
    // single ElasticIngestion over a MultiVectorRepository fan-out (issue #636).
    // The Container returns itself untouched when no sink is active, so
    // `elastic_ingestion` stays `None` and no ingestion runs.
    let container = match container.with_elastic_ingestion(opts).await {
        Ok(c) => c,
        Err(e) => {
            return Err(CliExit::IoError(format!(
                "no se pudo inicializar la ingesta de vectores: {e}"
            )))
        },
    };

    Ok(container.elastic_ingestion)
}

#[cfg(test)]
mod tests {
    use super::{drain_remaining, IngestTasks};
    use crate::error::ScraperError;

    /// The URL used by the join-failure tests — an ingestion task that dies
    /// must be attributable to the page whose vectors it never ingested.
    const DEAD_URL: &str = "https://example.test/pagina-rota";

    /// #1941: a task that PANICS yields `Err(JoinError)`, never
    /// `Ok(Err(..))`. The final drain matched only the `Ok(Err(..))` arm, so a
    /// panicked ingestion task was swallowed and `run_elastic_ingestion`
    /// reported success — silently losing that URL's vectors, in direct
    /// contradiction of the frozen D2 fail-fast decision.
    #[tokio::test]
    async fn final_drain_reports_panicked_task_instead_of_swallowing_it() {
        let mut tasks = IngestTasks::new();
        // The abort handle is kept alive by `IngestTasks`: dropping it must not
        // be what makes the task fail, or this test would prove cancellation,
        // not panic.
        tasks.spawn(DEAD_URL, async { panic!("ingestion task blew up") });

        let outcome = drain_remaining(&mut tasks)
            .await
            .expect_err("a panicked ingestion task must not drain as Ok(())");

        // User-facing text is Spanish; assert the distinction the crawl_task
        // contract draws (a panic is a defect, cancellation is a control
        // signal), not the exact wording.
        assert!(
            outcome.to_string().contains("pánico"),
            "a panic must be reported as such, got: {outcome}"
        );
    }

    /// #1941 follow-up: the failure must name the URL it is about. `url = ""`
    /// left the operator unable to tell WHICH page lost its vectors, on a
    /// defect class (`InternalFatal`) whose whole cost is silent data loss.
    #[tokio::test]
    async fn panicked_task_failure_names_the_url_it_was_ingesting() {
        let mut tasks = IngestTasks::new();
        tasks.spawn(DEAD_URL, async { panic!("ingestion task blew up") });

        let outcome = drain_remaining(&mut tasks)
            .await
            .expect_err("a panicked ingestion task must not drain as Ok(())");

        assert!(
            outcome.to_string().contains(DEAD_URL),
            "the operator-facing error must name the URL whose vectors were lost, got: {outcome}"
        );
    }

    /// Triangulation: a task ABORTED before completion also yields
    /// `Err(JoinError::is_cancelled())`, and must likewise not read as
    /// success — the crawl_task contract distinguishes the two, but both are
    /// terminal for the drain.
    #[tokio::test]
    async fn final_drain_reports_cancelled_task_instead_of_swallowing_it() {
        let mut tasks = IngestTasks::new();
        let handle = tasks.spawn(DEAD_URL, async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(())
        });
        handle.abort();

        let outcome = drain_remaining(&mut tasks)
            .await
            .expect_err("a cancelled ingestion task must not drain as Ok(())");
        assert!(
            outcome.to_string().contains("cancelada"),
            "a cancellation must be reported as cancellation, not as a panic: {outcome}"
        );
        assert!(
            outcome.to_string().contains(DEAD_URL),
            "a cancelled task must also name the URL it was ingesting: {outcome}"
        );
    }

    /// The D2 arm the drain already honoured must keep behaving exactly as
    /// before: an application-level ingestion error propagates verbatim.
    #[tokio::test]
    async fn final_drain_still_propagates_ingestion_error() {
        let mut tasks = IngestTasks::new();
        tasks.spawn(DEAD_URL, async {
            Err(ScraperError::ingestion("fallo de ingesta de prueba"))
        });

        let outcome = drain_remaining(&mut tasks)
            .await
            .expect_err("an Ok(Err(..)) arm must keep propagating");
        assert!(
            outcome.to_string().contains("fallo de ingesta de prueba"),
            "the original ingestion error must survive verbatim, got: {outcome}"
        );
    }

    /// Triangulation: every task succeeding drains clean.
    #[tokio::test]
    async fn final_drain_accepts_all_successful_tasks() {
        let mut tasks = IngestTasks::new();
        for i in 0..3 {
            tasks.spawn(&format!("https://example.test/{i}"), async { Ok(()) });
        }

        drain_remaining(&mut tasks)
            .await
            .expect("an all-success drain must stay Ok(())");
    }

    /// Triangulation: with several URLs in flight, the failure is attributed
    /// to the URL of the task that DIED, not to an arbitrary sibling.
    #[tokio::test]
    async fn join_failure_names_the_url_of_the_dead_task_not_a_sibling() {
        let mut tasks = IngestTasks::new();
        tasks.spawn("https://example.test/viva", async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(())
        });
        tasks.spawn("https://example.test/muerta", async { panic!("boom") });

        let outcome = drain_remaining(&mut tasks)
            .await
            .expect_err("the panicked task is terminal");

        assert!(
            outcome.to_string().contains("https://example.test/muerta"),
            "the URL reported must be the dead task's, got: {outcome}"
        );
        assert!(
            !outcome.to_string().contains("https://example.test/viva"),
            "a sibling that succeeded must not be blamed, got: {outcome}"
        );
    }
}

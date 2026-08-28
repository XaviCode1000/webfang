//! Elastic vector ingestion pipeline wiring.
//!
//! Builds and runs the optional vector ingestion pipeline (Elasticsearch-backed
//! or dependency-free JSONL stream) that the orchestrator drives after a scrape.

use tokio::task::JoinSet;
use tracing::{warn, Instrument};

use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;
use crate::domain::config::ScraperConfig;
use crate::domain::repository::DynVectorRepository;
use crate::error::ScraperError;
use crate::CrawlerConfig;

/// Run the elastic ingestion pipeline on all scraped results.
///
/// Each URL is processed concurrently via a bounded `JoinSet` with
/// concurrency limited by the elastic config's CPU core count.
///
/// Fail-fast (frozen Decision 3 + D2): the first ingestion error — including a
/// broken pipe / `WriteZero` while streaming JSONL — propagates immediately and
/// aborts the crawl, rather than being swallowed as a warning.
pub(super) async fn run_elastic_ingestion(
    ingestion: &std::sync::Arc<
        crate::application::elastic_ingestion::ElasticIngestion<DynVectorRepository>,
    >,
    results: &[crate::domain::ScrapedContent],
) -> Result<(), ScraperError> {
    if results.is_empty() {
        return Ok(());
    }

    let mut join_set = JoinSet::new();
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

        while join_set.len() >= concurrency {
            match join_set.join_next().await {
                // `T` is `Result<(), ScraperError>` (the spawned task's output).
                Some(Ok(Ok(()))) => {},            // success
                Some(Ok(Err(e))) => return Err(e), // ingestion error (D2 fail-fast)
                Some(Err(_join_err)) => {
                    return Err(ScraperError::ingestion(
                        "tarea de ingesta elástica cancelada",
                    ));
                },
                None => break,
            }
        }

        join_set.spawn(
            async move {
                let url_str = url.to_string();
                ing.run(&url_str).await
            }
            .in_current_span(),
        );
    }

    // Await remaining tasks (propagate the first error — D2 fail-fast).
    while let Some(result) = join_set.join_next().await {
        if let Ok(Err(e)) = result {
            return Err(e);
        }
    }
    Ok(())
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
        CrawlerConfig::new(opts.url.clone()),
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

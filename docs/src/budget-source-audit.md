# Task 2.8 — Post-rewiring source audit (Group A/E item 1)

Change: `stabilization-concurrency-budget` · branch `fix/concurrency-rewiring` · date 2026-08-23

Audit greps over `crates/` (production paths, tests excluded):
`clamp\(|Semaphore::new|buffer_unordered\(|can_spawn|max\(1\)|as u32|num_cpus::get\(\)|available_parallelism`

## Result: ZERO remaining production sites decide a concurrency number outside the budget model

| Former site (explore §6) | Mechanism | Now derives from | Evidence |
|---|---|---|---|
| engine.rs RateLimiterConfig burst | governor token bucket | `model.burst()` (Q1 DECOUPLE) | commit f5114cd6 (in #886) |
| crawl_scheduler.rs can_spawn/effective_concurrency | JoinSet gating | `model.operation.crawl` | commit 20806c7a (in #886) |
| session_pool.rs SessionPoolConfig.pool_size | DashMap pool slots | `model.domain()` | commit de54342a (in #886) |
| scrape_flow.rs buffer_unordered | buffer_unordered | `model.crawl()` via `scrape_concurrency()` | slice 2 |
| orchestrator.rs scraper/batch propagation | config carriers | `budget.crawl()/.asset()/.batch()` | slice 2 |
| batch/processor.rs Semaphore | Semaphore | fed `budget.batch().get()` from load_batch_manager + build_batch_sink | slice 2 |
| adaptive_engine inference_semaphore | Semaphore | `AdaptiveSelectorOptions.max_concurrent_inference` ← `budget.inference().get()` (webfang_cli main.rs) | slice 2 |
| cli/elastic.rs JoinSet bound | JoinSet gating | `BudgetModel::build(default, SystemDetector).elastic().get()` | slice 2 |
| resource_governor compute_max_instances | RAM Semaphore | pure `derive_max_instances` from the model (delegation + equality test) | commit d0e0bf4f |
| http factory / wreq_downloader pool_size | wreq pool | `detector::system_parallelism()` seam | commit ef622d32 |
| autotuning detect_cpu_cores / ram budgets | autotune budgets | seam (`system_parallelism`) | commit ef622d32 |
| webfang_ai inference workers | worker pool | seam (via core dependency) | commit ef622d32 |
| ConcurrencyConfig clamp ceiling 16 (×2 sites) | legacy carrier | single `clamp_budget` + `MAX_CONCURRENCY_CEILING` | PR #886 |

## Explicit-wins override surfaces (operator flags preserved)

- `--concurrency N` → `BudgetOverrides.crawl` (preflight + From<Args>)
- `WEBFANG_RATE_LIMIT_BURST` → `BudgetOverrides.rate_burst`
- `--batch-concurrency` → `BudgetOverrides.batch` (now `Option<usize>`; omitted = auto)
- `--download-concurrency` → `BudgetOverrides.asset` (now `Option<usize>`; omitted = auto)
- elastic frozen decision #12 precedence unchanged (CLI > env > autodetect), layered on the seam

## Documented SKIPs (non-goals per proposal/design)

- **MCP `CategorySemaphores`** (mcp_server/state.rs): NOT trivially adjacent (separate crate, own ServerOptions limits). Documented skip.
- **domain/config.rs `resolve()` available_parallelism**: legacy carrier consistent with the seam by construction (same underlying source, same auto table); retained for TOML provenance. Its enforcement consumers were all rewired to the model.
- **resource_downloader / elastic_ingestion test semaphores**: test-only fixtures.

## Trivially-adjacent cleanup absorbed

- Dead re-export `infrastructure/config.rs::ConcurrencyConfig` removed (zero consumers; verified by grep).

## Detector census after unification

Production `num_cpus::get()`: **0** (2 remaining in live-machine equivalence tests).
Production `available_parallelism`: only inside `budget/detector.rs` (the seam) and the documented legacy carrier in `domain/config.rs`.

# ADR 0019: Boundedness lives in the capture sink — block is out of scope, two bounds are the contract

- **Status:** Accepted
- **Date:** 2026-09-30
- **Deciders:** Project Architect, `webfang` maintainers
- **Related issues:** #1616 (P0.2, CC-D3, PERF-IO-1, PERF-SEM-1)
- **Supersedes:** —

## Context

Three separate audits raised, in total, seven findings that all reduce to one
question: **where does boundedness actually live in the crawler?**

Issue #1616 fixed the two defects (a producer that spawned one deferred task per
full `try_send`, and an unguarded `send().await` that a dead writer parked
forever). Two LOW findings in the same class were left open, and both are design
questions rather than defects, so they are answered here instead of in code.

| id | finding | today |
|---|---|---|
| **PERF-IO-1** | `CrawlResultRepositoryImpl::save` returns a backpressure error to the caller when the append channel is full, shifting persistence latency upstream | `crawl_result_repository.rs`: `try_send` → `CrawlError::Storage("canal lleno, backpressure")` |
| **PERF-SEM-1** | The byte-weighted resource semaphore bounds **memory**, not connection count: permits are acquired per streamed chunk and released by RAII | `resource_downloader.rs`: `acquire_chunk_permits` per chunk, released by `PermitGuard::drop` |

Both are *correct as written*. Neither is a bug, and the audits graded them LOW
for that reason. The risk being retired is not a live defect — it is that the
next person to read this code cannot tell an intentional trade-off from an
accident, and "fixes" it.

## Decision

### 1. Where boundedness lives: in the sink, and nowhere else

**Boundedness is a property of the sink that receives the data, not of the code
that produces it.** Every bound is owned by the component that holds the buffer,
is named there, and is enforced there. Call sites size buffers; they never
*decide whether* a buffer is bounded.

Concretely, `BoundedFileSink` owns a **pair** of bounds and they are the whole
contract:

| bound | owner | limits |
|---|---|---|
| `buffer_size` | `mpsc` channel capacity | pages waiting to be spooled |
| `DEFAULT_MAX_BACKLOG_BYTES` | `Backlog` | page bytes a full channel defers to |

The second half is what #1616 added. A synchronous `CrawlContentSink::capture`
cannot `await` its way through backpressure, so the pre-#1616 code spawned a task
per page to do it — and a bound on the *queue* is not a bound on the *tasks*.
A page now waits in the sink's own byte-bounded `Backlog`, drained by the single
writer task, so the number of in-flight producer tasks is **constant — zero** —
regardless of how far the writer falls behind.

The channel capacity is still derived from the budget tier at the one call site
that builds the sink (`build_batch_sink`, #2.5c) because that tier sizes
concurrency. The backlog ceiling is deliberately **not** derived from it: that is
a *memory* bound, a property of the machine, not of the crawl. Sizing it per
crawl would be the "bounds scattered across call sites" failure this decision
exists to prevent.

### 2. PERF-IO-1: keep blocking-on-full. Do not persist-later.

**A full append channel is backpressure, and it stays an error the caller sees.**

`save()` returning `Storage("canal lleno, backpressure")` on a full channel is
the correct behaviour, and "persist-later" — buffering in the caller until the
writer catches up — was considered and rejected:

- **It relocates the bound, it does not remove it.** A caller-side buffer needs
  its own ceiling, and that ceiling is a second, less observable one on a hotter
  path. #1616 is a direct demonstration of what happens when a hand-off buffer's
  bound is not where the data is.
- **It inverts the failure direction.** Today, a slow disk makes the *crawl*
  slow, which is correct: the crawl is producing data faster than it can be
  durably stored, and the operator sees a slow run. With persist-later, the crawl
  stays fast, the excess lands in memory, and the run's memory profile silently
  becomes a function of disk speed.
- **It would make `save()` lossless-looking but lossy under pressure.** #631
  exists because `--batch` reported success having written nothing. Trading an
  honest error for an invisible one is the same class of bug.

Cost of this choice, stated plainly: `save()` is a sync `fn` on a hot path, so
"block" is really "fail fast and let the caller decide". That is a real
limitation — the caller is the crawl, and a full channel mid-crawl costs a
scraped page. The bound is sized so that reaching it means the disk is
pathologically slow, and the error is typed, so the caller's handling is a
policy decision rather than a guess.

### 3. PERF-SEM-1: the byte semaphore bounds memory, deliberately. No connection budget.

**One semaphore, byte-weighted, is the whole resource budget for resource
downloads. A separate connection-count semaphore is not added.**

`acquire_chunk_permits` draws `chunk_len` permits *before* the chunk is
buffered, and `PermitGuard` returns them on drop. That is a memory bound, and
it is exactly the right primary bound: the OOM risk this code exists to prevent
(`max_size_bytes`, 25 MB default) is a bytes-in-RAM risk.

A connection budget would be a *second* bound on the same downloads, and:

- It would not bound memory better. Bytes already in flight are bounded by the
  byte semaphore; a connection cap only reduces the number of streams, and a
  handful of large streams can hold the whole budget while a hundred small ones
  hold almost none.
- It would need its own number, with no principled derivation available. The
  budget model has tiers for crawl and asset *concurrency*; connection count for
  a byte-weighted downloader is a different quantity, and inventing a constant
  for it is how `Semaphore::new(runtime_value)` sites get a zero (see CC-L2 in
  `scripts/check_concurrency_lints.sh`).
- It would be a second place to tune, and the two would have to be reasoned
  about together on every run.

Connection count is already bounded elsewhere and for a better reason:
`FetchRouter` / `WreqDownloader` pool limits and the asset downloader's own
per-page fan-out. Adding a third, overlapping bound here would be redundancy
without evidence.

**Revisit if** a measurement ever shows connection count — not memory — is the
binding constraint on resource downloads. Absent that measurement, the byte
bound stands alone.

## Consequences

**Accepted:**

- A slow spool degrades capture, not the crawl: pages are refused at the backlog
  ceiling, counted (`BoundedFileSink::dropped()`), and reported exactly once by
  a latched `warn!`, with the batch operator seeing a matching `warn!` from
  `flush_batch_sink`. The run is still valid, and the loss is bounded and named.
- `--batch` can export fewer page bodies than were fetched, in exactly the
  condition where the disk could not keep up. This is the same
  bounded-and-observed trade-off `InMemoryContentSink` already makes for its
  byte cap, and it is a deliberate choice over unbounded memory growth.
- `CrawlResultRepositoryImpl::save` can fail a scrape with a backpressure error
  under sustained disk pressure. This is intended and typed.

**Rejected, and why:**

| option | why not |
|---|---|
| Persist-later / caller-side buffering (PERF-IO-1) | Relocates the bound instead of removing it; hides a slow disk behind a fast crawl. |
| A separate connection-count semaphore (PERF-SEM-1) | Redundant with the byte bound for its purpose (memory), and would need an unprincipled constant. |
| Making `CrawlContentSink::capture` async so the producer can await backpressure | Removes the need for the `Backlog`, and is the design this ADR's decision 1 deliberately declines. Rejected because the trait's sync contract is load-bearing in 16 call sites across the engine, batch manager, and MCP handlers, and because the `Backlog` reaches the same bound without a cross-crate refactor. Worth revisiting only alongside a `RecordStore`/checkpoint redesign — **which this ADR does not propose.** |
| Scattering a per-call-site bound at each `capture` caller | The failure mode decision 1 exists to prevent. |

## Scope fence

Issue #1616 states: *"do not propose a checkpoint or `RecordStore` redesign from
this issue."* This ADR does not. The existing D3 + loom model remains the
strongest concurrency proof in the repository, and the two rejected options
above that would touch it are rejected, not deferred. No `RecordStore` type is
introduced, and the append-only log's format is unchanged.

## Rejected alternatives for the ADR itself

- **Fix the two LOW findings in code.** Both are working as designed; a change
  would be a redesign, not a fix, and the audit explicitly graded them LOW for
  that reason.
- **Close the issue without recording anything.** The findings would be
  re-raised, and the next reader would have no way to tell intent from accident.
  The value here is the written, reviewed answer, exactly as #1616's slicing
  note says: *"A decision, documented, is enough."*

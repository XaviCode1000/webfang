# crawl-session Specification

## Purpose

Defines the behavior of a single crawl run as one owned unit: who may decide what
is true for the whole run, how that decision is validated, how the run identifies
itself, how it positions (never redefines) persisted resume state, and what it must
emit so an operator can reconstruct it. Implementation shape lives in design.md.

## Requirements

### Requirement: Single Run Owner

The system MUST describe one crawl run with exactly one validated run object, and
that object MUST be the only place a run-level setting is decided. A caller MUST
NOT be able to reach a run setting through two independent types.

#### Scenario: A setting exists in exactly one place

- GIVEN a run-level setting such as checkpoint target, robots policy, render strategy or capture sink
- WHEN a caller constructs a crawl run
- THEN the setting is supplied once, on the run object
- AND no second type can carry a conflicting value for the same run

#### Scenario: Unreachable capability is impossible by construction

- GIVEN a run built without an optional capability
- WHEN the run executes
- THEN the resulting behavior equals today's `crawl_site` default for that capability
- AND the omission is recorded in the run's structured output, not silently dropped

### Requirement: Entry-Point Equivalence

Every crawl entry point MUST be expressible through the run object with no loss of
capability relative to the richest current entry point
(`crawl_site_with_options`). Adding a run setting MUST NOT require adding a new
entry function.

#### Scenario: Parity of expressive power

- GIVEN any configuration reachable today through `crawl_site`, `crawl_site_capturing` or `crawl_site_with_options`
- WHEN that configuration is expressed on the run object
- THEN the crawl produces the same result set, error breakdown and exported bytes

#### Scenario: New knob is one addition

- GIVEN a newly added run setting
- WHEN a caller opts in to it
- THEN exactly one construction site changes
- AND no caller silently receives the default for that setting

### Requirement: Validated Before Execution

The system MUST validate a run description completely before starting any worker,
and MUST NOT fail a run for a setting it can honestly degrade.

#### Scenario: Invalid configuration fails early

- GIVEN a run description whose checkpoint interval is zero while a checkpoint target is set, or whose seed is not a validated URL
- WHEN the run is built
- THEN construction fails with a permanent-configuration error before any network request is issued

#### Scenario: Degrable failure warns and proceeds

- GIVEN a checkpoint directory that cannot be created
- WHEN the run is built
- THEN checkpointing is disabled, the reason is logged through the shared error-logging path, and the crawl still produces its result

### Requirement: Durable Resume Formats Are Unchanged

The run object MUST position persisted resume state and MUST NOT redefine it. The
engine checkpoint format, its schema-version gate, the export `StateStore` version
contract and the record-store contract MUST remain byte- and decision-compatible.

#### Scenario: Identical bytes for an identical crawl

- GIVEN the same seed, budget and site responses across two runs
- WHEN each run finishes
- THEN the checkpoint payload equals what the current engine writes for the same crawl

#### Scenario: Stale schema still discards, never crashes

- GIVEN a persisted checkpoint whose version is not current
- WHEN a run resumes from it
- THEN the state is discarded with an informational log and the run starts fresh

#### Scenario: Out-of-scope knobs stay put

- GIVEN the default checkpoint cadence and the `ExportState {version:1}` default
- WHEN this change lands
- THEN both values are unchanged

### Requirement: One Run Identity

Each run MUST own one root identity, minted once per run, shared by every unit of
work and by every phase of the same process invocation, and MUST NOT be persisted
or used to join separate runs.

#### Scenario: Phases share the identity

- GIVEN one invocation that discovers, crawls, scrapes and exports
- WHEN its spans are written to the trace file
- THEN all spans carry one run-root trace identity
- AND each page carries a distinct child identity under that same root

#### Scenario: Separate runs never join

- GIVEN two consecutive runs over the same site
- WHEN their traces are compared
- THEN no identity is shared between them

### Requirement: Cooperative Cancellation And Termination

A run MUST expose one cancellation authority, MUST drain within a bounded time,
MUST leave resumable state when it did not complete, and MUST delete it only when
it completed fully.

#### Scenario: Interrupted run stays resumable

- GIVEN a run interrupted before completion
- WHEN it terminates
- THEN resumable state is written and the run summary reports that state was written

#### Scenario: Completed run leaves no residue

- GIVEN a run that finished with no pending work and no truncation by budget
- WHEN it terminates
- THEN its resumable state is deleted and the summary reports that it was deleted

#### Scenario: Cancellation is success, not failure

- GIVEN a run cancelled by the operator
- WHEN it reaches the CLI boundary
- THEN it exits as cancelled-success per the classification contract, regardless of partial failures

### Requirement: Mandatory Run Observability

Every run MUST emit an entry span with its identifying fields declared at span
creation, one periodic progress event, and one terminal summary with totals,
duration, per-category error breakdown, run identity and the resume-state decision.
Error paths MUST use the shared error-logging helper.

#### Scenario: Trace reconstructs without re-running

- GIVEN a finished run's trace file
- WHEN an operator queries the run-root identity
- THEN every span of that run is recoverable, including the resume-state decision

#### Scenario: Late-bound fields are not used

- GIVEN a field an operator needs for reconstruction
- WHEN it can only be known after the span opens
- THEN it is emitted as a structured event, not added to the span after creation

### Requirement: Error Stratification And User-Facing Text

Run-level failures MUST be classified where they are born and MUST reach the user
through the existing classification-to-exit boundary. User-facing text MUST be in
Spanish; trace fields and logs MUST be in English. No new error level may be
introduced to satisfy a documented but non-existent tier.

#### Scenario: Configuration error exits as configuration error

- GIVEN a rejected run description
- WHEN it reaches the CLI
- THEN it maps to the configuration exit code with a Spanish message

#### Scenario: Raw infrastructure errors never bubble

- GIVEN a storage or network failure underlying a run failure
- WHEN it surfaces at the CLI boundary
- THEN it arrives wrapped through the existing domain/infrastructure error chain, not as a raw type

### Requirement: Dependency Direction Preserved

The run object MUST live in the application layer, MUST depend only inward on
domain types and the existing composition root, and MUST NOT add an entry to the
intra-crate allowlist or any new inter-crate dependency.

#### Scenario: Strict gate stays green

- GIVEN the change as proposed
- WHEN the intra-crate and inter-crate direction gates run
- THEN both pass with the allowlist at its current terminal size

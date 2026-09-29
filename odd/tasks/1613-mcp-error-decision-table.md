# #1613 slice 1 (EC-01): publish the MCP error channel decision table

## Goal
Give the MCP server's error handling a **published contract** so an operator or an
agent consumer can tell a retriable failure from a caller mistake from a server
fault. The server currently uses three channels (JSON-RPC protocol error,
`result.isError = true`, and a success-shaped diagnostic) with no published
mapping, so the choice of channel per failure site is implicit in the code.

## Slice boundary
This slice is **documentation only**. It describes what the code does *today*, so
the code slices that follow have something to conform to:

| Slice | Code | What it will change |
| :--- | :--- | :--- |
| 2 | EC-03 | Conflicted channel selection (Channel A vs B) |
| 3 | EC-02 | rmcp error-code mapping (rows A1/A2 are marked *unverified* until then) |
| 4 | EC-06 / EC-07 | Partial-success encoding |
| 5 | EC-04 | Collapse the third, success-shaped channel |

EC-05 (`expect` sites) is deliberately **untouched** here: the issue itself warns
that blind replacement would be churn.

## What landed
- `docs/src/mcp-error-contract.md` (327 lines) — 83 rows covering all 86
  error-producing construction sites under `crates/webfang_mcp/src/mcp_server/`.
  Every row cites file and line, the wire shape, the code, whether **retrying
  unchanged** can succeed, and what an agent consumer should do.
  - Channel 0 — transport refusals (no JSON-RPC yet)
  - Channel A — JSON-RPC protocol errors (`-32601` / `-32602` / `-32603`)
  - Channel B — `isError: true` tool results (no machine-readable code at all)
  - Channel C — success-shaped diagnostics
  - "Unsettled rows" section for the two rows that depend on rmcp internals
  - Appendix with the full construction-site inventory
- `docs/src/SUMMARY.md` — page registered in the mdBook.
- `crates/webfang_mcp/src/mcp_server/mod.rs` — 5 doc lines pointing module docs
  at the table, so the next handler author finds it.

## Tasks
1. [x] Inventory every error-producing construction site in `mcp_server/`.
2. [x] Classify each site into one of the four channels with a cited source line.
3. [x] Mark the retriable / caller-change / server-fault axis per row.
4. [x] Publish the page, register it in the mdBook, point module docs at it.
5. [x] Commit the slice as one work unit (`89f593df`).
6. [x] Reconstruct this tracking artifact (the subagent's report was lost, and it
       never wrote the ODD task file).
7. [x] Local gate `scripts/ci_fast_gate.sh` re-run on the candidate.
8. [ ] Native review of the candidate.
9. [ ] Push / PR — the maintainer's call, not the agent's.

## Acceptance
- Every error-producing construction site in `mcp_server/` appears in exactly one
  channel section, cited by file and line.
- A consumer can answer "can I retry this unchanged?" from the table alone.
- Rows that are not established are marked unverified rather than smoothed over.
- The evidence class is stated in the document itself, not implied away.

## Evidence
- Commit: `89f593df docs(mcp): publish the error channel decision table (EC-01)`.
- `runtime_validation: NOT_RUN` — every row was read out of the source, not
  observed over a transport. The document carries that label on its first screen.
- Local gate, re-run in this worktree on an isolated `CARGO_TARGET_DIR`:
  - `cargo fmt --all -- --check` — green
  - strict `cargo clippy --all-targets --all-features` — green
  - `RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps` — green
  - repo guards + release-provenance L1 matrix (70/70) — green
  - `cargo nextest run -p webfang_mcp --all-features --tests` — **544/544 green**,
    but only after `cargo build -p webfang_cli` first run. The gate's own
    targeted `-p webfang_mcp` run does **not** build the sibling `webfang`
    binary, and the parity/staleness tests panic with
    `webfang binary not found — build it first`. That is an environmental
    prerequisite, not a defect in this slice; recorded here so the next lane
    does not re-investigate it as a regression.

## Out of scope
- Any production-code change to error construction, mapping, or encoding.
- `crates/webfang_core/` — the table is scoped to the MCP server surface.
- `CHANGELOG.md` (release-plz owns it).

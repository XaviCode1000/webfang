# Observability Engineering Audit

## Goal
Audit WebFang's native tracing, correlation, metrics, and production `jq` queryability without reintroducing OpenTelemetry.

## Tasks
- [x] Map FileTraceLayer, hot-path spans, error events, and async instrumentation.
- [x] Audit CorrelationId root/child propagation across CLI, MCP, batch, crawl, and sitemap paths.
- [x] Audit ScrapeMetrics, MetricsSnapshot, domain overflow, outcomes, timing, and determinism.
- [x] Validate `docs/src/debugging.md` and `scripts/analyze-trace.sh` against the current JSONL schema.
- [x] Write four evidence-based audit deliverables.
- [x] Validate document consistency and query examples.

## Acceptance
- Four root Markdown deliverables exist.
- Claims cite repository paths and distinguish verified behavior from gaps.
- No production code or observability behavior is changed.
- No OpenTelemetry dependency or external collector is introduced.

## Evidence
Audit is static source inspection. The primary implementation is `crates/webfang_core/src/infrastructure/observability/`, with MCP metrics in `crates/webfang_mcp/src/mcp_server/metrics.rs` and query tooling in `docs/src/debugging.md` plus `scripts/analyze-trace.sh`.

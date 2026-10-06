# WebFang Template Map

Filtered mapping from webfang subsystems to awesome-architecture slices. Use only these; all other templates are out-of-scope (notably payment-system/stripe, ecommerce, social-feed, ticketing).

## Filtered Source Inventory

- **Tutorials allowed:** 02, 05, 06, 07, 10, 12, 13, 32, 33
- **Templates allowed:** search-engine, rag-knowledge-base, vector-database, inference-serving, cloud-storage
- **Case allowed:** documind-rag
- **Explicitly excluded:** payment-system/stripe, ecommerce, social-feed, ticketing, and any unlisted template

## Subsystem Map

| WebFang subsystem | Crates | Filtered template | Tutorials | Case | Notes |
| --- | --- | --- | --- | --- | --- |
| **Sitemap discovery** | `webfang_core` (crawler, sitemap) | search-engine | 02 (scalability fundamentals), 07 (queue/backpressure), 10 (indexing/discovery) | — | Bounded channels, sitemap parsing, URL dedup. See `AGENTS.md` async rules. |
| **Downloader (wreq)** | `webfang_core` (http, infra) | search-engine | 02, 05 (caching/rate-limit), 07 | — | Always `wreq` with TLS fingerprint; governor + dashmap for rate limiting. |
| **Extractor (HTML -> Markdown)** | `webfang_core` (extractor, adapters) | search-engine | 06 (storage layering), 10 | — | CPU work via `CpuBridge.dispatch` (spawn_blocking); port = domain trait. |
| **AI cleaner (ONNX embeddings)** | `webfang_ai` + `webfang_core` | rag-knowledge-base, vector-database, inference-serving | 12 (embeddings/vector), 13 (RAG pipeline) | documind-rag | Granite-97M default (384d) via hf_hub cache; `--features ai` gated; `cleaner.clean(html) -> Vec<DocumentChunk>`. Follows documind-rag chunk -> embed -> store pattern. |
| **Export (Markdown/JSON/SQLite)** | `webfang_core` (export, state_store) | cloud-storage | 06, 33 (cloud/export) | — | `StateStore` versioned `ExportState {version:1}`; `load_or_default` handles legacy/missing version. |
| **Observability** | `webfang_core` (infrastructure/observability) | — | 32 (observability/tracing) | — | `tracing` + `FileTraceLayer --trace-file` + `CorrelationId`; no OpenTelemetry. Structured fields, `#[instrument]` on hot paths. |
| **TUI selector** | `webfang_tui` -> `webfang_core` | — | 02 | — | ratatui UI; depends inward on core only. |
| **MCP server** | `webfang_mcp` -> `webfang_core`, `webfang_ai` (feature-gated) | rag-knowledge-base (tool surface) | 13 | documind-rag | Canonical path `crates/webfang_mcp/src/mcp_server/`; 36 tools, Streamable HTTP at 127.0.0.1:8080/mcp. |

## Usage

1. Pick the single row matching the task's subsystem.
2. Apply that template's component shape; cite tutorials only for rationale, not for copy-paste.
3. If a request does not map to any row, reject or remap to the closest allowed template and log the exclusion in the ADR.

## Local References

- `../../../AGENTS.md` — inter-crate allow-matrix, Clean Architecture layers, observability contract.
- `../../../crates/webfang_core/src/domain/` — ports (traits) defined inward.
- `../../../crates/webfang_core/src/infrastructure/observability/` — FileTraceLayer and CorrelationId.
- `../../../docs/debugging.md` — trace query cookbook.

# Overview

**WebFang** is a production-ready web scraper built in Rust (1.88), with
Clean Architecture, a TLS-fingerprinting HTTP client, optional AI semantic
cleaning, and sitemap-based crawling.

This book is the narrative documentation. The complete **API reference** for
all five crates is generated from source via `cargo doc` and published
alongside this book under `/api/<crate>/` (e.g. `/api/webfang_core/`).

## Crates

| Crate | Role |
| :--- | :--- |
| `webfang_core` | Domain, application, and infrastructure layers — the scraping engine |
| `webfang_ai` | ONNX embeddings + semantic cleaning (feature-gated) |
| `webfang_mcp` | MCP server (35 tools) |
| `webfang_cli` | CLI binary (`webfang`) |
| `webfang_test_utils` | Shared test helpers |

## Chapters

- **[Debugging & Observability](debugging.md)** — built-in tracing, correlation
  IDs, and the `jq` query cookbook for `debug.jsonl`.
- **[Testing](testing.md)** — E2E integration tests, snapshot strategy, and
  coverage exclusions.
- **[Troubleshooting](troubleshooting.md)** — diagnosing slow crawls, silent
  failures, WAF blocks, and async deadlocks.
- **[CLI Reference](cli-reference.md)** — the complete `webfang` flag
  reference, auto-generated from the binary itself.

## Regenerating this documentation

All heavy compute (mdBook build + rustdoc + link checks) runs in CI. Locally
you only pull the generated result:

```bash
just docs        # downloads the combined NotebookLM/LLM markdown from the latest CI run
just docs-local  # local preview: builds and serves this mdBook only
```

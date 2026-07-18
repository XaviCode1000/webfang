# Research Brief: Modern Web Scraping Techniques for MCP Agent Servers (2026)

**Date**: 2026-07-15
**Depth**: standard (5 sub-agents, one per technique)
**Audience**: Rust developer building a production web scraper (webfang) that needs to integrate with MCP agent servers

## Question

What are the 5 most important modern web scraping techniques used by MCP agent servers in 2026, and how viable is each for implementation in a Rust-based scraper?

## Scope

**In**: Concrete API surfaces, real data formats, actual GitHub repos, token costs/latency where available, Rust implementation viability
**Out**: Marketing fluff, vague "AI-powered" claims without specifics, techniques older than 2024

## Angles (one sub-agent each)

1. **Accessibility Tree (AXTree)** — Playwright MCP ref snapshots, agent-browser, ARIA data format
2. **LLM-First Extraction** — Crawl4AI extract_data, Firecrawl LLM extraction, server-side vs client-side
3. **Vision Grounding (Set-of-Mark)** — Browserbase API, Anthropic Computer Use, token costs
4. **Semantic DOM Pruning** — Jina AI Reader, Readability comparison
5. **Smart Error Hints (Reactive)** — Scrapling, DrissionPage selector failure handling

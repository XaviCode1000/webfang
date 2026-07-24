//! MCP Server — Streamable HTTP transport
//!
//! Launches the webfang MCP server on `http://127.0.0.1:8080/mcp` using the
//! MCP Streamable HTTP protocol (2025-03-26). This is the correct transport
//! for MCP clients that connect over HTTP (Kilo, OpenCode, Claude Desktop
//! via remote config, Cursor, etc.).
//!
//! ## Quick start
//!
//! ```bash
//! # Build and run (release recommended — BoringSSL takes ~45s debug)
//! cargo run --example mcp_server --release -p webfang_mcp
//!
//! # Verify with curl (see testing section below)
//! ```
//!
//! ## Architecture
//!
//! ```text
//! Axum router (/mcp)
//!   └── StreamableHttpService (rmcp)
//!         ├── LocalSessionManager — tracks Mcp-Session-Id per client
//!         └── McpHandler — 35 tools across 8 categories
//!               ├── State: Container (HTTP client, config) + semaphores
//!               └── Tools: scraping, content, export, url_utils,
//!                          security, obsidian, assets, ai
//! ```
//!
//! ## Streamable HTTP protocol (critical for testing)
//!
//! The `StreamableHttpService` maintains **server-side sessions**. Each client
//! gets a unique `Mcp-Session-Id` header that MUST be included in every
//! subsequent request. Without it, the server rejects with:
//!
//! ```text
//! "Unexpected message, expect initialize request"
//! ```
//!
//! ### Full session flow
//!
//! ```text
//! Step 1: Initialize
//!   POST /mcp
//!   Content-Type: application/json
//!   Accept: application/json, text/event-stream
//!   Body: {"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}
//!   Response: Mcp-Session-Id header + server capabilities
//!
//! Step 2: Confirm initialization
//!   POST /mcp
//!   Mcp-Session-Id: <from step 1>
//!   Body: {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
//!   Response: (empty — notification)
//!
//! Step 3: List tools / call tools
//!   POST /mcp
//!   Mcp-Session-Id: <from step 1>
//!   Body: {"jsonrpc":"2.0","id":N,"method":"tools/list","params":{}}
//!   Response: JSON-RPC result with tool definitions
//! ```
//!
//! ### Response format
//!
//! The server responds in **SSE (Server-Sent Events)** format:
//!
//! ```text
//! data: {"jsonrpc":"2.0","id":1,"result":{...}}
//! ```
//!
//! The first `data:` line may be empty. Parse only lines starting with `data: `.
//!
//! ## Testing with curl
//!
//! ```bash
//! # Step 1: Initialize and capture session ID
//! SESSION_ID=$(curl -s -D - -X POST http://127.0.0.1:8080/mcp \
//!   -H "Content-Type: application/json" \
//!   -H "Accept: application/json, text/event-stream" \
//!   -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{ \
//!     "protocolVersion":"2025-03-26","capabilities":{}, \
//!     "clientInfo":{"name":"test","version":"1.0"} \
//!   }}' | grep -i "mcp-session-id" | awk '{print $2}' | tr -d '\r')
//!
//! echo "Session: $SESSION_ID"
//!
//! # Step 2: List tools
//! curl -s -X POST http://127.0.0.1:8080/mcp \
//!   -H "Content-Type: application/json" \
//!   -H "Accept: application/json, text/event-stream" \
//!   -H "Mcp-Session-Id: $SESSION_ID" \
//!   -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
//!
//! # Step 3: Call a tool
//! curl -s -X POST http://127.0.0.1:8080/mcp \
//!   -H "Content-Type: application/json" \
//!   -H "Accept: application/json, text/event-stream" \
//!   -H "Mcp-Session-Id: $SESSION_ID" \
//!   -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{ \
//!     "name":"validate_url","arguments":{"url":"https://example.com"} \
//!   }}'
//! ```
//!
//! ## Testing with Python (stateful session)
//!
//! ```python
//! import json, subprocess, time, select
//!
//! proc = subprocess.Popen(
//!     ['cargo', 'run', '-p', 'webfang_mcp', '--example', 'mcp_server', '--release'],
//!     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
//! )
//!
//! def send(msg):
//!     proc.stdin.write((json.dumps(msg) + '\n').encode())
//!     proc.stdin.flush()
//!
//! def recv(timeout=5):
//!     ready, _, _ = select.select([proc.stdout], [], [], timeout)
//!     if ready:
//!         line = proc.stdout.readline().decode().strip()
//!         if line.startswith('data: '):
//!             return json.loads(line[6:])
//!     return None
//!
//! # Initialize
//! send({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
//!     'protocolVersion': '2025-03-26', 'capabilities': {},
//!     'clientInfo': {'name': 'test', 'version': '1.0'}
//! }})
//! resp = recv()
//!
//! # Send initialized notification
//! send({'jsonrpc': '2.0', 'method': 'notifications/initialized', 'params': {}})
//! time.sleep(0.3)
//!
//! # List tools
//! send({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list', 'params': {}})
//! resp = recv()
//! print(f"Tools: {len(resp['result']['tools'])}")
//!
//! # Call tool
//! send({'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call', 'params': {
//!     'name': 'validate_url', 'arguments': {'url': 'https://example.com'}
//! }})
//! resp = recv()
//! print(resp['result']['content'][0]['text'])
//!
//! proc.terminate()
//! ```
//!
//! ## Stdio transport (for subprocess MCP clients)
//!
//! For clients that launch the server as a subprocess (OpenCode, Claude Desktop),
//! use `mcp_server_stdio` instead:
//!
//! ```bash
//! cargo run -p webfang_mcp --example mcp_server_stdio
//! ```
//!
//! The stdio transport reads JSON-RPC from stdin and writes to stdout.
//! All logging goes to stderr. See `mcp_server_stdio.rs` for details.
//!
//! ## Complete tool catalog (35 tools)
//!
//! ### Category 1: Scraping Core (8 tools)
//!
//! | Tool | Description | Key params |
//! |------|-------------|------------|
//! | `scrape_url` | Single URL → Readability extraction | `url` |
//! | `scrape_with_options` | Configurable scrape with CSS selector | `url`, `selector?`, `max_pages?`, `download_images?`, `download_documents?` |
//! | `scrape_batch` | Multiple URLs with concurrency | `urls[]`, `concurrency?` |
//! | `crawl_site` | BFS website crawl | `url`, `max_depth?`, `max_pages?` |
//! | `crawl_with_sitemap` | Sitemap-based crawl | `url`, `sitemap_url?` |
//! | `discover_urls` | Extract links from one page | `url` |
//! | `discover_sitemap` | Auto-discover sitemap from robots.txt | `url` |
//! | `detect_spa` | Check if page needs JS rendering | `url` |
//!
//! ### Category 2: Content Processing (7 tools)
//!
//! | Tool | Description | Key params |
//! |------|-------------|------------|
//! | `clean_html` | Remove scripts/nav/footer/SVG | `html` |
//! | `convert_html_to_markdown` | HTML → Markdown | `html` |
//! | `extract_links` | Extract href list from HTML | `html`, `base_url` |
//! | `highlight_code_blocks` | Add syntax highlighting to fenced code | `markdown` |
//! | `convert_wiki_links` | HTTP links → Obsidian [[wiki-links]] | `markdown`, `base_domain` |
//! | `generate_frontmatter` | YAML frontmatter for scraped docs | `title?`, `url?`, `author?`, `excerpt?`, `tags?` |
//! | `generate_rich_metadata` | Word count, reading time, language | `content?` |
//!
//! ### Category 3: Export (4 tools)
//!
//! | Tool | Description | Key params |
//! |------|-------------|------------|
//! | `export_file` | Save as MD/TXT/JSON | `output_dir`, `filename`, `format` |
//! | `export_jsonl` | Export to JSONL (RAG-ready) | `output_dir?`, `filename?` |
//! | `export_vector` | Export with embeddings for vector DB | `output_dir?`, `filename?` |
//! | `process_export_pipeline` | Scrape → chunk → validate → export | `url?`, `format?` |
//!
//! ### Category 4: URL Utilities (6 tools)
//!
//! | Tool | Description | Key params |
//! |------|-------------|------------|
//! | `validate_url` | RFC 3986 parse + validate | `url` |
//! | `extract_domain` | Extract host from URL | `url` |
//! | `normalize_url` | Remove fragments, sort query params | `url` |
//! | `match_url_pattern` | Glob pattern match against URL | `url`, `pattern` |
//! | `is_internal_link` | Check same domain as seed | `url`, `seed_domain` |
//! | `url_to_file_path` | URL → domain-based file path | `url` |
//!
//! ### Category 5: Security & Diagnostics (4 tools)
//!
//! | Tool | Description | Key params |
//! |------|-------------|------------|
//! | `detect_waf` | Scan HTML for WAF/CAPTCHA signatures | `html` (NOT url — pass scraped HTML) |
//! | `verify_waf_integrity` | Multi-layer WAF inspection | `html?` |
//! | `list_waf_providers` | List detectable WAF providers | (none) |
//! | `get_scrape_metrics` | Request timing, status codes | (none) |
//!
//! ### Category 6: Obsidian Integration (4 tools)
//!
//! | Tool | Description | Key params |
//! |------|-------------|------------|
//! | `detect_obsidian_vault` | Auto-detect vault path | `vault_path?` |
//! | `build_obsidian_uri` | Build obsidian:// protocol URI | `vault_name`, `file_path` |
//! | `open_in_obsidian` | Open note in Obsidian app | `vault_name`, `file_path` |
//! | `search_obsidian` | Semantic search (requires --features ai) | `query`, `vault_path?`, `limit?` |
//!
//! ### Category 7: Asset Management (1 tool)
//!
//! | Tool | Description | Key params |
//! |------|-------------|------------|
//! | `download_assets` | Download images/docs from HTML | `html`, `base_url`, `images?`, `documents?` |
//!
//! **⚠️ Not yet implemented** — see #170. Returns explicit error.
//!
//! ### Category 8: AI Semantic (feature-gated)
//!
//! Requires `--features ai` (~90MB ONNX model).
//!
//! | Tool | Description |
//! |------|-------------|
//! | `semantic_clean` | LLM-based content cleaning |
//! | `score_relevance` | Vector similarity scoring |
//! | `generate_embedding` | text → 384-dim embedding |
//!
//! ## Selector feature (scrape_with_options)
//!
//! The `selector` parameter accepts CSS selectors for targeted extraction:
//!
//! ```json
//! {"name": "scrape_with_options", "arguments": {
//!   "url": "https://example.com",
//!   "selector": "article h2, .post-title"
//! }}
//! ```
//!
//! Response includes selector metadata:
//!
//! ```json
//! {
//!   "results": [...],
//!   "selector_applied": true,
//!   "selector_matched": true,
//!   "diagnostic": null
//! }
//! ```
//!
//! When selector matches zero elements, returns fallback with diagnostic:
//!
//! ```json
//! {
//!   "results": [...full page...],
//!   "selector_applied": true,
//!   "selector_matched": false,
//!   "diagnostic": {
//!     "error_kind": "ZeroMatches",
//!     "report": { "element_count": 15, "tag_counts": {...} },
//!     "suggestions": [{ "selector": ".main-content", "score": 0.85 }]
//!   }
//! }
//! ```
//!
//! ## Backpressure
//!
//! Each tool category has its own `tokio::sync::Semaphore` to prevent
//! resource exhaustion. Concurrent tool calls in the same category will
//! queue behind the semaphore.
//!
//! ## Connection pooling
//!
//! Without a shared `Downloader`, each tool call creates a fresh connection
//! pool. For production, inject a shared `Downloader` via
//! `McpState::with_downloader()`. See `server.rs` docs for example.

use std::net::SocketAddr;

use anyhow::Result;
use webfang_core::config::Config;
use webfang_core::di::{Container, ContainerExt};
use webfang_mcp::mcp_server::server::{start_mcp_server, DEFAULT_MCP_ADDR};
use webfang_mcp::mcp_server::McpState;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let config = Config::default();
    let container = Container::from_config(config)
        .await
        .map_err(|e| anyhow::anyhow!("failed to build container: {e}"))?;
    let state = McpState::new(container);
    let addr: SocketAddr = DEFAULT_MCP_ADDR.parse()?;
    start_mcp_server(state, addr).await
}

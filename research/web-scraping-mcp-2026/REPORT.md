# 5 Modern Web Scraping Techniques for MCP Agent Servers (2026)

> Generated 2026-07-15 · depth: standard · 45+ sources · workspace: research/web-scraping-mcp-2026/

## Executive summary

- **Accessibility Tree (AXTree)** is the dominant LLM-agent browser interface in 2026. Playwright MCP snapshots cost ~200-400 tokens vs 3000-5000 for screenshots [1][3]. Format-only differences produce 51-79% token savings — interactive-only refs vs all-element refs is the biggest lever [6]. agent-browser is a 100% Rust-native CLI using CDP directly, proving a Rust scraper can build its own AXTree layer [7]. There is no single W3C JSON schema for AXTree; each tool defines its own shape [9][10].

- **LLM-First Extraction** is server-side by default: Crawl4AI and Firecrawl both run LLM calls internally, the orchestrator agent never touches raw HTML [1][10]. Crawl4AI's `generate_schema()` one-time utility produces reusable CSS/XPath schemas at zero ongoing LLM cost [4]. Firecrawl exposes 12 MCP tools including `/extract` and `/agent` with Pydantic schemas, natively callable by MCP servers [7][8]. Schema quality is the real bottleneck — GPT-4 has ~12% invalid output on complex schemas; Amazon's PARSE research improved this to 98.7% [12].

- **Vision Grounding (Set-of-Mark)** is proven effective but expensive. Anthropic Computer Use screenshots cost ~$0.007 each (Claude Opus 4.7) with fixed 466-499 token system overhead [2][3]. Browserbase/Stagehand defaults to accessibility tree extraction (DOM-grounded), with visual extract being opt-in [4]. Stagehand's `observe()` returns CSS selectors, not coordinates — a key difference from pixel-grounded approaches [6]. Browserbase costs 4.4x more and runs 6.7x slower than Browser Use for equivalent tasks [8].

- **Semantic DOM Pruning** has strong Rust traction. Jina Reader uses Mozilla Readability + Turndown (same stack as webfang) [1][2]. Prune4Web achieves 25-50x DOM reduction and 46.8% → 88.28% grounding accuracy via LLM-generated Python scoring programs [4][5]. Trafilatura outperforms Readability with F-score 0.909 vs 0.801 [6]. Cloudflare markdown.new achieves 82% token reduction at CDN layer [10]. SentienceAPI prunes ~95% of DOM nodes using TreeWalker in Rust/WASM [7].

- **Smart Error Hints (Reactive)**: Scrapling's `scrapling-rs` v0.2.0 (July 2026) ports a 12-factor structural similarity scoring to Rust using `strsim` — directly relevant for webfang [2]. DrissionPage uses `NoneElement` falsy sentinel + configurable failure modes rather than recovery [7][8]. LLM self-healing systems (2026) use local models to propose replacement selectors, validate against live HTML, and promote without redeploy [11]. No existing library combines both Scrapling-style structural similarity AND DrissionPage-style graceful degradation [summary].

## Background & scope

Research question: What are the 5 most important modern web scraping techniques used by MCP agent servers in 2026, and how viable is each for implementation in a Rust-based scraper (webfang)? Scope: 2024-2026 developments, concrete API surfaces, real data formats, Rust implementation viability. Out: marketing claims without specifics, pre-2024 techniques.

## 1. Accessibility Tree (AXTree)

The AXTree is the structural representation of a web page's accessibility semantics — roles, names, states — serialized for LLM consumption. In 2026, it has replaced screenshots as the primary browser interface for agent servers.

### Playwright MCP Snapshot Format

Playwright MCP exposes snapshots as YAML-like indented text where each line encodes a role, accessible name, and optional `ref` for interactive elements [1]:

```
- heading "todos" [level=1]
  - textbox "What needs to be done?" [ref=e5]
  - listitem:
    - checkbox "Toggle Todo" [ref=e10]
```

Refs use format `e` + number (e1, e15, e203), are stable within a single snapshot, and assigned only to interactive elements (buttons, links, inputs) [2]. Role-specific properties appear as bracket annotations: `checkbox "I accept" [checked=true]`, `heading "Title" [level=1]` [4].

Token cost: ~200-400 tokens per page vs ~3000-5000 for screenshots or full DOM. Precision is exact — refs point to specific elements. Reliability is deterministic [3].

### Format Variations and Token Savings

Playwright MCP assigns refs to ALL elements including non-interactive ones (generic, rowgroup, cell), producing ~789 refs on GitHub. WebClaw's compact format only refs interactive elements, producing 245 refs — a 78% reduction [5][6]:

```
GitHub page: 19,409 tokens (Playwright MCP) → 4,304 tokens (WebClaw) — 78% smaller
```

Format-only choices (which elements get refs, what attributes are included) produce 51-79% token savings. Interactive-only refs vs all-element refs is the biggest lever [6].

### agent-browser: Rust-Native AXTree

agent-browser is a 100% native Rust CLI using Chrome/Chromium via CDP with no Playwright or Puppeteer dependency [7]. It produces accessibility tree snapshots with compact `@eN` ref format (e.g. `@e1`, `@e2`), differs from Playwright MCP's bare `eN` format [8].

The agent-browser loop pattern for scraping is: snapshot → LLM parses refs → issue action by ref → re-snapshot → verify. Stale refs must never be reused — refs are scoped to the snapshot in which they were created [12].

### Underlying CDP Protocol

CDP's `Accessibility.getFullAXTree` returns a flat array of nodes with typed objects: `{type: "role", value: "heading"}`, not flat strings [10]. Lightpanda provides a `semantic_tree` CLI format with flat JSON fields (role, name, isInteractive, xpath) distinct from CDP's typed-object schema — the two are not interchangeable [11]. Cloudflare Browser Run exposes a `/accessibilityTree` REST endpoint returning JSON with nested `{role, name, children}` nodes — no refs, just structural data [9].

### Viability for webfang

**Fully implementable in Rust today.** CDP's `Accessibility.getFullAXTree` is the canonical Chrome implementation and can be consumed directly via wreq or a CDP client without Playwright. agent-browser proves this works. webfang could implement its own compact AXTree serialization on top of CDP, choosing interactive-only refs for token efficiency. Deps needed: `chromiumoxide` or similar CDP client (webfang already uses wreq for HTTP).

**Tradeoffs**: AXTree adds a CDP dependency (requires a running browser). For static content, Readability + markdown is cheaper. For interactive scraping where the agent needs to click/type, AXTree is essential. The 200-400 token cost is negligible compared to screenshot-based approaches.

## 2. LLM-First Extraction (Schema + Markdown)

The dominant architecture for LLM-first extraction in 2026 is server-side: the scraping service fetches + cleans HTML, sends it to an LLM with a user-defined schema, and returns structured JSON — the orchestrator agent never touches raw HTML [10].

### Crawl4AI

Crawl4AI's `LLMExtractionStrategy` is provider-agnostic via LiteLLM and always runs LLM calls server-side [1]. It accepts Pydantic JSON schemas and returns validated JSON via `extraction_type="schema"` [2]. The extraction API has two key methods: `extract(url, html)` and `run(url, sections)` — the orchestrator agent calls these, and the LLM call happens server-side within the strategy [3].

Key feature: Crawl4AI offers a one-time LLM-assisted schema generation utility (`generate_schema()`) that produces reusable CSS/XPath schemas. While schema generation uses LLM, it's a one-time cost — the generated schema can be reused for unlimited extractions without further LLM calls [4]. This is the most cost-efficient pattern for stable sites.

`LLMExtractionStrategy` supports automatic chunking with overlap for large pages, sending chunks to the LLM in parallel and merging results — critical for token-limit management [11]:

```
chunk_token_threshold sets the approximate max tokens per chunk...
overlap_rate=0.1 means each subsequent chunk includes 10% of the previous chunk's text
```

### Firecrawl

Firecrawl's `/scrape` endpoint accepts Pydantic schemas via `formats` list with `{"type": "json", "schema": ...}` and returns typed JSON alongside markdown in the same response [5]. The `/extract` endpoint supports both `prompt` (natural language) and `schema` (rigid JSON structure) as mutually optional parameters, with wildcard URL support for full-domain extraction [6].

Firecrawl's `/agent` endpoint takes a natural language prompt + Pydantic schema directly (no `.model_json_schema()` needed) and uses its own models (spark-1-mini/spark-1-pro) to autonomously browse, read, and extract [7]. Two model choices: spark-1-mini (default) for straightforward questions, spark-1-pro for complex research.

Firecrawl's MCP server exposes 12 tools via Model Context Protocol, including all extraction endpoints, making it natively callable by MCP agent servers [8].

### Client-Side SLM (Emerging)

Client-side SLM extraction is now viable with sub-1GB quantized models (e.g. SmolLM2-360M) running via WebGPU/ONNX in the browser, achieving ~70-85% valid JSON on single-entity schemas without post-processing [9]. This is relevant for edge deployment but not yet production-grade.

### Schema Quality: The Real Bottleneck

GPT-4 shows 11.97% invalid response rate on complex extraction tasks. Amazon's PARSE research traces the problem to a mismatch: JSON schemas were built as contracts between human developers and static systems, not as instructions for LLMs. PARSE improved valid JSON rates from 82.3% to 98.7% via schema refinement [12].

### Viability for webfang

**Server-side LLM extraction is implementable but requires an LLM backend.** webfang could expose a tool that accepts markdown + JSON schema, sends to an LLM (OpenAI, Ollama, etc.), returns structured data. The Crawl4AI pattern of one-time schema generation → reusable CSS/XPath is the most cost-efficient for stable sites and fully implementable in Rust with no LLM dependency at extraction time.

**Firecrawl-style MCP integration** would require webfang to expose MCP tools for `/scrape` (markdown + JSON extraction) and `/extract` (prompt/schema-based). This is straightforward — webfang already does the scraping; adding schema-based extraction is an LLM API call layer.

**Tradeoffs**: LLM extraction adds latency (1-10s per page depending on model) and cost ($0.001-0.05 per extraction). For high-volume scraping, Crawl4AI's one-time schema generation → CSS/XPath extraction is far cheaper. For dynamic/changing pages, LLM extraction is more robust than brittle selectors.

## 3. Vision Grounding (Set-of-Mark)

Set-of-Mark (SoM) prompting overlays numbered marks on interactive elements in screenshots, enabling VLMs to ground natural language actions to specific UI elements. GPT-4V with SoM in zero-shot setting outperforms state-of-the-art fully-finetuned grounding models on RefCOCOg [1].

### Anthropic Computer Use

Computer Use tool adds 466-499 tokens to system prompt plus 735 tokens per tool definition, creating fixed overhead per screenshot round-trip [2]. Claude Opus 4.7 screenshots cost ~$0.007 per web-resolution image via area-based tokenization (w×h/750), 2.7x more expensive than GPT-5.5 for the same 1024×1024 image [3]:

```
Web image (1024×1024): Claude = $0.00699, GPT-5.5 = $0.00512, Gemini = $0.00206
```

Anthropic explicitly warns that latency is "too slow" for human-AI interactions and recommends background/information-gathering use cases [7]. The 20251124 beta adds zoom capability for small UI elements [9]. Recommended at medium thinking effort for UI tasks — low effort uses fewer output tokens than disabling thinking entirely due to fewer retries [12].

### Browserbase / Stagehand

Stagehand defaults to accessibility tree extraction, not screenshots — visual extract (`screenshot: true`) is opt-in and only supported with AI SDK clients [4]. This is a critical architectural decision: DOM-grounded extraction avoids screenshot costs entirely for most tasks.

Stagehand integrates SoM prompting for visual grounding — interactive elements are highlighted as numbered marks on the browser viewport [5]. Its `observe()` returns structured actions with selectors (not coordinates), making it DOM-grounded rather than pixel-grounded [6]:

```json
[{
  "description": "Learn more button",
  "method": "click",
  "arguments": [],
  "selector": "xpath=/html[1]/body[1]/shadow-demo[1]//div[1]/button[1]"
}]
```

Browserbase pricing (June 2026): Developer plan at $20/mo, overages at ~$0.12/hour; Extract API at ~$4/1,000 calls without proxies [11].

### Decoupled Architecture (LiteWebAgent)

LiteWebAgent (NAACL 2025) demonstrates a decoupled architecture where SoM is used for action grounding (translating natural language actions into Playwright selectors), while action generation uses trajectory history without the full observation [10]. This significantly reduces prompt tokens — a key design pattern for any MCP agent server.

### Viability for webfang

**Not directly implementable without a VLM.** Vision grounding requires a vision-language model (GPT-4V, Claude, Gemini) to interpret screenshots. webfang could:
1. **Expose screenshots as MCP tools** for agents that want visual grounding
2. **Implement SoM overlay** by combining accessibility tree (for element positions) with screenshot (for visual marks) — the overlay itself is a Rust image processing task
3. **Skip vision entirely** by defaulting to AXTree extraction (like Stagehand), making vision opt-in

**Tradeoffs**: Vision grounding is expensive ($0.007/image), slow (multi-second VLM latency), but necessary for pages where accessibility tree is insufficient (canvas apps, complex visual layouts). The Stagehand model — DOM-first, visual opt-in — is the right default for webfang.

## 4. Semantic DOM Pruning

DOM pruning reduces the HTML sent to LLMs or displayed to agents, cutting token costs while preserving semantic meaning.

### Jina Reader

Jina Reader uses Mozilla Readability as its initial content extraction step, removing headers, footers, navigation bars, and sidebars [1]. It then applies Turndown for HTML-to-markdown conversion — the same stack webfang currently uses [2].

Jina Reader API returns markdown by default with extensive header-based control, including CSS selector-based extraction and exclusion via `x-target-selector` [8]:

```
GET https://r.jina.ai/https://example.com
Headers: X-Return-Format: markdown, X-Target-Selector: .main-content
```

Reader-LM v2 is a 1.5B parameter SLM for HTML-to-Markdown conversion with 256K token context, supporting 29 languages with 20% higher accuracy than its predecessor [3]. Training uses Jina Reader API output as ground truth [9].

### Prune4Web (Research)

Prune4Web achieves 25-50x DOM reduction via LLM-generated Python scoring programs. An LLM generates executable Python scoring programs to dynamically filter DOM elements [4]. Grounding accuracy increases from 46.8% to 88.28% through programmatic DOM pruning [5].

### Benchmarks

Trafilatura outperforms Mozilla Readability in extraction benchmarks (F-score 0.909 vs 0.801) [6]. Cloudflare markdown.new achieves 82% token reduction at CDN layer — raw HTML 9,541 tokens → cleaned markdown 1,678 tokens [10]. SentienceAPI prunes ~95% of DOM nodes using TreeWalker in Rust/WASM [7].

### Viability for webfang

**Already implemented — webfang uses Readability + Turndown.** The research confirms this is the right approach. Potential improvements:
1. **Switch to Trafilatura** for higher F-score (0.909 vs 0.801) — requires a Rust port or Python interop
2. **Add CSS selector fallback** like Jina's `x-target-selector` for cases where automatic extraction misses content
3. **Implement TreeWalker-based pruning** (like SentienceAPI) before Readability to reduce DOM size by ~95% — this is a pure Rust optimization

**Tradeoffs**: DOM pruning is cheap (CPU-only, <100ms) and should always run before any LLM extraction. The 82% token reduction from Cloudflare's approach is essentially what Readability already does. The marginal gain from switching to Trafilatura (0.909 vs 0.801 F-score) may not justify the integration cost.

## 5. Smart Error Hints (Reactive)

When CSS/XPath selectors break due to page changes, reactive systems detect the failure and either recover automatically or provide actionable feedback.

### Scrapling (Adaptive)

Scrapling's adaptive feature uses a two-phase save/match protocol with 12-factor structural similarity scoring [1]. Element fingerprints (tag, text, attributes, siblings, path, parent info) are stored in SQLite and matched using configurable threshold (default 40%) [3]:

```python
# Save phase
element = page.css('#p1', auto_save=True)
# Match phase (selector broken, but Scrapling finds it)
element = page.css('#p1', adaptive=True)
```

The 12 factors: Tag name (exact match), Text content (SequenceMatcher ratio), Attributes (dict_diff), class/id/href/src attributes, Ancestor path, Parent tag, Parent attributes, Parent text, Sibling tags. Final score: `(sum / checks) * 100` [2].

**scrapling-rs v0.2.0** (July 2026) ports this to Rust using `strsim` crate for element relocation [2]. This is directly relevant for webfang.

Critical gap: Scrapling adaptive fails silently when tag types change (article→div, h2→h3) — returns 0 matches at every threshold with no warning signal [5]. Issue #260 proposes `ignore_tag` mode and explicit warnings.

Scrapling includes a built-in MCP server for AI-assisted scraping, pre-filtering content before passing to LLM agents [6].

### DrissionPage (Graceful Degradation)

DrissionPage takes a fundamentally different approach: `NoneElement` falsy sentinel + configurable failure modes rather than recovery [7][8]:

```python
# Default: returns NoneElement (falsy)
element = page.ele('#missing')
if element:
    element.click()

# Global raise mode
Settings.set_raise_when_ele_not_found(True)

# Custom fallback
tab.set.NoneElement_value('not found')
```

DrissionPage has built-in fuzzy matching (`:` for contains, `=` for exact) and 10-second auto-wait on all element searches [9]. Automatic retry on connection errors (default 2 retries) with configurable retry count and interval [10].

### LLM Self-Healing (Emerging)

An LLM-based self-healing system (2026) uses a local model to propose CSS/XPath replacements, validates candidates against live HTML and type schemas, and writes fixes to DB without redeploy [11]:

```
Self-Healer → Redis queue → Python sidecar → fetches current HTML →
trims to token budget → sends to local LLM → LLM proposes candidates
with confidence scores → tested against live HTML via BeautifulSoup/lxml
```

### Viability for webfang

**Scrapling's 12-factor scoring is implementable in Rust today** — `scrapling-rs` proves this with `strsim`. webfang could:
1. Add `auto_save` mode to store element fingerprints in SQLite (webfang already uses SQLite)
2. Implement the 12-factor similarity scoring for adaptive recovery
3. Combine with DrissionPage's `NoneElement` pattern for graceful degradation when recovery fails

The layered approach is optimal: try exact selector → try adaptive similarity → return NoneElement with diagnostic info (what was tried, what the page looks like now).

**Tradeoffs**: Adaptive recovery adds ~10-50ms per element (similarity scoring), SQLite storage overhead (~1KB per element), and a 40% similarity threshold that may need tuning per site. LLM self-healing adds seconds of latency and requires a local model — better as a batch background process than real-time.

## Comparison Table

| Technique | Token Cost | Latency | Rust Viability | webfang Priority |
|-----------|-----------|---------|---------------|-----------------|
| AXTree (CDP) | ~200-400/page | <100ms | High (agent-browser proves it) | **High** — replace screenshots |
| LLM Extraction | 0 (CSS/XPath) or LLM cost | 0 or 1-10s | Medium (needs LLM backend) | **Medium** — for dynamic content |
| Vision Grounding | ~$0.007/image | 1-5s | Low (needs VLM) | **Low** — opt-in only |
| DOM Pruning | CPU-only | <100ms | High (already done) | **Already done** — optimize |
| Smart Error Hints | CPU-only + SQLite | 10-50ms | High (scrapling-rs proves it) | **High** — critical for reliability |

## Open questions

1. **AXTree format standardization**: Should webfang adopt Playwright MCP's YAML-like format, agent-browser's `@eN` format, or CDP's raw typed-object schema? The format choice affects token efficiency by 51-79% [6].

2. **LLM extraction backend**: For server-side LLM extraction, should webfang integrate with external APIs (OpenAI, Anthropic) or bundle a local SLM (SmolLM2-360M via ONNX)? The client-side approach is viable but adds ~90MB to the binary.

3. **Scrapling-rs 12-factor weights**: The Rust port claims "12 equally-weighted factors" but the actual source code may use different weights. Need to verify against `scrapling-rs` v0.2.0 source.

4. **Trafilatura Rust port**: Does a Rust port of Trafilatura exist? If not, is the F-score improvement (0.909 vs 0.801) worth porting from Python?

5. **NoneElement + Adaptive combined**: No existing library combines Scrapling-style structural similarity AND DrissionPage-style graceful degradation. This would be a novel contribution for webfang.

## Sources

[1] Playwright MCP Snapshots — https://playwright.dev/mcp/snapshots (accessed 2026-07-15)
[2] Playwright MCP Accessibility Snapshots Reference — https://qaskills.sh/blog/playwright-mcp-accessibility-snapshots-reference (published 2026-05-03, accessed 2026-07-15)
[3] Playwright MCP Token Cost Comparison — https://playwright.dev/mcp/snapshots (accessed 2026-07-15)
[4] Playwright MCP Bracket Annotations — https://qaskills.sh/blog/playwright-mcp-accessibility-snapshots-reference (published 2026-05-03, accessed 2026-07-15)
[5] Accessibility Tree Formatting Affects Token Cost — https://dev.to/kuroko1t/how-accessibility-tree-formatting-affects-token-cost-in-browser-mcps-n2a (published 2026-02-26, accessed 2026-07-15)
[6] Token Savings: 51-79% from Format-Only Changes — https://dev.to/kuroko1t/how-accessibility-tree-formatting-affects-token-cost-in-browser-mcps-n2a (published 2026-02-26, accessed 2026-07-15)
[7] agent-browser: Rust-Native AXTree CLI — https://agent-browser.dev/ (accessed 2026-07-15)
[8] agent-browser @eN Ref Format — https://agent-browser.dev/ (accessed 2026-07-15)
[9] Cloudflare Browser Run Accessibility Tree — https://developers.cloudflare.com/browser-run/quick-actions/accessibility-tree-endpoint (published 2026-07-07, accessed 2026-07-15)
[10] CDP Accessibility.getFullAXTree — https://lightpanda.io/docs/guides/markdown-axtree (accessed 2026-07-15)
[11] Lightpanda semantic_tree CLI — https://lightpanda.io/docs/guides/markdown-axtree (accessed 2026-07-15)
[12] agent-browser Loop Pattern — https://qaskills.sh/blog/playwright-mcp-accessibility-snapshots-reference (published 2026-05-03, accessed 2026-07-15)
[13] Crawl4AI LLM Extraction Strategy — https://docs.crawl4ai.com/extraction/llm-strategies (accessed 2026-07-15)
[14] Crawl4AI Extraction API — https://docs.crawl4ai.com/api/strategies (accessed 2026-07-15)
[15] Crawl4AI Schema Generation — https://docs.crawl4ai.com/extraction/no-llm-strategies (published 2025-05-02, accessed 2026-07-15)
[16] Firecrawl AI-Powered Data Retrieval — https://www.firecrawl.dev/blog/ai-powered-data-retrieval (published 2026-03-03, accessed 2026-07-15)
[17] Firecrawl Extract API — https://docs.firecrawl.dev/features/extract (accessed 2026-07-15)
[18] Client-Side SLM Extraction — https://www.sitepoint.com/slm-structured-data-extraction-browser (published 2026-02-25, accessed 2026-07-15)
[19] Schema Quality: GPT-4 11.97% Invalid Rate — https://www.context.dev/blog/best-structured-data-extraction-apis-for-llms-2026 (published 2026-06-27, accessed 2026-07-15)
[20] Set-of-Mark Prompting Paper — https://arxiv.org/abs/2310.11441 (published 2023-10-17, accessed 2026-07-15)
[21] Anthropic Computer Use Token Costs — https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool (accessed 2026-07-15)
[22] Vision Model Image Token Costs — https://blog.roboflow.com/image-token-cost-vlm (published 2026-05-04, accessed 2026-07-15)
[23] Stagehand Extract Docs — https://docs.stagehand.dev/v3/basics/extract.md (accessed 2026-07-15)
[24] LiteWebAgent SoM Integration (NAACL 2025) — https://aclanthology.org/2025.naacl-demo.36.pdf (published 2025-04-30, accessed 2026-07-15)
[25] Stagehand observe() Returns Selectors — https://docs.stagehand.dev/v3/basics/observe.md (accessed 2026-07-15)
[26] Browserbase Cost Comparison — https://browser-use.com/posts/web-scraping-guide-2026 (published 2026-03-26, accessed 2026-07-15)
[27] Anthropic Computer Use Zoom — https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool (accessed 2026-07-15)
[28] Anthropic Computer Use Thinking Effort — https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool (accessed 2026-07-15)
[29] Browserbase Pricing — https://scrapegraphai.com/blog/browserbase-pricing (published 2026-06, accessed 2026-07-15)
[30] Jina Reader Uses Readability — https://ritvik19.medium.com/papers-explained-221-reader-lm-7382b9eb6ed9 (published 2024-09-30, accessed 2026-07-15)
[31] Jina Reader API — https://jina.ai/reader (accessed 2026-07-15)
[32] Reader-LM v2 — https://jina.ai/reader (accessed 2026-07-15)
[33] Prune4Web: 25-50x DOM Reduction — https://arxiv.org/html/2511.21398v1 (published 2025-11-26, accessed 2026-07-15)
[34] Prune4Web Grounding Accuracy — https://arxiv.org/html/2511.21398v1 (published 2025-11-26, accessed 2026-07-15)
[35] Trafilatura vs Readability Benchmarks — https://dev.to/stevengonsalvez/browser-tools-for-ai-agents-part-4-skip-the-browser-save-80-on-tokens-304c (published 2026-04-26, accessed 2026-07-15)
[36] SentienceAPI TreeWalker Pruning — https://www.reddit.com/r/LocalLLaMA/comments/1qcxllu/ (published 2026-01-15, accessed 2026-07-15)
[37] Jina Reader CSS Selector — https://github.com/jina-ai/reader (accessed 2026-07-15)
[38] Cloudflare markdown.new 82% Reduction — https://dev.to/stevengonsalvez/browser-tools-for-ai-agents-part-4-skip-the-browser-save-80-on-tokens-304c (published 2026-04-26, accessed 2026-07-15)
[39] Scrapling Adaptive Docs — https://scrapling.readthedocs.io/en/latest/parsing/adaptive/ (accessed 2026-07-15)
[40] scrapling-rs v0.2.0 12-Factor Scoring — https://docs.rs/scrapling/0.2.0/scrapling/adaptive/index.html (published 2026-07-09, accessed 2026-07-15)
[41] Scrapling Silent Failure Issue — https://github.com/D4Vinci/Scrapling/issues/260 (published 2026-05-02, accessed 2026-07-15)
[42] Scrapling MCP Server — https://github.com/D4Vinci/Scrapling (accessed 2026-07-15)
[43] DrissionPage NoneElement — https://github.com/leonvking0/drisson-page-docs/blob/main/browser_control/get_elements/behavior.md (accessed 2026-07-15)
[44] DrissionPage Fuzzy Matching — https://github.com/g1879/DrissionPage (accessed 2026-07-15)
[45] DrissionPage Auto Retry — https://pypi.org/project/DrissionPage/1.7.3/ (accessed 2026-07-15)
[46] LLM Self-Healing CSS Selectors — https://dev.to/viniciuspuerto/when-the-scraper-breaks-itself-building-a-self-healing-css-selector-repair-system-312d (published 2026-04-01, accessed 2026-07-15)

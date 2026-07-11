---
title: WebMCP Tools - Test Native Browser MCP
url: https://web-scraping.dev/mcp-tools
date: 2026-07-11
excerpt: This page registers MCP tools via the navigator.modelContext API (W3C WebMCP). Open it in any Chromium 147+ with the DevToolsWebMCPSupport and WebMCPTesting feature flags enabled, or use a Scrapfly Cloud Browser with enable_mcp=true (flags are enabled by default).
---

This page registers MCP tools via the [`navigator.modelContext`](https://developer.chrome.com/blog/webmcp-epp) API ([W3C WebMCP](https://www.w3.org/groups/cg/webmachinelearning/)). Open it in any Chromium 147+ with the `DevToolsWebMCPSupport` and `WebMCPTesting` feature flags enabled, or use a [Scrapfly Cloud Browser](https://scrapfly.io/docs/cloud-browser-api/mcp) with `enable_mcp=true` (flags are enabled by default). 

**How to test:** Load this page in a WebMCP-enabled browser, then send `WebMCP.enable` over CDP. The browser will fire a `toolsAdded` event listing the 5 tools below. You can also connect an MCP client to the browser's streamable-HTTP MCP endpoint and call tools directly. 

## Registered Tools

| Tool                | Parameters                 | Returns                                         | Description                                                                        |
| ------------------- | -------------------------- | ----------------------------------------------- | ---------------------------------------------------------------------------------- |
| `searchProducts`    | `query` (string, required) | `{matches, products[]}`                         | Search products on the page by keyword. Returns matching product names and prices. |
| `getProductCount`   | none                       | `{count}`                                       | Returns the total number of products displayed on the current page.                |
| `getProductDetails` | `index` (number, required) | `{title, price, description, image}`            | Get structured details for a product by its zero-based index.                      |
| `addToCart`         | `title` (string, required) | `{added, product}`                              | Add a product to the shopping cart by matching title.                              |
| `getPageInfo`       | none                       | `{url, title, productCount, navigationLinks[]}` | Get metadata about the current page: URL, title, product count, and navigation.    |

## Sample Products

These products are available for the tools to interact with:

![Widget A](https://web-scraping.dev/assets/media/icon.png)

### Widget Alpha

$29.99

A versatile widget for everyday automation tasks.

![Widget B](https://web-scraping.dev/assets/media/icon.png)

### Widget Beta

$49.99

Premium widget with advanced data extraction features.

![Widget C](https://web-scraping.dev/assets/media/icon.png)

### Widget Gamma

$19.99

Lightweight widget for quick scraping prototypes.

## Status

Checking... Detecting WebMCP support...

○

`navigator.modelContext` API

checking...

○

`registerTool()` support

checking...

○

Tools registered

checking...

○

Chromium version

checking...
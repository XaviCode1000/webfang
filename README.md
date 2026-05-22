# rust_scraper

**Extrae contenido de cualquier sitio web y guárdalo en Markdown, JSON o directamente en tu Obsidian.**

[![CI](https://github.com/XaviCode1000/rust-scraper/actions/workflows/ci.yml/badge.svg)](https://github.com/XaviCode1000/rust-scraper/actions)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88+-orange)](https://rust-lang.org)

[Quick Start](#-quick-start) · [Features](#-features) · [MCP Server](#-mcp-server) · [Docs](#-documentation) · [Contributing](#-contributing)

---

## Quick Start

```bash
# Instalar
git clone https://github.com/XaviCode1000/rust-scraper.git
cd rust-scraper
cargo install --path .

# Extraer una página
rust_scraper --url https://example.com

# Descubrir todo un sitio
rust_scraper --url https://example.com --use-sitemap --max-pages 50
```

El contenido se guarda en `output/` como Markdown por defecto.

---

## Features

| Feature | Qué hace |
|---------|----------|
| **Limpieza de contenido** | Extrae solo el texto relevante (Readability) — ignora menús, ads, sidebar |
| **Limpieza con IA** | Filtra contenido irrelevante con embeddings ONNX (feature `ai`) |
| **Exportación múltiple** | Markdown, JSON, JSONL (RAG), Vector (embeddings) |
| **Integración Obsidian** | Guarda directo en tu vault con wiki-links y metadatos |
| **Detección de sitemaps** | Encuentra todas las páginas automáticamente |
| **Descarga de assets** | Imágenes y documentos (PDF, DOCX, XLSX) |
| **WAF detection** | Detecta Cloudflare, reCAPTCHA, hCaptcha, DataDome |
| **MCP Server** | 34 herramientas para agentes AI |
| **Rate limiting** | Configurable, respeta Retry-After |
| **Reanudación** | Continúa crawls interrumpidos con `--resume` |

---

## Uso

### Modo básico

```bash
rust_scraper --url https://example.com
```

### Con sitemap

```bash
rust_scraper --url https://example.com --use-sitemap --max-pages 100
```

### En Obsidian

```bash
rust_scraper --url https://example.com/articulo --obsidian-wiki-links --quick-save
```

### Con limpieza de IA

```bash
rust_scraper --url https://example.com --clean-ai --export-format jsonl
```

### Opciones principales

```bash
# Formato de salida
rust_scraper --url https://example.com --format markdown  # (default)
rust_scraper --url https://example.com --format json
rust_scraper --url https://example.com --export-format jsonl
rust_scraper --url https://example.com --export-format vector

# Control de crawl
rust_scraper --url https://example.com --max-pages 50 --delay-ms 1000
rust_scraper --url https://example.com --concurrency 4

# Descarga de assets
rust_scraper --url https://example.com --download-images --download-documents

# Previsualizar
rust_scraper --url https://example.com --dry-run

# Modo silencioso
rust_scraper --url https://example.com --quiet
```

### Referencia completa

```bash
rust_scraper --help
```

---

## MCP Server

rust_scraper incluye un servidor MCP con **34 herramientas** para agentes AI:

```bash
# Servidor stdio (para OpenCode, Claude Desktop, Cursor, etc.)
cargo run --example mcp_server_stdio --quiet

# Servidor HTTP
cargo run --example mcp_server
```

**Herramientas disponibles:**

| Categoría | Tools |
|-----------|-------|
| Scraping | `scrape_url`, `scrape_batch`, `crawl_site`, `crawl_with_sitemap` |
| Contenido | `clean_html`, `extract_links`, `convert_html_to_markdown` |
| WAF | `detect_waf`, `verify_waf_integrity`, `list_waf_providers` |
| Export | `export_file`, `export_jsonl`, `export_vector` |
| Obsidian | `detect_obsidian_vault`, `search_obsidian`, `build_obsidian_uri` |
| URLs | `validate_url`, `normalize_url`, `is_internal_link` |

Configuración para OpenCode — manejado globalmente en `~/.config/opencode/opencode.json`.

---

## Configuración

Archivo: `~/.config/rust_scraper/config.toml`

```toml
format = "markdown"
max_pages = 50
delay_ms = 500
use_sitemap = true
```

Los argumentos de línea de comandos tienen prioridad sobre este archivo.

---

## Features de compilación

| Feature | Qué activa | Instalación |
|---------|-----------|-------------|
| `ai` | Limpieza semántica con ONNX (~90MB modelo) | `cargo install --path . --features ai` |
| `images` | Detección y descarga de imágenes | `cargo install --path . --features images` |
| `documents` | Detección y descarga de documentos | `cargo install --path . --features documents` |
| `full` | Todas las features | `cargo install --path . --features full` |
| `console` | Tokio console (debugging) | `cargo install --path . --features console` |

---

## Para desarrolladores

```bash
# Verificación rápida (fmt + clippy + tests)
just test-ci

# Tests durante desarrollo
just test-dev

# Coverage
just cov

# Audit de seguridad
just audit
```

**Stack:** Rust 1.88 · Tokio · wreq (TLS fingerprint) · ratatui (TUI) · scraper 0.27 · lol_html

**CI:** GitHub Actions ejecuta fmt, clippy, tests, Miri (UB detection), coverage, y security audit en cada push.

---

## Documentación

| Recurso | Qué cubre |
|---------|-----------|
| [Wiki (38 páginas)](docs/wiki/) | Arquitectura, módulos, flujos de ejecución |
| [Viewer interactivo](docs/wiki/index.html) | Navegación con búsqueda |
| [AGENTS.md](AGENTS.md) | Instrucciones para agentes AI |
| `rust_scraper --help` | Referencia CLI completa |

La wiki se genera automáticamente desde el grafo de conocimiento del proyecto:

```bash
gitnexus wiki --model openrouter/auto --concurrency 1 --force
```

---

## Contributing

1. Fork → branch `feature/nombre` → commit → PR
2. Tests deben pasar: `just test-ci`
3. Commits en formato Conventional Commits: `feat:`, `fix:`, `refactor:`, `ci:`, `docs:`

---

## Licencia

MIT OR Apache-2.0

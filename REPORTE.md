# REPORTE — Semantic DOM Pruning before Readability (#791)

## Resumen Ejecutivo

Se implementó el pre-pruning del DOM antes de la extracción con Readability,
eliminando elementos invisibles (display:none, visibility:hidden) y wrappers vacíos,
mejorando la calidad del contenido extraído.

## Cambios Implementados

### Nuevo Archivo
- `crates/webfang_core/src/infrastructure/scraper/dom_pruner.rs` — Lógica de pruning con regex

### Archivos Modificados
- `crates/webfang_core/src/cli/args/crawler.rs` — Flag `--dom-preprune` con default=true
- `crates/webfang_core/src/cli/args/mod.rs` — Propagación de config
- `crates/webfang_core/src/cli/orchestrator.rs` — Wiring del flag al ScraperConfig
- `crates/webfang_core/src/application/crawl_options.rs` — Campo dom_preprune en CrawlLimits
- `crates/webfang_core/src/application/scraper_service.rs` — Integración en clean_html_for_scrape()
- `crates/webfang_core/src/infrastructure/config.rs` — Export público del módulo
- `crates/webfang_core/src/infrastructure/scraper/mod.rs` — Registro del módulo
- `crates/webfang_core/tests/args_test.rs` — Tests del flag CLI

## Características

### Flag CLI
```bash
# DOM pre-pruning HABILITADO por defecto
webfang https://example.com

# Explicitamente habilitar
webfang --dom-preprune https://example.com

# Deshabilitar
webfang --dom-preprune=false https://example.com
```

### Variable de Entorno
```bash
WEBFANG_DOM_PREPRUNE=true webfang https://example.com
WEBFANG_DOM_PREPRUNE=false webfang https://example.com
```

## ¿Qué elimina el pre-pruning?

1. **Elementos con display:none o visibility:hidden** — Mantiene contenido visible
2. **Wrappers vacíos** — `<div><span></span></div>` → solo siempre que tengan padding semántico

## Trazabilidad

- Emite evento de tracing `dom_preprune_reduction` con bytes antes/después
- `reduction_ratio` entre 0.0 y 1.0

## Verificación

```bash
# Tests
cargo test -p webfang_core --lib
cargo test -p webfang_core --test args_test dom_preprune

# Clippy
cargo clippy --all-targets --all-features -- -D warnings

# Format
cargo fmt --check
```

## Historial de Commits

1. `feat(crawler): DOM pre-pruning before Readability (#791)` — Implementación core
2. `fix(cli): dom_preprune flag parsing` — Límite de argumentos
3. `refactor(scraper): fix clippy warnings and formatting in dom_pruner` — Refactorización final

## Notas

- El pre-pruning usa regex-based removal por limitaciones de la API de scraper
- Máximo 5 iteraciones por llamada para evitar bucles infinitos
- Falla abiertamente (retorna HTML original) en caso de errores
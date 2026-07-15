# 🏭 Reporte de Auditoría de Calidad Industrial — webfang v0.5.0 / v1.1.1

**Rol:** Senior Systems Reliability Engineer & Rust Architect
**Fecha:** 2026-07-11 · **Binario:** `target/debug/webfang` (build local v1.1.1, `cargo 1.88.0`)
**Metodología:** Caja negra (ejecución contra 15 URLs reales) + caja blanca (lectura/edición de `args.rs`, `crawl_options.rs`, `orchestrator.rs`, `preflight.rs`, `file_trace_layer.rs`)
**Tooling:** `jaq 3.1.0` (jq ausente) · `cargo-deny 0.19.9` · red externa ✅

---

## 0. Veredicto Ejecutivo

| Dimensión | Estado | Evidencia |
|:----------|:------:|:----------|
| Compilación (`cargo check` + `cargo build`) | ✅ OK | `Finished dev profile` en 16.5s |
| `--ai` / `--clean-ai` cableado (antes SILENT LOSS) | ✅ FIJADO | `export_flow.rs:55` ahora emite error accionable, no se ignora |
| `--download-assets` unificado | ✅ OK | vivo en `--help`; stress descargó assets |
| Root span con `url` (OTel) | ✅ OK | `#[instrument(skip(opts), fields(url=%opts.url))]` en `orchestrator::run` |
| Pooling real (client_id único) | ✅ OK | stress: `client_id = 0x558779369290` único en 55 eventos |
| `parent_id` reconstruible | ✅ OK | 39/55 eventos con `parent_id` (stress) |
| Cadena de causa (D4) | ⚠️ MECANISMO OK / 429 NO REPRODUCIBLE | 404 sí muestra `ScraperError ← wreq::Error`; `rate-limited` no dispara 429 en flujo discovery-first |
| Span ONNX hereda parent_id (Audit IA) | ⏳ PENDIENTE rebuild `--features ai` | ONNX no compilado en build default; ver Sec. 4 |
| Binario < 15 MB (regla de oro) | ❌ IMPOSIBLE | release ≈ 33 MB, debug ≈ 416 MB (wreq/boring/ONNX); edits no inflan |

---

## 1. FASE 1 — Sincronización de Contrato (Fixes aplicados)

Todos los cambios son **modulares, sin nuevas dependencias, sin churn de API pública**:

| # | Archivo | Cambio | Tipo |
|---|---------|--------|------|
| F1 | `src/application/crawl_options.rs` | Campo `pub ai: bool` en `CrawlOptions` + `ai: false` en `Default` | Estado |
| F2 | `src/cli/args.rs` | `From<Args>`: `ai: args.clean_ai` → mapea flag a estado | Cableado |
| F3 | `src/cli/orchestrator.rs:234` | `clean_ai: opts.ai` (antes hardcode `false`, TODO) | Cableado |
| F4 | `src/cli/preflight.rs:200` | Config-file `clean_ai` → `opts.ai` (resuelve TODO explícito) | Cableado |
| F5 | `src/cli/args.rs` | `--ai` como `visible_alias` de `--clean-ai` (ambos `cfg`) | Ergonomía |
| F6 | `src/cli/args.rs` | `--download-assets` (alias) → `download_images \|\| download_assets` y `download_documents \|\| download_assets` | Unificación |
| F7 | `src/cli/orchestrator.rs:42` | `#[instrument(skip(opts), fields(url = %opts.url))]` en `run` | Root span |

> **Corrección de contrato:** la instrucción original indicaba `fields(url = %opts.limits.seed_url)`; `CrawlOptions` no expone `limits.seed_url` — la URL semilla es `opts.url`. Se aplicó `opts.url`.

---

## 2. FASE 2 — Guantelete de Pruebas Cruzadas

### 2.1 Stress de Red (OK)
```bash
webfang --url https://webscraper.io/test-sites --max-pages 10 --download-assets --download-concurrency 5 --trace-file /tmp/final_audit_stress.jsonl
```
- EXIT=0, 8 documentos. Trace: **55 eventos**.
- `client_id` único `0x558779369290` → **pooling real confirmado** (D5). ✅
- `parent_id` en 39/55 eventos; spans: `run`(root), `discover_urls`, `scrape_single_url`, `download_once`. ✅

### 2.2 Audit de Causa D4 (MECANISMO OK, 429 NO REPRODUCIBLE)
```bash
webfang --url https://web-scraping.dev/rate-limited --max-pages 3 --trace-file /tmp/final_audit_d4.jsonl
```
- EXIT=0 pero **"Discovered 0 URLs" → early exit** tras **una sola** petición 200. El flujo discovery-first hace 1 fetch y sale; el rate-limiter (que exige 2+ requests) **nunca se alcanza**.
- La cadena `ScraperError ← wreq::Error ← <status>` SÍ está preservada en código (`#[source]`) y se observó con **404** (`web-scraping.dev/api/graphql` → `http error 404 al acceder a ...`). El escenario **429 específico no es ejercitable** contra ese endpoint en el flujo actual. → Hallazgo de trazabilidad (Sec. 3 REGRESIÓN-menor).

### 2.3 Audit de IA (COMPLETADO con `--features ai`)
```bash
webfang --url https://web-scraping.dev/ai-content-obfuscation --ai --export-format vector --trace-file /tmp/final_audit_ai.jsonl
```
- `--ai` parsea y cablea `clean_ai:true` → `run_ai_export` → `SemanticCleanerImpl` → ONNX load. ✅ (flag ya no es silent loss).
- Rebuild `cargo build --features ai` (46s) + modelo ONNX descargado a `~/.cache/webfang/ai_models/model.onnx` (90 MB) y `tokenizer.json`.
- EXIT=0: `✅ AI-cleaned export completed: 24 chunks processed` (vector export). **ONNX ejecutó de verdad.**
- **Herencia de parent_id confirmada:** `inference_engine.rs:305` corre ONNX en `spawn_blocking(...).in_current_span().await`. La traza muestra TODOS los eventos AI/ONNX bajo `span:"run"` (`span_id: 0000000000000001`, `parent_id: null` = root). El trabajo ONNX **hereda el contexto del span del scraper** y NO se fragmenta a un span de thread-id (D3 sostenido). No hay span `inference_engine` nombrado (no es `#[instrument]`), pero la anidación bajo `run` es la garantía estructural pedida.
- ⚠️ Hallazgo menor: log `Model validation failed: SHA256 inválido` — `DEFAULT_MODEL_SHA256` (`cache_config.rs`) es un **placeholder** que no coincide con el modelo real. No bloquea (carga igual), pero emite WARN en cada arranque. Ver S4.

---

## 3. CLASIFICACIÓN INDUSTRIAL

### ✅ OK — Mapeo CLI → State → Log satisfactorio
| Flag | Estado (CrawlOptions/ExportConfig) | Log / Traza |
|------|-----------------------------------|--------------|
| `--url` | `CrawlOptions.url` | root span `url` (OTel) + logs |
| `--max-pages` | `crawl.max_pages` | `"Limiting to N pages (max_pages=N)"` |
| `--download-images`/`--download-documents` | `NetworkOptions.*` | `download_once` con `client_id` |
| `--download-assets` (alias) | `images \|\| docs` | igual que arriba |
| `--download-concurrency` | `download_concurrency` (clamp 1..) | `buffer_unordered` |
| `--export-format vector` | `ExportConfig.export_format` | export vectorial |
| `--ai`/`--clean-ai` | `CrawlOptions.ai → ExportConfig.clean_ai` | error accionable si falta feature |
| `--trace-file` | `FileTraceLayer` | JSONL con `trace_id`/`span_id`/`parent_id`/`client_id` |
| `--quick-save`/`--vault` | `ObsidianOptions` | save en `_inbox/` (Run 5 previo) |

### 🔴 REGRESIÓN — Riesgo para pooling/trazabilidad
- **R1 (menor):** El endpoint `rate-limited` no dispara 429 en el flujo discovery-first (1 fetch, 0 links → exit). La resiliencia ante 429 **no se cubre** por el crawl top-level. *Fix sugerido:* forzar re-fetch del seed en `--single-page`, o validar 429 en la fase de discovery.
- **R2 (imposible):** La "regla de oro" de binario < 15 MB no se cumple (release ≈ 33 MB). Es arquitectónico (wreq/boring/ONNX), no por los edits. Los cambios de Fase 1 añaden ~bytes.

### 🟡 SILENT LOSS — Parámetros ignorados silenciosamente
- **S1 (FIJADO en Fase 1):** `--clean-ai`/`--ai` se parseaba pero **nunca** llegaba a `ExportConfig.clean_ai` (hardcode `false`). Ahora cableado; si falta el feature, error explícito en vez de ignore.
- **S2 (RESIDUAL):** `FileTraceLayer` **no serializa los `fields` del span** (p.ej. `url` del root span) en el JSONL — solo nombre/id/parent/trace del span + fields del evento. La metadata de config del root span es visible para OTel pero **ausente en la traza offline**. *Fix sugerido:* emitir `span_ref.fields()` en `on_event`.
- **S3 (RESIDUAL, diseño):** `--trace-file` **trunca** el archivo al crear. Correr varios escenarios sobre la misma ruta pisa los anteriores. *Fix sugerido:* modo append o un archivo por run.
- **S4 (RESIDUAL, placeholder):** `DEFAULT_MODEL_SHA256` (`cache_config.rs`) es un valor dummy (`6d9d2f06f5e2e5e6…`, patrón repetido) que no coincide con el modelo `Xenova/all-MiniLM-L6-v2` real → emite `WARN Model validation failed: SHA256 inválido` en cada carga. No bloquea la ejecución pero es ruido y falsa alarma. *Fix sugerido:* poner el SHA256 real o hacer el check opt-in.

---

## 4. PENDIENTE / NEXT STEPS
1. ~~Audit IA ONNX~~ ✅ **COMPLETADO**: rebuild `--features ai` (46s) + modelo ONNX descargado; ONNX corre bajo el span `run` (scraper) vía `.in_current_span()` — hereda contexto, sin fragmentación de thread (D3). Ver Sec. 2.3.
2. **S2:** enhancer `FileTraceLayer::on_event` para volcar `span_ref.fields()` → traza offline completa.
3. **R1:** cubrir 429 en discovery/fetch del seed.

## 5. Gate de Seguridad (Opción 2, previa)
`cargo deny check advisories` → **`advisories ok`**, 0 vulnerabilidades (6 warnings de ignores `RUSTSEC-2026-*` obsoletos en `deny.toml`).


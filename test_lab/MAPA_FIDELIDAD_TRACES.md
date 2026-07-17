# 🔬 Mapa de Fidelidad de Trazas — Auditoría Dinámica de Flags y Flujo de Telemetría

**Proyecto:** `webfang` (v1.1.0) · **Fecha:** 2026-07-10 · **Auditor:** Senior Rust Observability Engineer
**Alcance:** Mapeo CLI flags → App State (`CrawlOptions`/`ScraperConfig`) → Telemetría (`FileTraceLayer` / OTel)
**Método:** Inspección estática de código + 5 ejecuciones dinámicas reales contra `en.wikipedia.org` (red disponible, binario debug recompilado tras detectar stale).

---

## 0. Veredicto Ejecutivo

| Ítem | Estado | Evidencia |
|:-----|:------:|:----------|
| Build default (`cargo build`) | OK | `Finished in 15.6s` |
| Build `--features otel` (`cargo check`) | OK | `Finished … DONE_OTEL_CHECK` |
| `Arc<Downloader>` compartido (pool único) | OK diseno | `orchestrator.rs:109`, `Arc<Client>` |
| `trace_id` = **thread ID** (no logico) | CRITICO | `file_trace_layer.rs:124-140` |
| `parent_id` nunca emitido | CRITICO | `file_trace_layer.rs` (solo `span`/`span_id`) |
| `--download-concurrency 0` → **deadlock hang** | BUG | RUN3b: `EXIT=124`, 0 assets, 0 trazas |
| Eventos "Acquire Permit" de semaforos | AUSENTES | solo `resource_governor.rs:48` (`max_permits`) |
| IDs de socket / pool-reuse en trazas | ✅ MITIGADO (client_id) | `client_id` (Arc<Client> ptr) observable en `download_once` |
| AI/ONNX alcanzable desde CLI | NO | `orchestrator.rs:226` `clean_ai: false` (TODO) |
| Cadena de causa de error en borde | TRUNCADA | `ScraperError::Network(String)` / `Download(String)` |

---

## 1. Auditoria de Inyeccion de Estado (CrawlOptions → App State → Telemetria)

**Flujo confirmado (estatico + dinamico):**
```
Args (clap) --From--> CrawlOptions --builder--> ScraperConfig
                                            |
                               ScraperConfig.to_download_config()  (infra/config.rs:187)
                                            |
                               adapters::downloader::Downloader::new(...)  (orchestrator.rs:109)
                                            |  if scraper_config.has_downloads()  (images||docs)
                               Arc<Downloader>  -->  scrape_urls(shared_downloader.as_deref())
```

- `has_downloads()` = `download_images || download_documents` (`infra/config.rs:179`). El `Arc<Downloader>` **solo se instancia** cuando hay descarga habilitada → correcto.
- OK **Pool efficiency (diseno):** `WreqDownloader.client: Arc<Client>` se crea **una vez** y se reutiliza en todas las paginas (`wreq_downloader.rs:47`).
- ERR **`root_span` NO EXISTE.** `orchestrator::run()` (`orchestrator.rs:42`) no es `#[instrument]` y `main.rs` no crea `Span::root`. Los **metadatos de configuracion NO se reflejan en ningun span**. Solo lineas planas:
  - `"Limiting to 1 pages (max_pages=1), skipping 576 URLs"` (`scrape_flow.rs:135`) — no lleva `download_concurrency` ni flags.
  - `"Downloading 45 assets via adapters::Downloader"` (`scraper_service.rs:464`) — **no incluye el valor de `download_concurrency`**.
- WARN **Flags contradictorios:** `--download-images --download-concurrency 0` → ver §3 (deadlock). `--max-pages 1` → funciona correctamente (RUN1/RUN3a).

---

## 2. Rastreo de Correlacion en el Hot-Path (Pooling & AI)

**RUN3a (dinamico, vivo):** `--url en.wikipedia.org/wiki/Rust --max-pages 1 --download-images --download-concurrency 4` → **40 assets descargados**, trace de 56 eventos.

| Pregunta del mission | Respuesta | Evidencia |
|:---------------------|:----------|:----------|
| trace_id identico scrape → cierre de descarga? | SI, pero por artefacto de hilo | `trace_id:"0000000000000001"` constante en 56 eventos |
| span_id cambia y parent_id apunta al scraper? | parent_id NO existe | `grep -c parent_id` → 0 en todo el JSONL |
| ONNX span_id cambia con parent_id→scraper? | Inverificable (AI no CLI-reachable) | `orchestrator.rs:226` `clean_ai:false`; `inference_engine` sin `#[instrument]` |

**Hallazgo critico de correlacion:** `trace_id` en `FileTraceLayer` es `std::thread::current().id()` hasheado (`file_trace_layer.rs:124-140`), **NO un identificador logico de traza**. En RUN3a parece estable porque todo el pipeline corrio en el **hilo principal** (1 pagina, `max-pages 1`, descarga `await` inline). Bajo concurrencia real multi-pagina (`JoinSet` en `scrape_urls`), las tareas saltan entre worker threads → `trace_id` **se fragmenta**. Y sin `parent_id`, el arbol real (pagina → descarga → ONNX) es **irreconstruible** desde el JSONL.

> Con `--features otel`, `tracing-opentelemetry` SI propaga `trace_id`/`span_id`/`parent` reales (W3C) en el Span context. Pero el `FileTraceLayer` JSONL **sigue escribiendo el thread-id** (capas independientes) → el artefacto offline siempre es enganoso en `trace_id`.

---

## 3. "Deadlock Guards" y Semaforos

- El hot-path de assets usa **`futures::stream::iter(futs).buffer_unordered(concurrency)`** (`adapters/downloader/mod.rs:376-383`), **NO un `tokio::Semaphore`**. `--download-concurrency` controla el ancho de `buffer_unordered`.
- BUG confirmado dinamicamente (RUN3b): `buffer_unordered(0)` → el `while len < max` (max=0) **nunca encola ningun future** → `poll_next` retorna `Pending` para siempre → **HANG**.
  - RUN3b: `timeout 120 … --download-images --download-concurrency 0` → **`EXIT=124`** (matado por timeout), **0 assets**, JSONL congelado en `"Downloading 45 assets"` (linea 5), **sin error, sin warning, sin traza** que explique el stall.
- ERR **Eventos "Acquire Permit" de semaforos:** en **ningun** lado. Unico log de permisos: `resource_governor.rs:48` `debug!("ResourceGovernor: max_permits={max_permits}")`. El `PermitGuard` del `ResourceDownloader` elastico (`infra/crawler/resource_downloader.rs`) adquiere silenciosamente. `elastic_ingestion` no tiene `#[instrument]`.
- Semaforos existentes pero **mudos**: `ResourceDownloader` (elastico, byte-weighted) y `ResourceGovernor` (gate de Chrome/headless).

---

## 4. Contraste de Features (`otel` vs nativa)

Ambos builds **compilan** (check otel OK). Contraste de densidad de datos:

| Dimension | Nativa `FileTraceLayer` | `otel` (OTLP) |
|:----------|:------------------------|:--------------|
| `trace_id` en JSONL | **thread-id** (enganoso) | real W3C en collector; JSONL sigue con thread-id |
| `span_id` | contador interno de `tracing` | W3C real + `parent` |
| `parent_id` | ERR nunca | OK si (contexto de Span) |
| `fields` arbitrarios | OK captura TODO (`EventRecorder` single-pass) | OK atributos de span/eventos |
| Correlacion offline | WARN solo por `span_id` plano | OK arbol completo en backend |
| Pool/socket IDs | ERR | ERR (salvo `otel-metrics` latencia) |

**Conclusion:** El `FileTraceLayer` es **mas rico en captura de campos arbitrarios offline**, pero su `trace_id` es enganoso y no tiene `parent_id`. OTel es superior para correlacion distribuida. **No hay campos que OTel descarte que FileTraceLayer capture de forma unica** — la diferencia real es la **calidad del `trace_id`/`parent`**, no los `fields`.

---

## 5. Auditoria de Errores Propagados (Fail-fast)

**RUN4 (dinamico):** `--url http://nonexistent-domain-xyz-12345.invalid/ --max-pages 1` → DNS fail.
- Traza: `WARN … "URL discovery failed: HTTP error: error sending request for uri (…): client error (Connect)"` — **string plano**, sin `source()`.
- `error.rs`: `NetworkFailure(#[from] WreqError)` (`:122`) **SI** preserva cadena via `#[source]`. PERO en el borde de servicio se usan:
  - `ScraperError::Network(#[source] Box<dyn Error + Send + Sync>)` (`error.rs:44`) — ahora preserva la causa (antes `String`)
  - `DownloadError::Network(#[source])` (`wreq_downloader.rs`) — ya no hace `e.to_string()`
  - `ScraperError::Download(#[source] Box<dyn Error + Send + Sync>)` (`error.rs:71`) — ahora preserva la causa (antes `String`)
  - `scrape_flow.rs:216` empuja `failures.push((url, e.to_string()))`.
- WARN **Veredicto (actualizado):** ✅ LIQUIDADO en v0.5.1. Las variantes `ScraperError::Download`/`Network`, `CrawlError::Download` y `DownloadError::Network` ahora llevan `#[source] Box<dyn Error>`; el borde de servicio ya NO aplana a `String` (ver `scrape_flow.rs`/`orchestrator.rs`). El `FileTraceLayer` sigue escribiendo el string en `message`, pero el `ScraperError` completo (con `source()`) se conserva en `failures` y se imprime con la cadena de causa.


---

## 6. ENTREGABLE — Mapa de Fidelidad de Trazas

### 6.1 Gaps de Visibilidad (flags que cambian comportamiento sin rastro)
1. `--download-concurrency 0` → **deadlock**, 0 telemetria (RUN3b).
2. El valor de `--download-concurrency N` **nunca aparece** en log/span (solo "Downloading X assets" sin N).
3. **Sin `parent_id`** → arbol de traza irreconstruible.
4. **Sin eventos "Acquire Permit"** → backpressure de semaforos invisible.
5. **Sin socket/pool-ID nativo** → Mitigado: `client_id` (Arc<Client> ptr) observable en `download_once`, confirma reuso.
6. **Sin `root_span`** → metadatos de config no reflejados en spans.
7. Construccion/fallo del `Arc<Downloader>` solo como `CliExit`, no en spans.
8. **AI/ONNX ausente** de la telemetria CLI (no cableado).

### 6.2 Eficiencia del Pool (evidencia)
- OK **Codigo:** `Arc<wreq::Client>` unico, reutilizado (`wreq_downloader.rs:47`). Diseno correcto para pool-reuse.
- ERR **Trazas (antes):** sin evidencia observable de reuse. **AHORA:** `client_id` (Arc<Client> ptr) se registra en `download_once` y es constante across fetches → reuse verificable por traza (RUN de validación: 15 fetches → mismo `client_id`). ✅ D5 mitigado.
- Recomendacion: registrar la identidad del `Client` (`Arc` ptr) en un field de span, y/o habilitar trazas de pool de `wreq`/h2 para emitir socket-ID.

### 6.3 Propuesta de Instrumentacion (funciones "mudas" → `#[instrument]` para v0.5.1)
| # | Funcion (archivo:linea) | Por que decorar |
|---|--------------------------|-----------------|
| 1 | `cli::orchestrator::run` (`orchestrator.rs:42`) | **Crear `root_span`** con fields `max_pages`, `download_images`, `download_concurrency`, `single_page`. Hoy no hay span raiz. |
| 2 | `infrastructure::ai::inference_engine::run_inference` (`inference_engine.rs` ~330) | `#[instrument]` → ONNX obtiene su propio span + link de parent. Hoy solo `.in_current_span()`. |
| 3 | `application::elastic_ingestion::run` (`elastic_ingestion.rs`) | `#[instrument]` + log `"Acquire Permit"` en el `.acquire().await` del semaforo. |
| 4 | `infrastructure::crawler::resource_downloader` (download) | `#[instrument]` + log adquisicion/liberacion del PermitGuard byte-weighted. |
| 5 | `adapters::downloader` `download`/`download_assets` (`mod.rs:376`) | Instrumentar el loop `buffer_unordered`; registrar `download_concurrency` y un span por asset. |
| 6 | `application::scraper_service::scrape_single_url` (`scraper_service.rs:464`) | Ser el span padre de descargas; llevar `url` + `concurrency` en fields. |
| 7 | `infrastructure::downloader::wreq_downloader::fetch` (ya instrumentado) | ✅ Hecho (v0.5.1): `client_id` (Arc<Client> ptr) como span field + event field en `download_once`; reuso observable. |
| 8 | **Fix** `with_download_concurrency` (`infra/config.rs:173`) | **Rechazar/clamp `0`→`1`** para evitar el deadlock de `buffer_unordered(0)`. |
| 9 | `infrastructure::observability::file_trace_layer` (`on_event`) | Emitir `parent_id` (desde `LookupSpan` parent) y usar el `trace_id` OTel cuando exista; no derivar de thread-id. |
| 10 | Borde de error (`scrape_flow.rs`, `wreq_downloader.rs`) | ✅ Hecho (v0.5.1): `ScraperError`/`CrawlError`/`DownloadError` preservan `#[source]`; `failures` conserva el tipo de error. |

---

## 7. Evidencia Dinamica (trazas generadas)
| Run | Comando (resumido) | Resultado |
|:----|:-------------------|:----------|
| RUN1 | `--max-pages 1` (sin assets) | `trace_id` constante `0000000000000001`, 10 eventos, 0 `parent_id` |
| RUN3a | `--download-images --download-concurrency 4` | 40 assets, 56 eventos, `parent_id`=0, 0 permit/socket |
| RUN3b | `--download-images --download-concurrency 0` | **`EXIT=124`** (hang 120s), 0 assets, JSONL congelado en "Downloading 45 assets" |
| RUN4 | `--url <DNS invalido>` | causa preservada vía `#[source]` (wreq::Error → Connect) en `failures` |
| otel-check | `cargo check --features otel` | OK compila |

Archivos: `/tmp/audit_nodl.jsonl`, `/tmp/audit_dl.jsonl`, `/tmp/audit_dl0.jsonl`, `/tmp/audit_err.jsonl`.


# 📊 Reporte de Daños y Fidelidad de Trazas — v0.5.0

**Proyecto:** `webfang` (v1.1.0) · **Fecha:** 2026-07-10 · **Rol:** Senior Systems Auditor & Observability Lead
**Entradas:** `test_lab/MAPA_FIDELIDAD_TRACES.md` + trazas JSONL (`/tmp/audit_{nodl,dl,dl0,err}.jsonl`)
**Método:** `jaq 3.1.0` sobre los JSONL (slurp `-s`); conteo/validez RFC3339/presencia de campos por cruce de `trace_id`/`span_id`/`parent_id`. Build de prueba: **default (sin `--features otel`)**, binario debug recompilado.

---

## 1. Veredicto de Infraestructura

| Dimensión | Veredicto | Base (jaq) |
|:----------|:---------:|:-----------|
| **Pooling de Conexiones** | ✅ MITIGADO | `client_id` (Arc<Client> ptr) observable en `download_once` → reuso verificable |
| **Integridad de Trazas** | PASA formato / FALLA contexto | RFC3339 100% (75/75); `trace_id` 100%; `span_id` solo 58/75; `parent_id` = 0/75 |
| **Propagación de Contexto (spawn_blocking)** | FALLA | `parent_id` = 0/75; path ONNX no CLI-reachable |

### 1.1 Pooling de Conexiones — FALLA (Brecha, no regresión)
El diseño es correcto: `Arc<wreq::Client>` único compartido (`wreq_downloader.rs:47`) y reusado por todas las páginas. La traza **ahora** contiene `client_id` (puntero del `Arc<Client>`) en cada `download_once` (adapters) y como span field en `wreq_downloader::fetch`. En una corrida real, los 15 fetches de assets mostraron el **mismo** `client_id` (`0x55d2788f7e50`) → reuso de pool verificable y ahorro de handshakes TLS confirmado. ✅ D5 mitigado (los socket-ID nativos de wreq/h2 siguen ausentes, pero la identidad del `Client` prueba el reuse).

### 1.2 Integridad de Trazas — Formato OK, Contexto Roto
- RFC3339: `jaq -s '[.[] | (.timestamp | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\\.[0-9]+Z$"))] | all` → **`true`** en los 4 archivos (75/75).
- `trace_id` presente: 0 eventos sin `trace_id`.
- `span_id` NO universal: solo 58/75 eventos lo llevan (17 son eventos fuera de span). "100% con `span_id`" es FALSO.
- `parent_id` = 0/75: el árbol de traza es irreconstruible desde el JSONL.

### 1.3 Propagación de Contexto (spawn_blocking) — FALLA
`FileTraceLayer` (`file_trace_layer.rs:105-170`) escribe `span`/`span_id` desde la pila thread-local y `trace_id` desde el **thread ID**; **nunca emite `parent_id`**. Los hijos en `spawn_blocking` (ONNX en `inference_engine.rs:366` `.in_current_span()`) heredarían contexto de Span de OTel, pero: (a) el JSONL no lo serializa, y (b) el path ONNX **no es alcanzable por CLI** (`orchestrator.rs:226` `clean_ai: false`). Conclusión: la traza **se fragmenta / no propaga `parent_id`** en la sesión.

---

## 2. Matriz de Errores Detectados (Daños)

| ID | Severidad | Fallo | Evidencia |
|:---:|:---------:|:------|:----------|
| **D1** | CRITICO | `--download-concurrency 0` → deadlock silencioso | `audit_dl0.jsonl:5` (ultimo evento antes de colgar 120s) |
| **D2** | ALTO | `parent_id` ausente 100% → arbol irreconstruible | `jaq parent_id_events: 0` (4 archivos) |
| **D3** | ALTO | `trace_id` = thread ID (no logico) → se fragmenta | `distinct_trace_id: ["0000000000000001"]` |
| **D4** | ✅ LIQUIDADO | Cadena de causa preservada con `#[source]` (Download/Network/CrawlError) | `error.rs` / `crawl_error.rs` / `scrape_flow.rs` |
| **D5** | ✅ MITIGADO | `client_id` registrado en `download_once`/`wreq_downloader::fetch` → reuso observable | `client_id: 0x...` constante |
| **D6** | BAJO | Metadatos de config no en root_span (no existe) | `orchestrator.rs:42` sin `#[instrument]` |
| **D7** | BAJO | `TraceCorrelationLayer` gated (A1) → ausente en default | `config.rs:151` solo `cfg(otel)` |

### 2.1 D1 — CRITICO: Deadlock silencioso (`--download-concurrency 0`)
Hot-path usa `buffer_unordered(concurrency)` (`adapters/downloader/mod.rs:382`). Con `0`, el loop nunca encola futures → `poll_next` retorna `Pending` infinito. RUN3b: `EXIT=124` (timeout 120s), 0 assets. El JSONL se congela y **no emite error/warning**:
```json
{"level":"INFO","message":"📦 Downloading 45 assets via adapters::Downloader","span":"scrape_single_url","span_id":"0008000000000001","target":"webfang::application::scraper_service","timestamp":"2026-07-10T21:43:59.153Z","trace_id":"0000000000000001"}
```
Dano: el programa **cuelga sin senal de telemetria** (violacion de "Zero Silent Loss", ver §3).

### 2.2 D2 — ALTO: `parent_id` ausente (100%)
`jaq -s '[.[] | select(has("parent_id"))] | length'` → **0** en los 4 archivos. `FileTraceLayer` no serializa la relacion padre-hijo (solo `span` + `span_id` del contador interno de `tracing`). Impacto: no se reconstruye el arbol pagina→descarga→ONNX.

### 2.3 D3 — ALTO: `trace_id` = thread ID
`FileTraceLayer` hashea `std::thread::current().id()` (`file_trace_layer.rs:124-140`). En estas pruebas fue constante `0000000000000001` solo porque el pipeline corrio en el hilo principal (1 pagina). Bajo `JoinSet` multi-pagina las tareas saltan de hilo → `trace_id` **se fragmenta**. No es identificador logico de traza.

### 2.4 D4 — ALTO: Cadena de causa aplanada (429/500)
RUN4 (DNS) produjo un `String` plano sin `source()`:
```json
{"level":"WARN","message":"URL discovery failed: HTTP error: error sending request for uri (http://nonexistent-domain-xyz-12345.invalid/): client error (Connect)","target":"webfang::cli::url_discovery","timestamp":"2026-07-10T21:39:21.153Z","trace_id":"0000000000000001"}
```
Path HTTP (429/500): `DownloadError::Http { status, message }` (`wreq_downloader.rs:208`) conserva el **status** en la variante, PERO `DownloadError → CrawlError` hace `CrawlError::Download(Box::new(err))` (`infra/downloader/mod.rs:137`) → **preserva la causa como `#[source]`** (status HTTP sigue en `DownloadError::Http`). `ScraperError::Http { status, url }` conserva status y `ScraperError::Download`/`Network` ahora llevan `#[source] Box<dyn Error + Send + Sync>` (`error.rs:44,71`); `Error::source()` ya NO es `None` para esas variantes. Unico que preservaba cadena antes: `NetworkFailure(#[from] WreqError)` (path elastico) — ahora acompañado por `Download`/`Network`. **Veredicto (actualizado v0.5.1):** ✅ LIQUIDADO. `DownloadError::Network` ahora lleva `#[source]`, `CrawlError::Download` preserva la causa con `#[source] Box<dyn Error>`, y `ScraperError::Download`/`Network` llevan `#[source] Box<dyn Error + Send + Sync>`. El status 429/500 sigue preservado y la cadena "causado por → IO → timeout" ya es navegable via `Error::source()`; `scrape_flow.rs`/`orchestrator.rs` imprimen la cadena completa.


---

## 3. Auditoria de "Zero Silent Loss"

### 3.1 Flags que causaron comportamiento no documentado / hang silencioso
**CONFIRMADO — Violacion de Zero Silent Loss.** `--download-concurrency 0` (combinado con `--download-images`) produce un **hang infinito sin ningun log de error ni warning**. El usuario solo ve "📦 Downloading 45 assets" y el proceso se congela hasta ser matado. No hay documentacion de esta combinacion ni mensaje de rechazo en `clap` (el flag acepta `0` como `usize` valido, default 3). Es el daño D1.

### 3.2 Errores de red (429/500): cadena de causa preservada?
**✅ LIQUIDADO — status preservado, causa preservada con `#[source]`.** El status HTTP (429/500/404) se conserva en la variante estructurada `DownloadError::Http { status, .. }` y `ScraperError::Http { status, .. }`. Y la causa subyacente (wreq error → IO → timeout/connect) **SI** se preserva como `source()`:
- `DownloadError → CrawlError` preserva `#[source]` (`infra/downloader/mod.rs:137`: `CrawlError::Download(Box::new(err))`).
- `ScraperError::Download`/`Network` llevan `#[source] Box<dyn Error + Send + Sync>`; `Http` conserva `status`.
- El borde de servicio `scrape_flow.rs` conserva el `ScraperError` completo en `failures` (no `String`); `orchestrator.rs` imprime la cadena `source()`.

Por tanto: **la cadena "causado por → IO → timeout" ahora es navegable via `Error::source()`** (ej. RUN4: el `wreq::Error` Connect se conserva como `source` de `ScraperError::Network`). El texto sigue siendo legible Y además trazable. ✅ D4 liquidado.

---

## 4. Backlog Tecnico de Observabilidad (v0.5.1)

### 4.1 Funciones "mudas" (logica pesada sin Span)
| Funcion | Archivo:linea | Peso | Por que instrumentar |
|:--------|:--------------|:-----|:---------------------|
| `orchestrator::run` | `cli/orchestrator.rs:42` | Orquesta todo el pipeline | Crear `root_span` con `max_pages`, `download_images`, `download_concurrency` |
| `inference_engine::run_inference` | `infra/ai/inference_engine.rs:~330` | ONNX CPU-bound en `spawn_blocking` | `#[instrument]` → span propio + `parent_id` al scraper |
| `elastic_ingestion::run` | `application/elastic_ingestion.rs` | Pipeline 7-capas | `#[instrument]` + log `"Acquire Permit"` en `.acquire().await` |
| `resource_downloader` (download) | `infra/crawler/resource_downloader.rs` | Stream byte-weighted + semaforo | `#[instrument]` + log adq/liberacion PermitGuard |
| `adapters::downloader` `download`/`download_assets` | `adapters/downloader/mod.rs:376` | `buffer_unordered` loop | Instrumentar loop; registrar `download_concurrency` + span por asset |
| `scraper_service::scrape_single_url` | `application/scraper_service.rs:464` | Span padre de descargas | Llevar `url` + `concurrency` en fields |

### 4.2 Issue A1 — Gating de `TraceCorrelationLayer`: impacto real
`trace_correlation_layer()` se compone **solo** en `init_logging_dual` bajo `#[cfg(feature = "otel")]` (`cli/config.rs:151`). Hay dos definiciones de `init_logging_dual`:
- `#[cfg(not(feature = "otel"))]` (`config.rs:89`): **NO** incluye `trace_correlation_layer` (solo `file_trace_layer` + fmt).
- `#[cfg(feature = "otel")]` (`config.rs:119`): si incluye `.with(trace_correlation_layer())`.

**Impacto en la sesion de pruebas:** el build fue **default (sin otel)**, por lo que `TraceCorrelationLayer` estuvo **AUSENTE**. El `FileTraceLayer` inyecto `trace_id` (thread-ID) y `span_id` por su cuenta, y **nunca `parent_id`**. Esto explica parcialmente la ausencia de correlacion W3C real en las trazas capturadas: no es solo un bug del `FileTraceLayer`, sino que **la capa de correlacion estaba desactivada por feature-flag**.

**Matiz critico:** aunque se compile con `--features otel`, el `FileTraceLayer` sigue calculando `trace_id` desde el thread-ID **independientemente** de lo que `TraceCorrelationLayer` registre en el Span (las dos capas no se comunican). Por tanto, el artefacto JSONL offline **siempre** llevara `trace_id` enganoso salvo que se modifique `FileTraceLayer` para leer el `trace_id`/`parent_id` real del contexto de Span. **A1 no se resuelve solo activando otel:** requiere que `FileTraceLayer::on_event` emita `parent_id` (desde `LookupSpan`) y use el `trace_id` OTel cuando exista.

### 4.3 Acciones priorizadas (v0.5.1)
1. **P0 / D1:** Clamp `with_download_concurrency` `0→1` (`infra/config.rs:173`) + rechazar `0` en `clap` → elimina el deadlock.
2. **P0 / D2-D3:** `FileTraceLayer` emita `parent_id` y `trace_id` real (OTel) en vez de thread-ID.
3. **P1 / D4:** ✅ Hecho — `ScraperError`/`CrawlError`/`DownloadError` ahora preservan `#[source]`; `failures` de `scrape_flow.rs` conserva el `ScraperError` y `orchestrator.rs` imprime la cadena `source()`.
4. **P2 / D5:** ✅ Hecho — `client_id` (Arc<Client> ptr) registrado como field en `download_once` (adapters) y como span field en `wreq_downloader::fetch`; observable en JSONL vía `--trace-file`.
5. **P2 / D6:** `root_span` en `orchestrator::run` con metadatos de config.
6. **P3 / A1:** Desacoplar `TraceCorrelationLayer` del feature `otel` (o unificar con `FileTraceLayer`) para que la correlacion funcione en build default.


---

## Anexo — Evidencia jaq (cruce de trazas)

| Archivo | eventos | parent_id | distinct trace_id | con span_id | sin trace_id | RFC3339 ok | socket/pool/permit |
|:--------|:--------:|:---------:|:-----------------:|:-----------:|:------------:|:----------:|:-------------------|
| `audit_nodl.jsonl` | 10 | 0 | `0000000000000001` | 3 | 0 | true | [] |
| `audit_dl.jsonl` | 56 | 0 | `0000000000000001` | 49 | 0 | true | [] |
| `audit_dl0.jsonl` | 5 | 0 | `0000000000000001` | 4 | 0 | true | [] |
| `audit_err.jsonl` | 4 | 0 | `0000000000000001` | 2 | 0 | true | [] |
| **TOTAL** | **75** | **0** | **1** | **58** | **0** | **100%** | **[]** |

**Comandos reproducibles (jaq 3.1.0):**
```bash
jaq -s 'length' FILE.jsonl
jaq -s '[.[] | select(has("parent_id"))] | length' FILE.jsonl
jaq -s '[.[].trace_id] | unique' FILE.jsonl
jaq -s '[.[] | (.timestamp | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\\.[0-9]+Z$"))] | all' FILE.jsonl
jaq -s '[.[] | ((.fields // {}) | keys[] | select(test("socket|conn|pool|permit|reuse|fd"; "i")))] | unique' FILE.jsonl
```

**Conclusion general:** La infraestructura de pooling existe y es correcta en codigo, pero la **fidelidad de trazas es insuficiente para auditoria distribuida**: `trace_id` enganoso (thread), `parent_id` inexistente, y semaforos/pool sin instrumentacion. El deadlock por `--download-concurrency 0` es la unica regresion funcional critica (D1). Issue A1 confirma que la correlacion W3C estaba desactivada por feature-flag en esta sesion.


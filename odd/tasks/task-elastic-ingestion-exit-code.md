# Task — Exit code de la ingesta elástica: 69 de la matriz, no 74 hardcodeado

**Issue:** #1949
**Branch:** `fix/elastic-ingestion-exit-code`
**Base:** `main` @ 85cf3e62 (v2.8.0)

## Problema

La matriz de clasificación (fila 26, `docs/error-classification-matrix.md:92`)
declara el contrato: `Ingestion (Elastic) | TransientBackoff | 69 | Retry Yes`.

El camino real lo ignora: `cli/batch_flow.rs:494-496` mapea CUALQUIER error de
`run_elastic_ingestion` directo a `CliExit::IoError` (74), sin pasar por
`ScraperError::classify()` ni por los helpers canónicos
(`default_exit_code_for_class` / `cli_exit_for_class`) adoptados en #891.

El operador lee 74 y concluye "falló I/O"; la señal verdadera es "backend de
ingesta no disponible, reintentá" (69). Es el FINDING registrado en el task doc
de #1943: el panic quedó visible pero con etiqueta de clase equivocada.

Además `matrix:142` sigue citando #839 (cerrado 2026-08-23 vía #891) como
tracker de la divergencia — referencia podrida que este fix también corrige.

## Contrato (no inventar nada)

- `cli/error.rs:187` ya mapea `TransientRetriable | TransientBackoff →
  EXIT_UNAVAILABLE` (69), con unit test en `:627`. La maquinaria canónica
  existe; el fix es enrutar por ella, no crear un tercer sistema.
- `run_elastic_ingestion(...) -> Result<(), ScraperError>`
  (`cli/elastic.rs`) — sus errores ya son `ScraperError`, incluyendo los de
  `join_failure` (panic / cancelación via `ScraperError::ingestion(...)`).

## Tareas

- [x] T1 — Test RED: la falla transitoria de ingesta por batch/`--output-vectors`
      sale con exit 69, no 74 (behavioral, `batch_export_exit_code_test.rs`).
      RED observado: `left: 74, right: 69` con
      `Error: Falló la ingesta de vectores: error de red: … client error (Connect)`.
      El fixture: mock one-shot 200 (`.up_to_n_times(1)` — `.expect()` solo
      verifica en el drop, NO corta el matching) sirve el scrape; el
      re-download de la ingesta cae al segundo mock, un 301 a `dead.invalid`
      (RFC 6761: nunca resuelve) → error de transporte → `Network` →
      `TransientRetriable` → 69. Gate `#[cfg(feature = "persistence")]`
      porque `--elastic` sin la feature sale 78 en el preflight (#695).
- [x] T2 — GREEN: el call site de `batch_flow` enruta por classify/helpers
      canónicos; cero mapeos inline de `CliExit` para ingesta.
      `run_batch_elastic` ahora hace
      `.map_err(|e| crate::cli::error::ingestion_exit_for(&e))` — la decisión
      vive en el helper canónico `cli::error::ingestion_exit_for`
      (config 78 → Io permanente 74 → default de clase vía `cli_exit_for_class`:
      transitorio 69 / InternalFatal 3 → terminal 65).
- [x] T3 — Sub-casos pineados (tests `ingestion_*` en `cli/error.rs`):

  | Sub-caso | Variante construida hoy | Clase | Exit | Test |
  | --- | --- | --- | --- | --- |
  | Backend transitorio (re-download) | `Network(Box<wreq::Error>)` | `TransientRetriable` | **69** | `ingestion_transient_network_failure_maps_to_unavailable_69` + behavioral |
  | Backend transitorio (timeout) | `GlobalTimeout` | `TransientBackoff` | **69** | `ingestion_transient_backoff_timeout_maps_to_unavailable_69` |
  | Tarea en pánico | `Ingestion("…entró en pánico…")` (`join_failure`) | `InternalFatal` | **3** | `ingestion_panicked_task_maps_to_scraper_failure_3` |
  | Tarea cancelada | `Ingestion("…cancelada…")` (`join_failure`) | `InternalFatal` | **3** | `ingestion_cancelled_task_maps_to_scraper_failure_3` |
  | Persistencia del repo | `Persistence(String)` | `InternalFatal` | **3** | `ingestion_persistence_failure_maps_to_scraper_failure_3` |
  | SSRF entry refusal del downloader | `Config(String)` | PermanentFatal → override | **78** | `ingestion_ssrf_config_refusal_maps_to_config_error_78` |
  | Io sink permanente/transitorio | `Io(io::Error)` | por kind | **74 / 69** | `ingestion_permanent_io_maps_to_io_error_74_and_transient_keeps_69` |
  | Re-download oversized | `PayloadTooLarge` | PermanentFatal → terminal | **65** | `ingestion_payload_too_large_maps_to_data_format_error_65` |

  La división es coherente con la fila 26: la fila declara el contrato del
  fallo TRANSITORIO del backend (69, reintentar); pánico/cancelación son
  defectos de pérdida de datos (`InternalFatal` → 3), no un outage
  reintentable — documentado bajo la tabla de Family 4 en la matriz, no
  re-convertido a 69 por la fuerza.
- [x] T4 — `matrix:142` ya no cita #839 para Ingestion (la divergencia
      Ingestion queda resuelta a efectos de exit-code: fila 26 = 69 vía el
      camino canónico; `Ingestion`→`InternalFatal`→3 es el split pineado). La
      fila 26 documenta el split bajo la tabla Family 4. Las otras cuatro
      divergencias siguen abiertas para su propia auditoría.
- [ ] T5 — Verificación completa: `bash scripts/ci_fast_gate.sh` (check, clippy
      estricto, fmt --check, rustdoc, machete, duplicación).

## Hallazgo fuera de superficie (escalado al parent — RESUELTO por el parent)

`run_elastic_ingestion` tiene **dos** call sites (codedb, índice del worktree):
`cli/batch_flow.rs:494` (corregido por el worker) y **`cli/orchestrator.rs:259`** — el
flujo single-run mapeaba el error de ingesta al MISMO `CliExit::IoError`
hardcodeado (74). `orchestrator.rs` NO estaba en las superficies de edición
autorizadas, así que el worker lo reportó sin tocarlo. El parent extendió el
fix (misma corrección de una línea: `return ingestion_exit_for(&e)`), porque
es el mismo bug bajo el mismo contrato de la fila 26 y el helper ya estaba
pineado. La nota de frontera en la matriz se reescribió para reflejar ambos
call sites alineados.

## Fuera de alcance

- Las otras cuatro divergencias classify-vs-matriz de `matrix:142`
  (ExtractionFailed, Conversion, Readability/Extraction, Middleware): auditoría
  propia, issue propio.
- `Persistence(String)` con `#[source]` tipado: decisión congelada #4.
- Cambiar mensajes en español existentes salvo lo que el reenrutado exija.
- **El exit de la INICIALIZACIÓN** (`cli/elastic.rs:255`, `build_elastic_ingestion`:
  `IoError` 74 con "no se pudo inicializar…"): fase de setup, no corrida — la
  fila 26 no declara su contrato (¿config 78? ¿io 74?) y no hay evidencia para
  decidirlo acá. Auditoría propia si el maintainer quiere clasificarlo.

## Evidencia

| Commit | Verificación |
| --- | --- |
| `91b5067d` fix(cli) | RED: `cargo nextest run --features persistence --test batch_export_exit_code_test` → `left: 74, right: 69` (3 intentos, determinístico). GREEN: idem → 16/16 PASS, snapshot `batch_transient_ingestion_failure_stderr` aceptado tras revisión. Pins unit: `cargo nextest run --features persistence -p webfang_core --lib -E 'test(cli::error::tests::ingestion_)'` → 8/8 PASS. |
| `91b5067d` (extensión parent incluida) | T5 COMPLETO, 8/8 verde: check --all-targets --all-features PASS (26s); clippy estricto (gate exacto de CI) PASS 0 warnings (43s); fmt --check PASS; behavioral 16/16; pins 8/8; suite full -p webfang_core --features persistence 3716/3716, 0 FAIL; 0 *.snap.new; `ci_fast_gate.sh` GREEN (PASS=26 FAIL=0 SKIP=1). Verificación ejecutada por el parent tras dos stalls del subagente verify (watchdog de 4 min del host sobre bash silencioso; cosechada de /tmp/t5-*.log + corrida directa). |

### Resultados T5

- `cargo check --all-targets --all-features` → PASS
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` → PASS, 0 warnings
- `cargo fmt --all -- --check` → PASS
- `cargo nextest run --test batch_export_exit_code_test` (persistence) → 16/16 PASS
- `cargo nextest run -E 'test(cli::error::tests::ingestion_)'` (persistence) → 8/8 PASS
- `cargo nextest run -p webfang_core --features persistence` → 3716/3716, 0 FAIL
- `find crates -name "*.snap.new" | wc -l` → 0
- `bash scripts/ci_fast_gate.sh` → GREEN

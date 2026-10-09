# Task — Supervisión de JoinError en ingesta elástica y probe de autoscale

**Issue:** #1941
**Branch:** `fix/elastic-joinerror-supervision`
**Base:** `main` @ 454f2555

## Problema

Dos tareas spawneadas pueden fallar de forma invisible:

1. `crates/webfang_core/src/cli/elastic.rs:73-78` — el drenaje final del `JoinSet`
   de ingesta matchea solo `Ok(Err(e))`. Un `Err(JoinError)` (panic o cancelación)
   cae fuera del `if let` y se descarta: la función retorna `Ok(())` y el proceso
   sale con 0 pese a que una task de ingesta murió. El loop de throttling del mismo
   archivo (`:50-61`) sí maneja ese caso → dos políticas distintas en un mismo
   archivo, contrarias a la decisión congelada D2 (fail-fast).

2. `crates/webfang_core/src/application/crawler/engine.rs:534` — el probe de
   autoscale (`with_autoscale`) hace `tokio::spawn` y descarta el `JoinHandle` de
   inmediato. Un panic en ese loop sería invisible. El patrón correcto ya existe
   200 líneas más abajo: `engine.rs:730` guarda el signal handler en un campo y lo
   une en shutdown.

## Contrato de referencia (a replicar, no a inventar)

`application/crawler/crawl_task.rs:29-78` es el discriminador canónico del repo:

- `Ok(Ok(()))` → éxito.
- `Ok(Err(e))` → error de aplicación, se clasifica y se cuenta.
- `Err(join_err)` → `handle_join_error`: `is_cancelled()` (debug, no cuenta, es
  señal de control de #509) vs panic (`warn!` + contador + categoría
  `CrawlErrorCategory::Panic`).

La ruta elástica no tiene equivalente. Este trabajo lo agrega.

## Tareas

- [x] T1 — Test RED: un `JoinSet` con una task que panea, drenado con la función
      de producción, debe producir `Err` (o al menos un log), nunca `Ok(())`.
- [x] T2 — GREEN: `cli/elastic.rs` drena distinguiendo los tres desenlaces, con la
      misma simetría que el loop de throttling y fail-fast en panic.
- [x] T3 — Test RED + GREEN: el probe de autoscale deja de descartar su
      `JoinHandle`; se registra en el `Engine` y se une/abortea en shutdown.
- [x] T4 — Verificación: `cargo check --all-targets --all-features`, clippy con el
      comando exacto de CI, `cargo fmt --all -- --check`, `cargo nextest run` del
      módulo afectado.

## Desviación técnica (documentada, no escondida)

El precedente de `batch/processor.rs:243` (la task devuelve `(url, result)`) **no
alcanza** para el caso que motiva el issue: una task que panea nunca retorna su
propio output, así que el `JoinSet` devuelve un `JoinError` pelado y la URL no
viaja en el resultado. La atribución se resuelve desde el supervisor: se registra
`task id → url` al spawnear (`AbortHandle::id()`) y se resuelve al reapear
(`JoinError::id()`), lo que cubre panic **y** cancelación. Un test dedicado
(`join_failure_names_the_url_of_the_dead_task_not_a_sibling`) prueba que la URL
reportada es la de la task muerta y no la de una hermana viva.

## Fuera de alcance (decisiones de otro dueño)

- **`Persistence(String)` → `#[source]` tipado.** Es la decisión congelada #4
  documentada en `infrastructure/persistence/sqlite.rs:311-318`. No se toca acá.
- **`disallowed-methods` para `tokio::spawn` en `clippy.toml`.** Verificado que
  la lint matchea por `DefId` (aliases no la evaden) y que exige una entrada por
  path (`tokio::spawn` y `tokio::task::spawn_blocking` son DefIds distintos).
  El costo real es ~41 sitios en `src` + ~26 en tests, porque el lint es
  warn-by-default y `--all-targets` lo escala con `-D warnings`. Es un work unit
  propio, con su política de exención para tests.
- **`MID_JSONL_LINE`:** el constante existe en `cli/crash_points.rs:37` sin call
  site. Agregarlo habilita repro E2E del panic vía `WEBFANG_CRASH_AT`, pero es un
  cambio de contrato del harness de crash, no de este fix.

## Hallazgos de la verificación independiente

- **FINDING cerrado en este PR:** `log_scrape_error` con `url = ""` deja al
  operador sin saber qué URL perdió sus vectores. Cerrado con el mapa
  `task id → url` descrito arriba. `correlation_id: None` sigue: la firma de
  `run_elastic_ingestion` está pineada por `cli/orchestrator.rs` y
  `cli/batch_flow.rs`, fuera de la superficie de este trabajo.
- **FINDING preexistente, no introducido aquí:** la salida de la ingesta elástica
  no pasa por `classify()`. `ScraperError::Ingestion` clasifica
  `InternalFatal` (exit 3) según `error.rs:499`, la matriz dice 69, y el camino
  real hardcodea `CliExit::IoError` (74). Divergencia ya rastreada en #839. Este
  fix hace que un panic de ingesta sea visible con exit no-cero, pero con una
  etiqueta de clase equivocada.
- Sin test del workspace dependía del comportamiento viejo (exit 0).
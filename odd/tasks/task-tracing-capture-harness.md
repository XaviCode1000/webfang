# Task — Extraer el harness de captura de logs de `tracing`

**Issue:** #1944
**Branch:** `refactor/tracing-capture-harness`
**Base:** `origin/main` @ 7024e7dc

## Problema

Seis módulos `#[cfg(test)]` de `webfang_core` definen su propia copia del
mismo harness de captura de eventos de `tracing`: un `MakeWriter` sobre un
buffer compartido detrás de un mutex. Cinco lo llaman `SharedWriter` y uno
(`crawl_task.rs`) `LogBuffer`; dos ya divergieron —`scraper_service.rs`
desenvenena con `.lock().unwrap()` y `error_logging.rs` mapea el error.

Esto no es hipotético: el ratchet de duplicación bloqueó #1941 porque un test
nuevo copió el patrón por tercera vez (9349 > 9340). La salida de ese PR fue
borrar la copia nueva y dejar la deuda intacta.

## Call sites

| Archivo | Línea del `MakeWriter` |
|---|---|
| `application/crawler/crawl_task.rs` | 751 |
| `application/crawler/sitemap_discovery.rs` | 945 |
| `application/pipeline/executor.rs` | 96 |
| `application/scraper_service.rs` | 1110 |
| `infrastructure/crawler/sitemap_parser.rs` | 1194 |
| `infrastructure/observability/error_logging.rs` | 84 |

## Decisión de ubicación

El helper vive en `webfang_core` detrás de `#[cfg(test)]`, no en
`webfang_test_utils`. Razón: los seis call sites son unit tests inline del
mismo crate, y `#[cfg(test)]` los alcanza sin tocar `Cargo.toml` —agregar
`tracing`/`tracing-subscriber` a `webfang_test_utils` es una decisión de
dependencias que AGENTS.md pone en "ask first" y pertenece a otro cambio. Si
`webfang_ai` o `webfang_mcp` necesitan el harness después, la mudanza a
`webfang_test_utils` es el paso siguiente natural y ya estará aislada en un
módulo.

`webfang_core` no impone `#![deny(missing_docs)]` (ese deny vive en
`webfang_test_utils`), pero el helper igual lleva doc comments completos: es
la convención de los seis call sites y es lo que va a exigir el crate si el
harness migra. Ojo con el deny de `clippy::disallowed_methods`, que en core es
*ungated* a propósito: cubre también los módulos `#[cfg(test)]`.

## Tareas

- [ ] T1 — Helper compartido bajo `#[cfg(test)]` con una sola definición del
      `MakeWriter` + guard de escritura, toma de texto y subscriber.
- [ ] T2 — Los seis call sites pasan a usar el helper; sus asserts no cambian
      de semántica.
- [ ] T3 — `cargo nextest run` verde en los seis módulos.
- [ ] T4 — `bash scripts/check_duplication.sh` por debajo de 9340, y
      `scripts/quality-baselines.json` **bajado** al valor medido. Ratchet
      descendente: subirlo es un bug, no una negociación.
- [ ] T5 — clippy con el comando exacto de CI, `cargo fmt --all -- --check`,
      `cargo doc` con `RUSTDOCFLAGS=-D warnings`.

## Fuera de alcance

- Mover el harness a `webfang_test_utils` (requiere cambiar `Cargo.toml`).
- Cambiar la semántica de cualquier assert.
- Cualquier cambio de producción: todo vive bajo `#[cfg(test)]`.

## Evidencia

| Commit | Verificación |
| --- | --- |
| (pendiente) | (pendiente) |
# ADR-0004 — suite wiremock del PR-8 (gate técnico del paso 6)

- **Rama:** `test/adr0004-wiremock-suite`
- **Worktree:** `~/Projects/Rust/webfang-worktrees/adr0004-wiremock-suite`
- **Driver:** `docs/adr/0004-onnx-local-removal.md`, sección *Criterios de aceptación de M2* y *Condición de revisión* §1
- **Estado:** en curso

## Por qué esta tarea

El paso 6 de ADR-0004 (borrado total de ONNX) está bloqueado por el **criterio técnico**, que exige verde la suite wiremock del PR-8. De los ocho escenarios que el ADR enumera, solo uno tiene cobertura hoy.

## Estado medido en `main` (7024e7dc, post PR #1940)

Escenario exigido por el ADR | Cobertura | Nota
---|---|---
401 con y sin credencial | ✅ | `auth_source_none_test.rs`; cubierto por B1 (`AuthSource::None`)
429 con `Retry-After` en segundos | ❌ | —
429 con `Retry-After` en fecha HTTP | ❌ | —
5xx | ❌ | —
timeout | ❌ | —
respuesta malformada | ❌ | —
orden de batch | ❌ | —
truncado de stream | ❌ | —
DNS | ❌ | —

**Corrección de una premisa propia:** `llm_ssrf_hatch_test.rs` **no** es esta suite — es la regresión de SSRF de #1615, sin wiremock. El harness real está en `crates/webfang_core/tests/auth_source_none_test.rs` (`config_for`, `embedding_mock`, `recorded_authorization`), ampliado por el PR #1940.

## Fuera de alcance

- **B2** (`NetworkPolicy` / `allow_cidrs`): sigue con 0 hits post-#1940. Es el otro bloqueante duro del gate, pero es diseño de tipo nuevo y va en tarea aparte.
- **Registro reproducible contra Ollama y vLLM reales**: exige los servidores; el ADR dice que "probado a mano sin registro no cuenta". No lo cierra un agente.
- **Paso 6**: bloqueado además por calendario (2 releases minor tras el paso 5), con o sin esta tarea.

## Tareas

| # | Tarea | Estado |
|---|---|---|
| 1 | Mapear el harness wiremock existente y la superficie de error/retry de `RemoteEmbeddingAdapter` | ✅ hecho |
| 2 | 429 + `Retry-After` en segundos y en fecha HTTP | ✅ hecho |
| 3 | Clasificación 5xx y agotamiento de reintentos | ✅ hecho |
| 4 | Timeout | ✅ ya cubierto (fila unitaria preexistente, seam `with_http`) |
| 5 | Respuesta malformada | ✅ hecho |
| 6 | Orden de batch | ✅ hecho |
| 7 | Truncado de stream | ✅ cerrado como mapeo de error de lectura + enmienda del ADR |
| 8 | DNS | ✅ hecho (TLD reservado, 0.01s, no flaky) |
| 9 | Verde + fast gate + commit + PR | ✅ commit `ba1870c7` — push/PR pendientes de decisión |

## Hallazgo de producción corregido en el mismo PR

`retry_after_secs` (`remote_embedding.rs:502-509` en la base) parseaba solo `u64`: **`Retry-After` en fecha HTTP se descartaba en silencio** y caía a backoff exponencial puro. Corregido con `retry_after_delay(header, now)` — función pura con `now` inyectable, ambas formas de RFC 9110 §10.2.3, fecha pasada clampeada a `Duration::ZERO`, y descarte explícito vía `warn!` estructurado. Sin cambio de manifiesto: `chrono` ya era dep directa.

## Evidencia de commits

- `ba1870c7` — `fix(http): honor Retry-After in both RFC 9110 forms on the embedding adapter` (4 files, +728/-20)
- Verificación: 22 tests (8 filas nuevas + 14 preexistentes) verdes; `ci_fast_gate.sh` GREEN (PASS=26 FAIL=0); duplicación 9334 vs baseline 9340; rustdoc sin links huérfanos.
- Evidencia de mutación: con el parser mutilado, 7 de 8 filas siguen verdes — solo la fila de fecha futura falla. Esa fila es la regresión discriminante del bug.

## Incidencia operativa registrada

El primer worktree se creó con directorio `adr0004-wiremock-suite` (sin el prefijo `test-`), violando la convención `/` → `-`. El proxy de identidad reportó MISMATCH antes del primer commit y el protocolo de excepción no aplicaba (sin expectativa anclada externamente: sin PR, el nombre de la rama existe solo dentro del worktree). Resolución: se preservaron los cambios en `/tmp`, `git worktree remove --force`, re-`add` con el directorio `test-adr0004-wiremock-suite` adjuntando la MISMA rama, bootstrap completo, build state renombrado (mi propio cache, nunca seeds/quarantine), identidad verificada OK, y recién entonces el commit.

## Nota de linked issue

#1946 creada (`fix: el adapter de embeddings descarta Retry-After en fecha HTTP (RFC 9110) y reintenta antes de lo pedido`, form `bug_report.yml`, label `type:bug`, resultado `confirmed`). Pendiente: `status:approved` (label protegida — la agrega el maintainer, nunca self-approve), luego push y PR.
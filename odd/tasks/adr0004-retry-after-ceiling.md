# ADR-0004 #1956 — techo de Retry-After + aserción temporal robusta

- **Issue:** #1956 `chore: cerrar los hallazgos R3 del review de #1948`
- **Rama:** `fix/adr0004-retry-after-ceiling`
- **Worktree:** `~/Projects/Rust/webfang-worktrees/fix-adr0004-retry-after-ceiling`
- **Base:** `907ceaea` (PR #1948 ya mergeado)
- **Estado:** en curso

## Por qué

El review reliability de #1948 (aprobado, autoridad quemada) dejó tres findings informativos. Evaluados leyendo el código:

| Finding | Veredicto | Acción |
|---|---|---|
| `R3-overflow-saturation` (`:198`) | **Real, subestimado** — `Retry-After` hostil → sleep de ~584M años | Clampear con techo |
| `R3-integration-timing-flakiness` (`test:194`) | Real pero bajo — cota `elapsed < 10s` frágil en CI cargado | Ensanchar margen |
| `R3-clock-race` (`:448`) | **Benigno** — `Utc::now()` se lee una vez en el momento de la decisión; no es TOCTOU | Descartado, sin cambios |

## Decisiones de diseño (tomadas por el orquestador, reversible en una línea)

1. **Techo `RETRY_AFTER_MAX_MS = 60_000`.** Los 429 legítimos de APIs de embeddings piden ≤60s; 4 reintentos × 60s ≈ 4 min de stall, tolerable. Cuando una cabecera excede el techo → clampea **con `warn!` estructurado**, consistente con el principio "nunca silencioso" del propio fix de #1948.
2. **Aserción `elapsed < 10s` → 30s.** El poder discriminante está en el orden de magnitud (~1s vs 32 años), no en el techo; ensanchar no pierde señal y gana headroom de CI. La aserción inferior `>= 900ms` (prueba de sleep real, no hot loop) se preserva.

## Tareas

| # | Tarea | Estado |
|---|---|---|
| 1 | `RETRY_AFTER_MAX_MS` + clampeo en `retry_after_delay` con warn | pendiente |
| 2 | Tests: `Retry-After: u64::MAX` segundos y fecha HTTP año 9999 | pendiente |
| 3 | Ensanchar aserción `elapsed` del integration test | pendiente |
| 4 | Suite + gate + commit + PR | pendiente |

## Fuera de alcance

- No se toca `feat/b2-network-policy` (tiene dueño: otra sesión).
- No se toca el ADR-0004: este techo no cambia ningún criterio de aceptación de M2.
- No `CHANGELOG.md` (lo escribe release-plz).

## Evidencia de commits

`6509bf41` — `fix(http): bound a server-requested Retry-After so one header cannot hang the adapter` (2 files, +179/-15)

Verificación: 32 filas del adapter verdes (29 previas + 3 nuevas), 22 tests de integración verdes, clippy strict gate limpio, `cargo fmt --all -- --check` limpio, duplicación 9250 == baseline, `ci_fast_gate.sh` GREEN sobre la suite completa (4033 tests, PASS=26 FAIL=0).

Corrección aplicada sobre el techo: mi spec (60s) hacía insatisfacible el aserto preexistente `"120"` de #1948. El writer lo reorientó para que fije el clampeo, y agregó `retry_after_under_the_ceiling_is_honored_exactly` como contrapeso que prueba que el techo no recorta operación normal (frontera inclusiva en exactamente 60s).
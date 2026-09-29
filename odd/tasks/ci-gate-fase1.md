# ci-gate-fase1 — Decidir `code-quality` al gate con datos + break-glass

## Objective

Cerrar la auditoría de required checks sin tocar los 3 requeridos: medir `code-quality` (machete + duplicación) con serie histórica en main + muestra de PRs, y dejar documentado el break-glass de `enforce_admins`.

## Problem

`code-quality` corre tras `clippy` pero no bloquea (`ci.yml:748` no lo lista en `needs:` del gate). Subirlo a ojo es riesgo: p95 cerca del timeout de 10 min (`ci.yml:393`) o flakiness de instalación lo volverían un bloqueador ruidoso.

## Why

Acuerdo con el auditor externo: mantener `CI Gate` + `Validate PR metadata` + `cargo-mutants (PR diff)`, `strict + enforce_admins` en true, `feature-matrix` y `test-ai` advisory. Lo único abierto es `code-quality`, y solo con datos.

## Scope

In scope (Fase 1, read-only + 1 doc):

- Serie main: 20 runs `success`/`failure` de `ci.yml`, p95 + ok% de `Code quality`.
- Muestra PRs: ≥15 ejecuciones reales (`success`/`failure`, `skipped` aparte).
- Clasificación de fallos: deuda real vs flakiness (re-run mismo SHA).
- Doc break-glass `enforce_admins` (backup JSON en issue, open/close sin tocar `strict` ni checks).

Out of scope (Fase 2, solo si los datos dan):

- Agregar `code-quality` a `needs:` + loop narrow-scope (`ci.yml:786`) + pin `cargo-machete`.
- Los 3 casos de prueba (docs-only, código, fallo forzado).

## Tasks

- [ ] T1-serie-main — 20 runs success/failure, tabla ok/fail/excl + p95(min), veredicto por job.
- [ ] T2-muestra-prs — ≥15 ejecuciones reales de code-quality en `pull_request`, skipped aparte.
- [ ] T3-clasificar — cada failure: deuda real (machete/duplicación del diff) o flakiness (timeout, install, re-run).
- [ ] T4-breakglass — doc con backup + DELETE/POST `enforce_admins` + restauración verificada por lectura.
- [ ] T5-veredicto — ≥95% con ≥15 reales + p95 lejos de 10 min + 0 flakiness abierto ⇒ proponer Fase 2; si no, queda advisory.

## Authorized scope

Lectura + `gh api` GET únicamente. Ningún cambio en `.github/`, `crates/`, ni branch protection. El único write permitido es el doc de break-glass (ubicación por definir en T4) y este archivo + su espejo.

## Acceptance criteria

- T1: tabla con `Code quality` y `Feature matrix` + veredicto candidate/- por el umbral.
- T2: conteo de reales vs skipped + lista de conclusions por run.
- T3: cada failure con clase y evidencia (línea de log o re-run).
- T4: procedimiento verificable sin ejecutarlo (GET + shape de API).
- T5: veredicto explícito Fase 2 sí/no con números.

## Applicable checks

- `gh` read-only; sin `cargo`, sin builds, sin tests.
- Route: delegated direct (mapeo ya hecho inline por caída del subagente — free tier fuera de OpenCode).

## Progress

- 2026-09-29: creado por autorización "Fase 1 primero". T1 en curso.
- 2026-09-29: T1 DONE — 23 runs current-schema (09-09..09-29): Code quality 23/23 p95 0.6min, Feature matrix 23/23 p95 4.4min, CI Gate 23/23. Nota: la primera pasada mezcló eras viejas del workflow (Deny/Audit/Test 1.88) — se recalculó filtrando runs con `CI Gate`. Run 36507069953 (failure global) tiene gate verde; lo rojo es `windows-latest` advisory.
- 2026-09-29: T2 DONE — 16 PR runs escaneados, 15 ejecuciones reales 15/15 success, 1 skipped (docs-only, por arrastre de `clippy` sin `if:` propio).
- 2026-09-29: T3 DONE — 0 failures en ambas muestras: nada que clasificar (ni deuda ni flakiness observados).
- 2026-09-29: T4 DONE — creado `docs/break-glass-enforce-admins.md` (solo GET verificado; DELETE/POST documentados sin ejecutar).
- Commits: (ninguno — Fase 1 solo agrega el doc T4, pendiente de commit en el próximo work-unit).

## Next step

Correr T1-serie-main.

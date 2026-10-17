# Feature: auditoría de veracidad documental — plan por inventario medido

**Issue:** TBD (crear al implementar; requiere `status:approved` del owner)
**Labels:** `type:chore`
**Branch:** `docs/truthfulness-audit` (rama base: `main`; `check-topology.sh` admite `docs/*` → `main`)
**Base:** `main` @ `v2.11.0` (`67dc2335`)
**Ruta de entrega:** `single-pr` por fase (≤400 líneas por PR; este repo no admite stacked PRs — ver AGENTS.md "No stacked PRs")

## Objetivo

Cerrar las 4 clases de mentira documental que la auditoría delegada **midió** como
presentes, y convertir los detectores que ya existen pero no gates (linter en warn-only)
en gates fail-closed. Cada tarea nace de un conteo verificado, no de una hipótesis.

## Specs

- **S1** — Ningún link intra-doc roto permanece en superficie privada. Los 39 links que
  `--document-private-items` revela deben resolverse o documentarse como tal.
- **S2** — Ningún pin de versión en narrativa contradice la fuente de verdad (workspace
  version / git tags). Los 5 pins `v2.4.0` deben regenerarse o parametrizarse.
- **S3** — Todo capítulo bajo `docs/src/` está enlazado desde `SUMMARY.md`. Los 3
  huérfanos (`budget-source-audit.md`, `bounded-memory-decisions.md`,
  `ai-providers-design.md`) se enlazan o se reclasifican.
- **S4** — Un detector de drift que reporta `::error::` no puede mergear verde. Hoy
  `check_support_drift.sh` sale exit 1 gritando y CI lo deja pasar (warn-only).
- **S5** — Los 3 lints rustdoc `allow-by-default` quedan explícitos en el código, no
  implícitos en el `-D warnings` de CI.
- **S6** — Un sample de código narrativo tiene una fuente única: el archivo real o el
  `{{#include}}`. Ningún sample de `docs/src/` es copiado a mano si puede extraerse.
- **S7** — `SUPPORT.md` y `support.json` reflejan la línea publicada vigente (2.11 al
  momento de esta auditoría).

## Contexto verificado

Superficie pública **verde**: `cargo test --doc --workspace --all-features` → 77 passed,
0 failed, 7 ignored. `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features
--no-deps` → exit 0. Los 3 lints `allow-by-default` forzados
(`missing_crate_level_docs`, `private_doc_tests`, `unescaped_backticks`) → 0 warnings.

| # | Hallazgo | Conteo | Ruta |
| --: | --- | --- | --- |
| 1 | Links intra-doc rotos, solo en superficie privada | **39** | `container.rs:237,301,313,322,331,957`; `cli/args/crawler.rs:173`; `cli/args/mod.rs:627,682`; `mcp_server/handlers/scraping.rs:1154`; `mcp_server/params.rs:1181` |
| 2 | Pins `VERSION=v2.4.0` contra workspace `2.11.0` | **5** | `README.md:37`; `docs/src/installation.md:57,137,174,220` |
| 3 | Capítulos huérfanos de `SUMMARY.md` | **3** | `budget-source-audit.md`, `bounded-memory-decisions.md`, `ai-providers-design.md` |
| 4 | `support.json` STABLE=2.1 con publicado 2.11 | **1** | `.github/support.json:4-11` |
| 5 | Lints rustdoc `allow-by-default` no declarados | 0/4 crates | `crates/*/src/lib.rs` |
| 6 | Cero `{{#include}}` / `{{#rustdoc_include}}` en `docs/src` | 0 | `docs/book.toml` sin preprocessors |

Notes de método: los 3 auditores `gentle-ai-explore` no tienen shell; mapean con
read/grep/codegraph y el parent ejecuta los comandos. Lauu medición #1 y #2 fue
re-ejecutada por el parent.

## Tasks

### Fase 1 — Determinístico, fail-closed (el 90% del valor)

- [ ] **T1 (S1)** — Resolver o marcar los 39 links privados. Ojo: `[[`100,599`]]` y
      `REASON_*` son code spans, no links — distinguir antes de editar.
      Ruta: delegated (writer, 6+ archivos no triviales). Riesgo: item 3 (contrato
      público consumido).
      Verificación: `RUSTDOCFLAGS="--document-private-items" cargo doc --workspace
      --all-features --no-deps` → 0 `unresolved link`.
- [ ] **T2 (S2)** — Parametrizar los 5 pins de versión. Preferencia: preprocessor mdBook
      que inyecte la versión desde `Cargo.toml` (mata la clase, no la detecta); fallback
      documentado: marker `<!-- version-stale:allow -->` para historia intencional.
      Ruta: inline. Riesgo: ninguno.
      Verificación: `rg -n 'VERSION=v[0-9.]+|\$Version =' docs/ README.md` → solo
      el valor vigente; `mdbook build docs` verde.
- [ ] **T3 (S3)** — Enlazar los 3 huérfanos en `SUMMARY.md` o reclasificarlos fuera de
      `docs/src/`. Nota: `ai-providers-design.md` contiene los **9 únicos fences rust**
      de `docs/src/` — si se mueve, esos ejemplos salen del alcance de `mdbook test`.
      Ruta: inline. Riesgo: ninguno.
      Verificación: todo `.md` de `docs/src/` listado en `SUMMARY.md`.
- [x] **T4a (S4, S7)** — Backfill de `support.json` a la minor publicada (Opción A,
      aprobada por el owner). Commit `f404dc7c` en `chore/support-window-backfill`.
      Estado declarado: 2.11 STABLE / 2.10 MAINTENANCE / 2.2-2.9 y 2.1 EOL con
      `eol_reason` de backfill; `2.1 latest` corregido a 2.1.1 (tag existente).
      `rotate-stable.sh` se negó a cruzar el gap por diseño (`:51-66`) — cierre de
      gobernanza, no script run.
      Verificado: `--scope=both` exit 0 · `test_support_drift.sh` 26 ok, 0 fail ·
      `render-support-md.sh --check` exit 0. **Pendiente:** issue con
      `status:approved` + PR (el agente no auto-aprueba).
- [ ] **T4b (S4)** — Wiring: pasar `--scope=published` de warn-only a required gate.
      Va DESPUÉS del merge de T4a; hacerlo antes deja todo PR en rojo.
      Riesgo: item 3 (contrato de CI) + item 5 (delivery).
- [ ] **T5 (S5)** — Declarar los 3 lints en `crates/*/src/lib.rs`; añadir
      `deny(missing_docs)` + `[lints] workspace=true` a `webfang_benchmark`.
      Ruta: inline. Riesgo: ninguno (0 warnings hoy).
      Verificación: los 3 lints explícitos presentes; cargo doc sigue exit 0.

### Fase 2 — Estructural (evita la recurrencia)

- [ ] **T6 (S6)** — Migrar samples de `docs/src/` a archivos reales + `{{#include}}`.
      Empezar por los que importan: `installation.md` (47 bash fences) y
      `debugging.md` (17). Los 6 powershell son un caso aparte (nada los ejecuta).
      Ruta: delegated (writer, editor de superficie amplia).
      Verificación: `rg -c '{{#include' docs/src/` > 0; `mdbook build docs` verde;
      `scripts/gen-cli-reference.sh --check` sigue verde.
- [ ] **T7 (S1)** — Mover el guard MCP version-marker de local-only a CI
      (`scripts/check_mcp_public_surface.sh --base`).
      Ruta: delegated (toca `.github/`). Verificación: marker check en job de CI.

### Fase 3 — Reporte externo (warn-only, nunca gate)

- [ ] **T8** — Links externos vivos (`lychee` sin API key, o `jina read` con
      `JINA_API_KEY` en secrets) y drift semántico (`jina embed` doc-vs-código) como
      reporte scheduled. Claims de deps contra Context7 con versión fijada.
      Ruta: deferred — sólo tras Fase 1 verde.
      Justificación de no-gate: red + quota + versions = flaky por diseño.

## Log

- **L1 (2026-10-17)** — Usuario reporta preocupación por documentación con datos falsos
  y pide verificar si aplica a webfang. Auditoría inicial: superficie pública verde
  (77 doctests, rustdoc exit 0).
- **L2 (2026-10-17)** — Rechazo explícito de la primera propuesta por sesgo de anclaje
  ("te estás focalizando en solo uno o dos errores"). Investigación externa con Jina +
  Context7 → taxonomía de 10 clases de mentira.
- **L3 (2026-10-17)** — Auditoría delegada (3 exploradores paralelos, superficies
  disjuntas) + mediciones shell del parent. Inventario de 6 hallazgos medidos arriba.
- **L4 (2026-10-17)** — Este plan. Orden Fase→Fase por valor/riesgo; sin stacked PRs
  (este repo las rechaza en `check-topology.sh`).

## Next steps

1. Owner decide T4: rotar `support.json` a 2.11 (gobernanza, no agente) y si el
   gate pasa a required.
2. T1 primero: es el único hallazgo con superficie de código real.
3. `cargo fmt --all -- --check`, clippy strict y rustdoc verdes antes de cada commit.
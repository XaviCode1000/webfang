# Feature: auditoría de veracidad documental — plan por inventario medido

**Issue:** TBD (crear al implementar; requiere `status:approved` del owner)
**Labels:** `type:chore`
**Branch:** `docs/truthfulness-audit` (T1/T2/T3/T5/T6) y `ci/support-drift-required`
(T4b); rama base `main`; `check-topology.sh` admite `docs/*` y `ci/*` → `main`
**Base:** `main @ v2.11.0` (`67dc2335`); mediciones re-validadas sobre `main @ 979f73bb`
**Ruta de entrega:** `single-pr` por fase (≤400 líneas por PR; este repo no admite stacked PRs — ver AGENTS.md "No stacked PRs")

## Objetivo

Cerrar las clases de mentira documental que la auditoría delegada **midió** como
presentes (6 hallazgos, 7 specs), y convertir los detectores que ya existen pero no
gates (linter en warn-only) en gates fail-closed. Cada tarea nace de un conteo
verificado, no de una hipótesis.

## Specs

- **S1** — Ningún link intra-doc roto permanece en superficie privada. Los 39 warnings
  que `--document-private-items` revela se cierran en dos clases distintas: **33 links
  reales** (resolver) y **6 falsos positivos** (escapar), no "39 links".
- **S2** — Ningún pin de versión en narrativa contradice la fuente de verdad (workspace
  version / git tags). Las **8** ocurrencias de `v2.4.0` deben regenerarse o
  parametrizarse — no 5: el conteo original sólo miraba asignaciones `VERSION=`.
- **S3** — Todo capítulo bajo `docs/src/` está enlazado desde `SUMMARY.md`. Los 3
  huérfanos (`budget-source-audit.md`, `bounded-memory-decisions.md`,
  `ai-providers-design.md`) se enlazan o se reclasifican.
- **S4** — Un detector de drift que reporta `::error::` no puede mergear verde. Antes
  `check_support_drift.sh` salía exit 1 gritando y CI lo dejaba pasar (warn-only).
  Cerrado en T4b: el scope `published` corre dentro de `Validate PR metadata`, que ya
  es required y no está path-filtered.
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
  (`rustdoc::missing_crate_level_docs`, `rustdoc::private_doc_tests`,
  `rustdoc::unescaped_backticks`) → 0 warnings. Nótese el namespace `rustdoc::`:
  en rustc 1.88 los nombres sin namespace están renombrados y `-D warnings`
  los convierte en error, así que la declaración debe usar el nombre nuevo.

| # | Hallazgo | Conteo | Ruta |
| --: | --- | --- | --- |
| 1 | Links intra-doc rotos, solo en superficie privada | **39** | `container.rs:237,301,313,322,331,957`; `cli/args/crawler.rs:173`; `cli/args/mod.rs:627,682`; `mcp_server/handlers/scraping.rs:1154`; `mcp_server/params.rs:1181` |
| 2 | Pins `v2.4.0` contra workspace `2.11.0` | **8** | `README.md:37`; `docs/src/installation.md:57,64,67,120,121,137,174,220` |
| 3 | Capítulos huérfanos de `SUMMARY.md` | **3** | `budget-source-audit.md`, `bounded-memory-decisions.md`, `ai-providers-design.md` |
| 4 | `support.json` STABLE=2.1 con publicado 2.11 | ~~1~~ **0** | `.github/support.json` — cerrado en #2003 |
| 5 | Lints rustdoc `allow-by-default` no declarados | 0/4 crates | `crates/*/src/lib.rs` |
| 6 | Cero `{{#include}}` / `{{#rustdoc_include}}` en `docs/src` | 0 | `docs/book.toml` sin preprocessors |

Notes de método: los 3 auditores `gentle-ai-explore` no tienen shell; mapean con
read/grep/codegraph y el parent ejecuta los comandos. La medición #1 y #2 fue
re-ejecutada por el parent el 2026-10-17 sobre `main @ 979f73bb`:
`--document-private-items` → exactamente **39** `unresolved link`; `grep -c` sobre
`v2.4.0` → **8**.

De los 39, **6 no son links**: `src`, `srcset` ×2 y `href` son atributos en prosa
(`/// Selector for img[srcset]`, `crates/webfang_core/src/extractor/mod.rs:17`) y
`[100,599]` / `` [`REASON_*`] `` son brackets literales
(`crates/webfang_mcp/src/mcp_server/params.rs:1181`,
`crates/webfang_mcp/src/mcp_server/validation.rs:65`). rustdoc los parsea como
sintaxis de link, así que **aparecen en el conteo aunque no haya nada que resolver**:
se escapan con `\[` o backticks. El conteo es correcto; la descripción original
("code spans, no links") no lo era.

## Tasks

### Fase 1 — Determinístico, fail-closed (el 90% del valor)

- [x] **T1 (S1)** — Cerrar los 39 warnings en dos pasadas: **33 links reales** (resolver
      a un path válido o reescribir la prosa) y **6 a escapar** (`src`, `srcset` ×2,
      `href`, `100,599`, `REASON_*` — son brackets literales, no items). No son code
      spans: rustdoc los cuenta como links justamente porque usa la misma sintaxis.
      Ruta: delegated (writer). Riesgo: item 3 (contrato público consumido).
      Verificación: `RUSTDOCFLAGS="--document-private-items" cargo doc --workspace
      --all-features --no-deps 2>&1 | grep -c 'unresolved link'` → **0**.

      **Implementado.** 24 archivos, 39 líneas (una por warning), 32 links resueltos
      a su path real, 1 reescrito como code span (`jsonrpc_panic_response` es un `fn`
      module-private en `server.rs`: rustdoc no lo linkea desde un módulo hermano ni
      con `--document-private-items`, y ampliar su visibilidad sería un cambio de
      código), 6 brackets escapados. Dos patrones dominaron: el path bare se resuelve
      sólo contra el módulo actual (faltaba el prefijo), y un método de trait no se
      puede linkear como `Type::method` sin el trait en scope.
      Verificado: `--document-private-items` → 0 · `-D warnings` exit 0 ·
      doctests 77 passed / 7 ignored (baseline intacto) · `ci_fast_gate.sh` GREEN
      (PASS=26, FAIL=0) · diff 100% doc comments, 0 `allow()` agregados.
- [ ] **T2 (S2)** — Parametrizar las 8 ocurrencias de `v2.4.0`. Preferencia:
      preprocessor mdBook que inyecte la versión desde `Cargo.toml` (mata la clase, no
      la detecta); fallback documentado: marker `<!-- version-stale:allow -->` para
      historia intencional.
      Ruta: inline. Riesgo: bajo, no nulo — toca `docs/book.toml` y el build del libro.
      Verificación: `grep -rn 'v2\.4\.0' README.md docs/src/` → sólo historia marcada;
      `mdbook build docs` verde. Ojo: el regex del plan original
      (`VERSION=v[0-9.]+|\$Version =`) **no ve** `installation.md:64,67,120,121`, que
      también están pinneadas — la verificación tiene que ser sobre `v2\.4\.0` a secas,
      no sobre la forma de asignación.
- [ ] **T3 (S3)** — Enlazar los 3 huérfanos en `SUMMARY.md` o reclasificarlos fuera de
      `docs/src/`. Nota: `ai-providers-design.md` contiene los **9 únicos fences rust**
      de `docs/src/` — si se mueve, esos ejemplos salen del alcance de `mdbook test`.
      Ruta: inline. Riesgo: bajo, no "ninguno" — mover `ai-providers-design.md` saca
      esos 9 fences del alcance de `mdbook test`, que es una pérdida de cobertura real.
      Verificación: `comm -23 <(ls docs/src/*.md | xargs -n1 basename | sort)
      <(grep -oE '\([a-z0-9-]+\.md\)' docs/src/SUMMARY.md | tr -d '()' | sort)` → vacío.
- [x] **T4a (S4, S7)** — Backfill de `support.json` a la minor publicada (Opción A,
      aprobada por el owner). **MERGED** en `main` como `03b3a620` (PR #2003).
      Estado declarado: 2.11 STABLE / 2.10 MAINTENANCE / 2.2-2.9 y 2.1 EOL con
      `eol_reason` de backfill; `2.1 latest` corregido a 2.1.1 (tag existente).
      `rotate-stable.sh` se negó a cruzar el gap por diseño (`:51-66`) — cierre de
      gobernanza, no script run.
      Verificado post-merge sobre `979f73bb`: `--scope=declared` exit 0 ·
      `--scope=published` exit 0 · `test_support_drift.sh` 26 ok, 0 fail.
      Artefactos locales gitignoreados aparte en `979f73bb` (PR #2006).
- [x] **T4b (S4)** — Wiring: `--scope=published` de advisory a gate de merge.
      **El plan original era incorrecto y no se puede ejecutar tal cual.** Tres
      correcciones, medidas sobre `main @ 979f73bb`:

      1. **Path filter = deadlock.** El bloque `paths:` de `support-drift.yml`
         (entonces `:40-51`, hoy `:45-56`) filtra `pull_request` a 9 rutas. Agregar
         ese job a branch protection sin tocar el filtro deja "pending" para siempre a
         todo PR que no toque esas rutas → repo inmergeable. Un workflow
         path-filtered **no puede** ser required context.
      2. **No renombrar `Validate PR metadata`.** Es un required context
         (`docs/research/ci-test-playbook.md:21`). Renombrarlo deja el contexto
         requerido sin reportar y traba todos los PRs sin error visible.
      3. **El header de `support-drift.yml` era falso** (`:12-35` antes del cambio):
         documentaba "main is drifted RIGHT NOW … 2.1 STABLE" y pedía quitar
         `continue-on-error` "en ese mismo commit" — que fue #2003. Reescrito acá.

      **Diseño elegido:** meter el step en `Validate PR metadata`
      (`pr-validation.yml`), que ya es required y corre en todo PR y en `merge_group`,
      sin path filter. Requiere `fetch-depth: 0` en ese checkout (el script rechaza
      shallow clone a propósito, `check_support_drift.sh:120-122`). `support-drift.yml`
      conserva el job para `push: main` (release-plz crea tags y nada más valida ese
      drift post-merge) y pierde `continue-on-error`.
      Riesgo: item 3 (contrato de CI). Verificación: `actionlint` verde; en un PR de
      prueba con `support.json` alterado, `Validate PR metadata` falla; con el archivo
      correcto, pasa. El gate queda activo **sin tocar branch protection** — la razón
      principal de esta opción sobre un contexto nuevo (`docs/ci-slo.md:60`).

      **Implementado.** `pr-validation.yml`: `fetch-depth: 0` + step
      `Support drift vs published tags (enforcing)`. `support-drift.yml`: fuera
      `continue-on-error`, job renombrado, header reescrito (la sección "WHY ADVISORY"
      contradecía el estado real y se reemplazó por "WHY THE MERGE GATE IS NOT THIS
      JOB"). Verificado en worktree aislado: caso verde → exit 0; `latest` cambiado a
      `2.11.9` → `::error::` + exit 1. `actionlint` verde en ambos workflows.
      **No se renombró `Validate PR metadata`** (required context) ni se tocó branch
      protection.
- [x] **T5 (S5)** — Declarar los 3 lints en las **4 crates publicadas**.
      **Hallazgo de implementación:** en rustc 1.88 los nombres sin namespace
      (`missing_crate_level_docs`, `private_doc_tests`, `unescaped_backticks`) están
      renombrados a `rustdoc::*`, y `-D warnings` los convierte en error de compilación.
      Se usó el nombre nuevo en todos lados.
      - `webfang_core`, `webfang_ai`, `webfang_mcp`: 3 líneas `#![allow(rustdoc::…)]`
        en `lib.rs`.
      - `webfang_cli`: crate binario (solo `main.rs`, sin `lib.rs`) → las 3 líneas
        fueron a `src/main.rs`. La verificación `grep -c` contempla `main.rs` también.
      - `webfang_benchmark`: añadido `[lints] workspace = true` a `Cargo.toml` (era la
        única sin bloque `[lints]`) y `#![deny(missing_docs)]` en `lib.rs`.
        El deny destapó **62** items sin doc en `error.rs`, `aggregate.rs`,
        `competitor/mod.rs`, `corpus.rs`, `cost/config.rs`, `runner.rs`; se documentaron
        todos con comentarios precisos (inglés, coherente con la convención del crate).
      Ruta: inline. Riesgo: bajo tras documentar.
      Verificación: `grep -cE 'rustdoc::missing_crate_level_docs|rustdoc::private_doc_tests|rustdoc::unescaped_backticks'`
      por `lib.rs`/`main.rs` → 3 en cada publicada; `RUSTDOCFLAGS=-D warnings cargo doc
      --workspace --all-features --no-deps` → exit 0; `cargo fmt --check` y clippy de
      `webfang_benchmark` limpios; `ci_fast_gate.sh` verde.

### Fase 2 — Estructural (evita la recurrencia)

- [ ] **T6 (S6)** — Migrar samples de `docs/src/` a archivos reales + `{{#include}}`.
      Empezar por los que importan: `installation.md` (47 bash fences) y
      `debugging.md` (17). Los 6 powershell son un caso aparte (nada los ejecuta).
      Ruta: delegated (writer, editor de superficie amplia).
      Verificación: `grep -rF '{{#include' docs/src/ | wc -l` → > 0 (no `rg -c`, que
      imprime `archivo:conteo` y no es comparable); `mdbook build docs` verde;
      `scripts/gen-cli-reference.sh --check` sigue verde.
- [ ] **T7 (S1)** — Mover el guard MCP version-marker de local-only a CI
      (`scripts/check_mcp_public_surface.sh --base`).
      Ruta: delegated (toca `.github/`). Riesgo: mismo path-filter trap que T4b —
      verificar que el job que lo hospeda corra en todo PR antes de hacerlo required.
      Verificación: `actionlint` verde; el step aparece en el run de un PR que **no**
      toca el fixture ni el marker (prueba de que no hay filtro que lo saltee).

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
- **L5 (2026-10-17)** — Revisión del plan contra `main @ 979f73bb`. Cuatro
  correcciones: T4a ya está mergeada (#2003, #2006); S2 son 8 pins y no 5; los 39
  warnings incluyen 6 escapes que no son links; y T4b tal como estaba escrito
  deadlockeaba los PRs por el path filter de `support-drift.yml`. T4b rediseñado
  sobre `Validate PR metadata` (ya required, sin filtro).
- **L6 (2026-10-17)** — T4b implementado en `ci/support-drift-required`:
  `fetch-depth: 0` + step `check_support_drift.sh --scope=published` en
  `Validate PR metadata`; `support-drift.yml` sin `continue-on-error`, job renombrado
  a `(enforcing)` y header reescrito. Verificado: `actionlint` 0 · caso verde exit 0 ·
  `latest` roto → `::error::` + exit 1 (worktree aislado). **Bloqueado para PR:** no
  existe issue con `status:approved` y el agente no puede aplicar ese label.
  **Mergeado** después como `f6a14063` (PR #2008); la issue fue #2007.
- **L7 (2026-10-17)** — T1 ejecutado en worktree aislado
  (`webfang-worktrees/docs-truthfulness-audit`, rama `docs/truthfulness-audit`) según
  `AGENTS.md`: bootstrap per-tree, CodeGraph/CodeDB indexados con path absoluto, writer
  delegado con superficies de edición acotadas, verificación del parent antes de
  commitear. La lista de 11 archivos del inventario original era parcial: los 39
  warnings viven en **24 archivos**.

- **L9 (2026-10-17)** — T5 implementado en `chore/lint-declaration` (rama nueva desde
  `origin/main`, para no inflar el PR #2012 de T2+T3). Hallazgo clave: en rustc 1.88 los
  3 lints `allow-by-default` están renombrados a namespace `rustdoc::` y `-D warnings`
  los convierte en error, así que la declaración usa `rustdoc::missing_crate_level_docs`,
  `rustdoc::private_doc_tests`, `rustdoc::unescaped_backticks` (3 líneas `#![allow(...)]`
  por crate). `webfang_cli` es binario (solo `main.rs`) → las 3 líneas fueron a
  `src/main.rs`. `webfang_benchmark`: `[lints] workspace = true` + `#![deny(missing_docs)]`
  en `lib.rs`; el deny destapó 62 items sin doc (error.rs, aggregate.rs, competitor/mod.rs,
  corpus.rs, cost/config.rs, runner.rs) que se documentaron con comentarios precisos.
  `cargo doc -D warnings` exit 0, fmt/clippy limpios, `ci_fast_gate.sh` verde.

## Next steps

1. T5: PR en `chore/lint-declaration` (verificar `ci_fast_gate.sh` verde antes de abrir).
2. T8: reporte scheduled de links externos vivos (lychee sin API key, o jina con
   `JINA_API_KEY` en secrets) + drift semántico; **warn-only, nunca gate**. Toca
   `.github/` → requiere issue con `status:approved` (mismo patrón que T4b/#2007).
3. `cargo fmt --all -- --check`, clippy strict y rustdoc verdes antes de cada commit.
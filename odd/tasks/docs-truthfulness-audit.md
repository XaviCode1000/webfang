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
  version / git tags). Las **9** ocurrencias de `v2.4.0` deben regenerarse o
  parametrizarse — no 8: el conteo anterior tenía un error aritmético (8 en
  `installation.md` + 1 en `README.md`), corregido al implementar T2.
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
- **S8** — Ninguna afirmación de seguridad en la narrativa contradice lo que el release
  publica. Medido: `installation.md` afirmaba "checksums, not GitHub attestations"
  mientras cada asset de `v2.11.0` trae una atestación SLSA verificable.

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
| 2 | Pins `v2.4.0` contra workspace `2.11.0` | **9** | `README.md:37`; `docs/src/installation.md:57,64,67,120,121,137,174,220` |
| 3 | Capítulos huérfanos de `SUMMARY.md` | **3** | `budget-source-audit.md`, `bounded-memory-decisions.md`, `ai-providers-design.md` |
| 4 | `support.json` STABLE=2.1 con publicado 2.11 | ~~1~~ **0** | `.github/support.json` — cerrado en #2003 |
| 5 | Lints rustdoc `allow-by-default` no declarados | 0/4 crates | `crates/*/src/lib.rs` |
| 6 | Cero `{{#include}}` / `{{#rustdoc_include}}` en `docs/src` | 0 | `docs/book.toml` sin preprocessors |
| 7 | `installation.md` afirma que no hay atestaciones; **sí las hay** | **1** | `docs/src/installation.md:128-132` |

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
- [x] **T2 (S2, S8)** — Cerrar las 9 ocurrencias de `v2.4.0` y la afirmación de
      seguridad que las acompañaba.
      Ruta: inline. Riesgo: bajo, no nulo — toca `docs/book.toml` y el build del libro.
      Verificación: `grep -rn 'v2\.4\.0' README.md docs/src/` → sólo historia marcada;
      `mdbook build docs` verde. Ojo: el regex del plan original
      (`VERSION=v[0-9.]+|\$Version =`) **no ve** `installation.md:64,67,120,121`, que
      también están pinneadas — la verificación tiene que ser sobre `v2\.4\.0` a secas,
      no sobre la forma de asignación.

      **Implementado, con dos desvíos del plan, ambos por una razón medida.**
      (1) **No hay preprocessor.** `README.md` lo renderiza GitHub, no mdBook: un token
      `{{…}}` se le mostraría crudo al lector. Y sustituir la versión dentro de un bloque
      "Observed output" fabricaría una observación que nadie hizo. Se implementó en su
      lugar `scripts/check_docs_version_pins.sh` (gate fail-closed contra
      `[workspace.package] version`, con marker `<!-- version-stale:allow -->` para
      historia intencional, alcance de línea) + `scripts/test_docs_version_pins.sh`
      (8 casos, incluidos los fail-closed). El gate **no está cableado en CI todavía**:
      tocar `.github/` requiere autorización aparte según AGENTS.md.
      (2) **S8 se destapó al medir.** `installation.md` decía "The releases publish
      **checksums, not GitHub attestations**" y que `gh release verify-asset v2.4.0`
      fallaba con `no attestations found`. Falso para la línea vigente: cada asset de
      `v2.11.0` publica un `.sigstore.json` y
      `gh attestation verify webfang-x86_64-unknown-linux-gnu.tar.gz --repo
      XaviCode1000/webfang` **verifica** (exit 0, 1 atestación, firmante
      `release.yml@refs/tags/v2.11.0`). Control negativo con un archivo cualquiera →
      exit 1 / HTTP 404, así que el 0 no es un falso positivo. La nota se reescribió
      para decir la verdad y el bloque "Observed output" se reemplazó por la salida
      real de 14 assets (tenía 5).
      Verificado: `check_docs_version_pins.sh` exit 0 · harness 8 ok / 0 fail ·
      `mdbook build` 0 · `mdbook test` 0 · `mdbook-linkcheck` 0 · `-D warnings` 0.
- [x] **T3 (S3)** — Los 3 huérfanos, **reclasificados** en vez de enlazados.
      La nota del plan era incorrecta: los 9 fences de `ai-providers-design.md` **no
      estaban** en el alcance de `mdbook test` — un capítulo huérfano no se construye,
      así que nunca se testearon. Y no compilan sueltos: son excerpts ilustrativos
      (`pub enum AuthSource` sin imports). Por eso reclasificar era correcto.
      **Pero el doc seguía mintiendo**: declaraba "Implementación a desarrollar en el
      worktree `feat/ai-providers`" cuando `infrastructure/llm/` ya existe. El rename
      puro (0 inserciones) no corrigió la mentira — la mudó de path.
      Cerrado con una reescritura de las regiones de estado contra el árbol verificado:
      8 ítems de "pendiente" volteados a hechos con paths reales, 3 firmas inventadas
      (`resolve_embedding` / `resolve_completion` / `resolve_capabilities`) sustituidas
      por la real `resolve(provider_id, Capability) -> &ProviderConfig`
      (`domain/providers.rs:251`), 2 bloques de estado por sesión eliminados
      (`codedb`/`codegraph`/`direnv`/SHA obsoleto), `TenantAiContext` marcado como no
      implementado con tracker (#2018), y los fixtures preservados por decisión del owner.
      6 afirmaciones nuevas introducidas por el rewrite, las 6 verificadas contra el árbol.
      Quedan 5 mentiras en §1–§5, fuera de alcance aquí → follow-up (#2019).
      Ruta: delegada (writer acotado) sobre investigación previa (read-only explorer).
      Riesgo: item 3 (el doc es spec normativa citada por 13 archivos de código) —
      mitigado por no tocar §1–§4 ni §8.
      Destinos: `ai-providers-design.md` → `docs/architecture/` (7 referencias
      `docs/src/…` en comentarios de código actualizadas, 15 en total mencionan el doc);
      Destinos: `ai-providers-design.md` → `docs/architecture/` (7 referencias
      `docs/src/…` en comentarios de código actualizadas, 15 en total mencionan el doc);
      `budget-source-audit.md` y `bounded-memory-decisions.md` → `docs/research/`.
      Verificación: `comm -23` de capítulos contra `SUMMARY.md` → vacío;
      `mdbook build` / `test` / `linkcheck` verdes.
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

- [x] **T9** — Advisories de la revisión de #2012 sobre `scripts/check_docs_version_pins.sh`
      (hallados por la revisión delegada; verificados por el parent el 2026-10-17):
      1. **Gate no cableado** — ningún workflow en `.github/` invocaba el script: era un
         guard inerte. **Resuelto en T9.1**: el gate y su harness corren en el job
         `repo-guards` de `ci.yml`, el único sin `paths:` ni `if:` y ya listado por `gate`
         como always-applicable en ambos scopes, así que el drift roja el `CI Gate`
         requerido incluso en un PR docs-only. La vía filtrada es imposible por el mismo
         motivo que T4b/T7 documentan: un workflow detrás de `paths:` nunca puede ser
         contexto requerido.
      2. **Asimetría RC (confirmada empíricamente)** — un pin de doc `v2.11.0-rc.1`
         PASABA contra workspace `2.11.0` stable (el grep pelaba el prerelease → falso
         negativo), pero un workspace `2.11.0-rc.1` ABORTABA el script
         ("formato inesperado"). El gate no corría durante la ventana de un RC.
         **Resuelto** (`a1f14d87`, PR #2025): una gramática única `VER_ERE`/`PIN_ERE` para
         ambos lados del compare; el workspace acepta sufijo y el pin se compara entero.
      3. **Harness no cubría prerelease (la advisory original es imprecisa)** — el harness
         SÍ prueba mismatch bien-formado plano: tests 2 y 3 usan `v2.4.0` vs `2.11.0` y
         fallan correctamente. La brecha real era que no había caso con pin prerelease, que
         es justo el que dispara el bug (2). **Resuelto** (#2025): casos 9/10/11, incluida
         la dirección estable→RC que antes pasaba en silencio.
      4. **Línea de éxito mentía / `$checked` muerto** — la variable `checked` se
         incrementaba pero nunca se imprimía; el OK de éxito usaba `${#FILES[@]}`
         (cantidad de archivos) y afirmaba que el doc "declara sólo v2.11.0" cuando en el
         caso RC declara `v2.11.0-rc.1`. **Resuelto** (#2025): la línea de éxito reporta
         `$checked` (líneas con pines, no documentos).
      Ruta: inline para (2)/(3)/(4) — sólo el script + harness, sin `.github/`; T9.1 (1)
      delegated tras issue `status:approved` + ask-first. Riesgo: bajo.
      Verificación: harness `11 ok, 0 fail`; gate exit 0 sobre `main`
      (`OK: 7 líneas con pines de versión en 11 documentos`); actionlint 1.7.12 +
      shellcheck 0.11.0 limpios con el wiring nuevo; `ci_fast_gate.sh` GREEN; review RDD
      aprobada y quemada sobre `a1f14d87` (lineage `review-38315f8f5b874640`).
      Tracking issue: #2017 (`status:approved`, `type:bug`). T9 (2)(3)(4) mergeado como
      `455215d2` (PR #2025); T9.1 en PR propio (commit `dc3af78e`).

### Fase 3 — Reporte externo (warn-only, nunca gate)

- [x] **T8** — Links externos vivos como reporte scheduled. **Slice lychee-keyless**
      (decisión del usuario): `.github/workflows/external-link-report.yml` corre
      `lychee` semanal + manual sobre `README.md`, `CHANGELOG.md`, `docs/src/**/*.md`,
      **report-only** (`fail: false`, fuera de cualquier required status, nunca gate) más un
      step que verifica que lychee realmente chequeó. El flag `--exit-zero` que esta sección
      citaba **no existe** en el release pineado (ver L11). Excluye links internos del repo y
      `localhost`.
      Descartado el 2026-10-17 (ver L12): ni drift semántico con `jina embed` ni claims de
      deps contra Context7 son planes del repo — son herramientas de investigación del
      agente. La justificación técnica sigue en pie: red + quota + versions = flaky.

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
  warnings viven en **24 archivos**. Mergeado como `15de3130` (PR #2010, issue #2009).
- **L8 (2026-10-17)** — T2 + T3 en `docs/truthfulness-audit`, sobre `main @ 15de3130`.
  Dos correcciones al plan y un hallazgo nuevo: el conteo de pins era **9**, no 8
  (error aritmético propio); los 9 fences de `ai-providers-design.md` nunca estuvieron
  en `mdbook test` porque un huérfano no se construye, así que moverlo no pierde
  cobertura; y `installation.md` afirmaba que el release no publica atestaciones cuando
  cada asset de `v2.11.0` trae una verificable. Ese último es el hallazgo más grave de
  toda la auditoría y no estaba en el inventario.
- **L9 (2026-10-18)** — Revisión delegada de #2012 encontró un blocker en T3: mover
  `ai-providers-design.md` NO corrigió la mentira de la línea 4 ("Implementación a
  desarrollar en el worktree `feat/ai-providers`") pese a que `infrastructure/llm/` ya
  existe. El rename fue puro. Corregido en esta rama: línea 4 y la nota de estado
  (línea 308) ahora dicen que la implementación vive en
  `crates/webfang_core/src/infrastructure/llm/`. T3 se des-chequea hasta que el fix de
  prosa se mergee y confirme. La revisión también dejó 4 advisories sobre
  `check_docs_version_pins.sh` (gate no cableado, asimetría RC, harness no testea
  versiones bien-formadas desparejadas, `$checked` muerto / línea de éxito miente) →
  falta tarea que los rastree.
- **L10 (2026-10-18)** — El parent verificó las 4 advisories de T9 contra el código y el
  harness (no se tomaron por fe: se leyó `check_docs_version_pins.sh`,
  `test_docs_version_pins.sh`, y se corrió el harness + 2 fixtures RC). Confirmadas (1)
  gate no cableado (0 refs en `.github/`), (2) asimetría RC (doc `v2.11.0-rc.1` pasa,
  workspace `2.11.0-rc.1` aborta), (4) `$checked` muerto / éxito miente. La (3) es
  imprecisa: el harness SÍ prueba mismatch bien-formado plano (v2.4.0 vs 2.11.0). La
  brecha real es falta de caso prerelease. Creada T9 para rastrearlas.

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

- **L10 (2026-10-17)** — T8 (slice lychee-keyless) en `ci/external-link-report`:
  workflow scheduled (cron semanal + `workflow_dispatch`) que corre `lychee` sobre
  `README.md`, `CHANGELOG.md`, `docs/src/**/*.md`, **report-only** (`fail: false` +
  `--exit-zero`, fuera de branch protection). Issue #2015 con `status:approved`,
  PR #2016. **Corrección (ver L12)**: los slices jina/Context7 que esta entrada daba por
  diferidos no son un plan del repo — Jina y Context7 son herramientas de investigación
  del agente, no dependencias de webfang.

- **L11 (2026-10-17)** — Cierre del slice lychee (defecto del step report-only). El
  `--exit-zero` que L10 lista como mecanismo **no existe** en el release pineado: lychee salía
  con exit 2 (`unexpected argument '--exit-zero' found`) y, con `fail: false`, el job quedaba
  verde — **nunca chequeó un link**. Fix en `1f1deec0`: pin `lycheeVersion: v0.24.2`, flags
  reales y step `Assert lychee actually checked something` (el reporte nunca queda en 0 bytes,
  pero exit 2 es ambiguo entre error de clap y hallazgos, así que el contenido es la única
  evidencia). Harness de regresión nuevo `scripts/test_external_link_workflow.sh` (`df5dcc74`,
  9 casos, offline, sin red) cableado en el job `repo-guards` de `ci.yml`. Review RDD aprobada y
  quemada sobre `df5dcc74` (lineage `review-7f94efc535b19b59`); 3 advisories triados: R3-001 /
  R4-001 **falsos positivos** (probes A/B: `set -e` no aplica en el cuerpo de una función
  invocada como condición `if`) y R3-002 **confirmado** y diferido. Issue #2020 con
  `status:approved`, PR #2021, merge `5fba8567`. Pendientes en #2020: comentario del harness
  `:86-87`, anclaje de los terminadores `awk`, clusters de flags cortos (`-qv`) y fragilidad por
  dependencia del `-e` contextual.

- **L12 (2026-10-17)** — Aclaración de alcance del usuario: **Jina y Context7 son herramientas
  de investigación del agente, no dependencias ni checks de webfang** ("no vamos a utilizar ni
  Jina ni context7 en webfang"). Se corrigen las tres afirmaciones que las daban por plan
  diferido del repo (Fase 3, L10 y `## Next steps`); no había nada que desarmar: cero
  referencias en cualquier `Cargo.toml`, `.github/**` o `mise.toml`, y `JINA_API_KEY` nunca se
  cableó en un workflow. El uso como herramientas de research (L2) queda intacto. En el mismo
  pase se corrigió una cuarta afirmación falsa de la misma sección: Fase 3 seguía describiendo
  `--exit-zero` como el mecanismo vigente, cuando L11 documenta que ese flag no existe en el
  release pineado — el logbook se contradecía a tres párrafos de distancia.

- **L13 (2026-10-17)** — T9.1: cableado del gate de pines de versión. Rama
  `ci/docs-version-pins-gate`, commit `dc3af78e`
  (`ci(repo-guards): cablear el gate de pines de versión en CI`): el gate y su harness corren
  ahora en el job `repo-guards` de `ci.yml`. **Decisión de destino**: `repo-guards` es el único
  job sin `paths:` ni `if:`, y `gate` ya lo lista entre los productores always-applicable de
  ambos scopes — así el drift roja el `CI Gate` requerido incluso en un PR docs-only. Las vías
  filtradas (`docs.yml`, `support-drift.yml`) quedan descartadas por la razón que T4b documenta:
  un workflow detrás de `paths:` nunca puede ser contexto requerido, porque los PR fuera del
  filtro no lo reportarían y quedarían "pending" para siempre. **Desviación respecto del plan
  inicial**: el harness no fue a `lane_docs`/`lane_fmt_and_guards` sino a `lane_ci` con trigger
  por archivo cambiado — `scripts/**` se clasifica CI-only, así que un PR que edita el guard o su
  harness cae en `lane_ci` y nunca en `lane_docs`; cablearlo junto al gate lo habría dejado muerto
  para el único PR que puede romperlo (el mismo trap que comenta el step del compatibility harness
  en `ci.yml`). El gate sí va en `lane_docs` (edición de docs) y en `lane_fmt_and_guards` (bump de
  `[workspace.package] version`), las dos superficies reales del drift; las lanes son mutuamente
  exclusivas, así que no hay doble corrida, y `ci_fast_gate.sh` termina GREEN.
  **Evidencia observada**: actionlint 1.7.12 (idéntico al pineado de CI) y shellcheck 0.11.0 con el
  vocabulario de ignores de `repo-guards`, ambos limpios; gate exit 0
  (`OK: 7 líneas con pines de versión en 11 documentos`); harness `11 ok, 0 fail`; la lane `docs`
  ejerció de verdad la línea nueva (`PASS: docs version pins gate (#2017)` — `odd/tasks/*.md` cuenta
  como docs, así que este PR cae en el branch docs+CI y corre `lane_docs` + `lane_ci`); y el trigger
  de `lane_ci` probado extremo a extremo con una sonda temporal sobre el guard
  (`PASS: docs version pins semantics (offline)`), restaurada byte-idéntica.
  **Verificación independiente** (agente verificador read-only sobre el rango commiteado): **PASS,
  sin bloqueantes**. Confirmó las cinco afirmaciones del cambio y **refutó el gap** que este log
  declaraba: ejercitó `lane_fmt_and_guards` de verdad (bump de `Cargo.toml` a 2.12.0 → el step corre
  y falla, `::error::FAIL: docs version pins gate (#2017)`, exit 1, nombrando
  `docs/src/installation.md:57,64,67,154,191,237` y `README.md:37`), con lo que el gate queda probado
  fail-closed y no decorativo. Verificó además con fixtures propias la dirección del guard (marcador
  en la misma línea vs en otra, RC en ambas direcciones, manifest malformado/ausente, sin docs) y la
  invariancia de hash del árbol, y comparó contra el baseline: ningún **step** preexistente cambió su
  salida. Tres advisories, ninguna bloqueante: **(A)** el comentario de `ci.yml` decía que
  `repo-guards` es "el único job sin `paths:` ni `if:`", y hay 8 jobs sin `if:` a nivel job —
  corregido en un commit aparte de este PR; **(B)** `lane_ci` emite una línea `SKIP` extra en PRs
  CI-only ajenos, inherente al `skip_step` y sin cambio de exit; **(C)** pre-existente y fuera de este
  candidato: el scan del guard cubre sólo `README.md` + `docs/src/*.md`, así que `AGENTS.md`,
  `docs/adr/**`, `docs/release-tag-ruleset.md` y `odd/tasks/**` llevan literales de versión sin
  gatear. No se tocó
  `AGENTS.md` (precedente consistente: ningún step de `repo-guards` se documenta ahí) ni
  `CHANGELOG.md`. Corregida de paso la fecha futura en la cabecera del bloque T9 (`2026-10-18` →
  `2026-10-17`, hoy); las otras dos fechas futuras de L9/L10 siguen pendientes.

## Next steps

1. Advisories no bloqueantes de la review de #2025 sobre `check_docs_version_pins.sh`
   (R2-version-grammar-message, R2-counter-semantics/R3-3, R3-2, R3-1): reportables como
   trabajo aparte, ninguno está en el AC de #2017. T9 ya está cerrado.
2. T6: migrar samples de `docs/src/` a `{{#include}}` (deferred).
3. T7: mover el guard MCP version-marker a CI (deferred, toca `.github/`).
4. `cargo fmt --all -- --check`, clippy strict y rustdoc verdes antes de cada commit.
5. T8 residual (issue #2020, `status:approved`): comentario del harness `:86-87`, anclaje de
   los terminadores `awk`, clusters de flags cortos (`-qv`) y la fragilidad por dependencia del
   `-e` contextual en los call sites.

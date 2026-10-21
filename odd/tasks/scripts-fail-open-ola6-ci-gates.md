# Campaña de remediación fail-open — Ola 6 (CI gates)

**Ola 6 del plan global del audit `AUDIT-SCRIPTS-FAIL-OPEN-20261018-222141`** (umbrella #2127, ola #2133).

17 defectos repartidos en 11 scripts de CI. Tres PRs por **guarantee de superficie disjunta** (un fichero
por worker salvo excepción explícita):

| PR | Ficheros | Hallazgos |
|---|---|---|
| PR-M | `scripts/ci_fast_gate.sh`, `scripts/ci_path_classifier.sh` | B45, B51, B52, B53 |
| PR-N | `scripts/merge-when-green.sh`, `scripts/apply-tag-ruleset.sh`, `scripts/ci-health-close.sh`, `scripts/ci_metrics.sh` | B46, B47, B49, B50, B54, B55 |
| PR-O | `scripts/check_mcp_public_surface.sh`, `scripts/check_concurrency_lints.sh`, `scripts/check_dead_tests.sh`, `scripts/check_archive_reproducibility.sh`, `scripts/check_no_ai_attribution.sh` | B48, B59, B56, B57, B58, B60, B61 |

El canon de verificación es `webfang-audit/.../repro/ci_repro.sh`: **2 matched / 15 confirmed** → **17/0**,
con dos corridas idénticas.

## Specs

- **S1** — "The code claims 'anything else is fail-closed (all=true)', but an unresolvable base falls back to `main`, then **`HEAD~1`**." (B45, audit)
  Una base que no se puede resolver ⇒ `all=true`/FULL; nunca `HEAD~1` en silencio.
- **S2** — "`timeout_effective` is applied only to the `deadline` in the `else` branch at `:277`. When branch protection cannot be read, the fallback runs `gh pr checks --watch --required --fail-fast` with no timeout wrapper, so `--timeout` is silently ignored and the watch blocks forever." (B46, audit)
  El timeout efectivo acota **todas** las ramas, incluida la de branch protection ilegible.
- **S3** — "the bucket value is `cancel`, not `cancelled`; the arm never matches and a cancelled required check waits to the deadline." (B47, audit)
  El bucket `cancel` se reconoce como terminal.
- **S4** — "the default base is the last commit touching the fixture, so a committed fixture edit without a version bump compares the fixture to itself. `ci_fast_gate.sh:599` invokes it exactly that way; CI passes `--base`, so the hole is local pre-push." (B48, audit)
  El modo por defecto no puede comparar el fixture consigo mismo.
- **S5** — "`get_our_ruleset` returns 2 on API failure and 1 only when genuinely absent; `if ruleset_json="$(...)"; then` collapses both, so `apply` POSTs a **duplicate** ruleset on a 500." (B49, audit)
  Un fallo de API no es "no existe": no se postea un ruleset duplicado.
- **S6** — "with `pipefail`, a failed `gh issue view` makes the condition false and prints 'Skip #N: missing label'; the loop exits 0 while the stale issue stays open." (B50, audit)
  Un fallo de lectura no es "falta el label": el error se propaga.
- **S7** — "`mapfile` collects only `^scripts/.*\.sh$`; `scripts/githooks/{pre-push,pre-commit}` are bash with shebangs, classified `ci_only=true`, and then skipped as 'no changed scripts' — never linted." (B51, audit)
  Los hooks con shebang se lintean como scripts.
- **S8** — "`all=true` runs `lane_full`, which never runs the `lane_ci` harnesses (`ci_path_classifier_test.sh`, `test_check_compatibility.sh`, the target-isolation harness) while claiming 'nothing skipped'." (B52, audit)
  El lane FULL incluye los harnesses del lane CI.
- **S9** — "`if grep -rn ...; then fail; fi; echo OK` conflates exit 1 (no match) with exit 2 (cannot scan); an unreadable source tree prints OK." (B53, audit)
  "No pude escanear" ≠ "no encontré nada": el escaneo fallido es fallo.
- **S10** — "required checks are read only from the deprecated `.contexts[]`; a `checks[]`-only protection reports 0 required contexts and exits 0." (B54, audit)
  Una protección con `checks[]` reporta sus contexts requeridos.
- **S11** — "a `jq` failure mid-list silently shrinks the sample while the snapshot still prints as complete." (B55, audit)
  Un fallo de `jq` no produce una muestra truncada que se presenta como completa.
- **S12** — "`let _ = semaphore.acquire().await;` matches the acquire regex, then the `$0 !~ /=/` exclusion skips it. A wildcard binding drops the permit immediately — semantically identical to the bare statement the gate exists to catch." (B56, audit)
  El binding comodín no esquiva la regla del permiso.
- **S13** — "`in_test = 1; next` is never reset, so production code after a test module is never scanned." (B57, audit)
  El estado de "en test" se restablece: el código de producción posterior sí se escanea.
- **S14** — "`mod foo;` is always resolved relative to the file's directory; for a non-root module Rust looks in `<stem>/bar.rs`. Live files are reported dead and dead files reachable." (B58, audit)
  La resolución de `mod` sigue las reglas del módulo.
- **S15** — "`flatten_sectioned` emits rows only for `[tools]`, `[properties]`, `[defs]`; a new section is dropped from both sides, so a breaking addition there leaves `SURFACE_CHANGED=0`." (B59, audit)
  Una sección nueva aparece en ambos lados del diff.
- **S16** — "`SOURCE_DATE_EPOCH=0` passes validation; for a zip, `expected` stays `None` when `epoch < DOS_EPOCH`, so the `date_time` assertion never runs while the success line still prints `mtime~0`." (B60, audit)
  Una aserción que no se ejecutó no puede reportarse como verificada.
- **S17** — "`--fixture` mode uses `printf '%s\n'`, so a message already ending in a newline has its trailer block reset by awk and reads as clean." (B61, audit)
  El modo fixture lee el mismo bloque de trailers que el path de producción.

## Tasks

| ID | Specs | Ruta | PR | Commit |
|---|---|---|---|---|
| T1 | S1, S7–S9 | `scripts/ci_fast_gate.sh`, `scripts/ci_path_classifier.sh` | PR-M (#2133) | pendiente |
| T2 | S2–S3, S5–S6, S10–S11 | `scripts/merge-when-green.sh`, `scripts/apply-tag-ruleset.sh`, `scripts/ci-health-close.sh`, `scripts/ci_metrics.sh` | PR-N (#2133) | pendiente |
| T3 | S4, S12–S17 | `scripts/check_mcp_public_surface.sh`, `scripts/check_concurrency_lints.sh`, `scripts/check_dead_tests.sh`, `scripts/check_archive_reproducibility.sh`, `scripts/check_no_ai_attribution.sh` | PR-O (#2133) | pendiente |
| T4 | S1–S17 | Verificación canónica: `ci_repro.sh` a 17/0, dos corridas idénticas sobre `origin/main` limpio | — | pendiente |

## Log

- **L1** (2026-10-21, petición del mantenedor): "tengo un agente mergeando las pr's abiertas continuamos con las siguientes olas?"
- **L2** (deuda heredada que esta ola asume): `B45` es CRITICAL y estaba sin asignar en el tablero de
  clases; entra en la Ola 6 porque su casa natural es el gate rápido de CI, que es lo que lo invoca.
- **L3** (regla de esta ola): los workers **no** ejecutan `scripts/ci_fast_gate.sh` en local — ese script
  compila el workspace y esta máquina ya sufrió dos OOM por builds concurrentes. La verificación es por
  harness (el repro y el harness propio de cada script), no por el gate completo.

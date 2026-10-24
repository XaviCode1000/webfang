# Merge y limpieza post-merge — Tasks

- [x] T1 · S1 — `scripts/merge-when-green.sh`: `state` y `mergeStateStatus` se leen en **una sola llamada**; `state == MERGED` ⇒ mensaje "already merged" + exit 0 **antes** del loop `UNKNOWN`; el loop re-consulta `state` (otro actor puede mergear el PR mientras espera).
- [x] T2 · S1 — `scripts/test_merge_when_green_merged_state.sh` (nuevo, harness de negación con `gh` falso, modelado sobre `scripts/test_merge_when_green_breaking.sh`) + step "Merged-state semantics harness" en el job `repo-guards` de `.github/workflows/ci.yml`.
- [x] T3 · S2 — `AGENTS.md` §Post-merge cleanup: el borrado del target dir es standalone, con path absoluto literal, nunca `$HOME`/`~` ni encadenado; la cadena de git (pasos 3-5) corre primero y completa.
- [x] T4 · S3 — `AGENTS.md` §Worktree policy: el registro de misión se crea/edita dentro del worktree de la misión.
- [x] T5 · S4 — **cancelada**: el ADR **no** se toca (su lista es un registro histórico congelado; la corrección que la actualiza ya está escrita). #2105 se cierra por evidencia.
- [x] T6 · S1–S4 — verificación: harness nuevo verde (25/0), `test_merge_when_green_breaking.sh` verde (26/0), `test_ci_merge_when_green.sh` verde (17/0), `test_topology.sh` verde, `test_guard_coverage.sh` con el **mismo** rojo pre-existente que `main`.

Spec: `odd/tasks/merge-when-green-merged-state.md`. Cierra #1970, #1971, #1972; #2105 se cierra por evidencia.

## Resultado de la implementación (2026-10-21)

Qué quedó hecho y dónde (el commit es del padre; **sin PR**):

- **S1 / T1** — `scripts/merge-when-green.sh`: helper `read_pr_merge_state` (**una sola
  llamada** `gh pr view --json state,mergeStateStatus`), helper `exit_already_merged`
  (mensaje "already merged" + exit 0) y tres puntos de detección: antes del guard de
  breaking-marker, antes del loop `UNKNOWN` y dentro del loop. Sin flag nuevo y sin código
  de salida nuevo: el estado deseado ("el PR está mergeado") ya está alcanzado, así que 0.
- **S1 / T2** — harness nuevo + los tres `gh` falsos de los harnesses existentes sirven la
  proyección combinada. El harness registra cada `pr view --json <campos>` que pide el
  script y **aserta** que `state` y `mergeStateStatus` se leyeron juntos y una sola vez.
- **S2 / T3** — `AGENTS.md` §Post-merge cleanup, paso 6.
- **S3 / T4** — `AGENTS.md` §Worktree policy §Iron rules (en inglés: el documento es
  íntegramente inglés).
- **S4 / T5** — cancelada. Premisa verificada con `codedb` sobre este árbol:
  `LocalEmbeddingPort` → 0 símbolos en código; `Tier2EmbeddingPort` es el newtype
  `pub struct Tier2EmbeddingPort(Arc<dyn EmbeddingPort>)`, sin variante `Local`. El ADR ya
  registra el colapso y su corrección del 2026-10-21; su lista de residuo es el registro
  congelado del 2026-10-20 y no se edita.

### Decisiones de la revisión del padre

1. **Fuera el fallback legacy**: la primera entrega caía a `--json mergeStateStatus` cuando
   la llamada combinada no devolvía nada — un segundo lector del mismo hecho que además
   **apagaba en silencio** la detección del defecto. Un solo lector; los tres stubs sirven
   la proyección combinada.
2. **Harness portable**: se quitó `timeout` (GNU coreutils); el watchdog es bash puro, para
   que un dev en macOS pueda correrlo.
3. **RED preciso**: el stub vuelve a servir las proyecciones individuales a propósito, para
   que el script pre-fix demuestre el defecto (rc=124: quemó la ventana de 90 s del loop
   `UNKNOWN`) en vez de morir en la primera lectura (rc=99). La propiedad "un solo lector"
   queda asertada, no supuesta.
4. **`AGENTS.md` en inglés**; el doc ODD en español (convención de `odd/tasks/`).
5. **T5 cancelada**: el ADR es un registro histórico.

### Evidencia RED/GREEN (padre, sobre el árbol final)

- **RED** (script pre-fix restaurado con `git show HEAD:scripts/merge-when-green.sh`):
  `PASS=19 FAIL=6` — el caso obligatorio falla con rc=124 (esperó 21 s y murió por el
  deadline del harness), sin mensaje "already merged", entró al loop `UNKNOWN`, y el estado
  se leyó **2 veces por separado**. Restore verificado por sha256.
- **GREEN** (script arreglado): `PASS=25 FAIL=0`; el caso `MERGED` volvió en 0 s.

### Verificación

`bash -n` (script y los tres harnesses): OK. `test_merge_when_green_merged_state.sh` 25/0 ·
`test_merge_when_green_breaking.sh` 26/0 · `test_ci_merge_when_green.sh` 17/0 ·
`test_topology.sh`: all checks passed. `test_guard_coverage.sh`: **mismo** rojo pre-existente
que en `main` (`scripts/test_intra_crate_direction.sh` exige el extractor intra-crate
compilado con cargo; en CI lo construye el job `repo-guards`).
`scripts/ci_fast_gate.sh` no se corrió: el lote es shell + YAML + markdown y un build de
BoringSSL es un riesgo de OOM en esta máquina.

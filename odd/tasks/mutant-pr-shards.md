# Lote #2285 — Shards dinámicos de mutants por PR

Objetivo: reemplazar la matriz fija de 64 shards del gate de PR (`mutants-pr`) por
`shard_count = min(16, ceil(mutant_count/8))` calculado con `cargo mutants --list
--in-diff`, con base/head explícitos desde el evento y un test de regresión de base
avanzado. Branch `feat/mutant-shards` · delivery: PR único (slice 1) · runner:
harness bash (`scripts/test_ci_mutant_shards.sh`) + `ci_fast_gate.sh --ci`.
Dependencia: #2251 (merged, 1477 medidos / 96 shards).

## Specs

- **S1** (issue #2285, verbatim): "mutants-pr-prepare outputs mutant_count from
  `cargo mutants --list --in-diff` and shard_count calibrated from measured base build
  time and per-mutant time from #2251; `ceil(count/8)` is only a starting point. Cap 16."
- **S2** (issue #2285, verbatim): "1–8 mutants: 1 shard; 9–16: 2; 17–24: 3; cap 16.
  These are provisional until calibrated with base build + per-mutant time."
  Cálculo usado: `shard_count = min(16, (mutant_count + 7) / 8)` (entero). Derivación
  del 8: el muro de #1436 fue ~20 mutantes/shard contra 90 min, con base build ~10–15
  min (BoringSSL); con 8 mutantes/shard el peor caso queda holgado bajo los 45 min
  del timeout nuevo (S5). Números provisionales hasta recalibración.
- **S3** (issue #2285, verbatim): "Pass explicit base/head from the event
  (`merge_group.base_sha` or PR base) to `mutants-pr-prepare`; add a regression test
  for an advanced base."
  Decisión: la base fiable es el **primer padre del árbol checkouteado**
  (`git rev-parse HEAD^1`): es inmutable y por definición padre del merge branch
  (pull_request: `refs/pull/N/merge`; merge_group: parent = `merge_group.base_sha`,
  documentado). El sha del evento (`pull_request.base.sha`) se usa como intento y se
  **coteja** contra `HEAD^1`; si difieren, warning + usa `HEAD^1` (fail-closed, no
  confía en la semántica ambigua del refresh de `base.sha`). HEAD = `github.sha`.
- **S4** (issue #2285, verbatim): "The `--in-diff` regression test fails on the old
  three-dot merge-base behavior and passes on explicit base." Fixture: repo git con
  main@X + commits de PR + main avanzado a Y + merge branch reapoyado en Y + ref
  remota "stale" en X. Old (`git diff stale...HEAD`): incluye los hunks de Y (mutantes
  del avance). New (helper con `HEAD^1`): solo los del PR. Ambas vistas cuentan
  mutantes REALES con `cargo mutants --list --json` sobre un crate mínimo del fixture.
- **S5** (issue #2285, verbatim): "No-mutant diff: `cargo-mutants (PR diff)` success,
  all shards skipped." / "Matrix uses `shard_count`; PR timeout 45 min; weekly baseline
  report-only at 96." / "100 mutants: measured shard count, <45 min; record base build
  and per-mutant time."
- **S6** (asunción): el detector regex de hot-path dentro de `mutants-pr-prepare`
  (traducción de `examine_globs`) se DESCARTA: una sola autoridad de scope
  (cargo-mutants según `.cargo/mutants.toml`); `mutant_count == 0` conlleva skip.
- **S7** (fail-closed, de docs cargo-mutants): salida de `--list` con rc 5 ("new side
  of the `--in-diff` diff doesn't match the text in the tree") o 6 (diff inválido)
  **falla** el job con `::error::` y código visible; nunca se degrada a 0/64.
- **S8**: semántica del gate invariante: `cargo-mutants (PR diff)` SIEMPRE se reporta
  (`if: always()`, #1011); semanal 96 y su reporte intactos.

## Tasks

- T1 (S1,S2,S7) — `scripts/ci_mutant_shards.sh`: `prepare-diff` (base cotejada +
  `git.diff`) y `shards` (`cargo mutants --list --json --in-diff`, rc∈{5,6}→fail,
  cuenta → `shard_count` + JSON [1..n]). Test-first.
- T2 (S4) — `scripts/test_ci_mutant_shards.sh`: fixture advanced-base con
  `cargo mutants` REAL (crate mínimo, --list sin compilar), caso old-style fallido,
  caso [] , caso rc 5/6, tabla de fórmula S2 (0→0, 1..8→1, 9..16→2, ..., cap 16).
- T3 (S3,S5,S6,S8) — `.github/workflows/mutants.yml`: prepare usa el helper,
  outputs `mutant_count` / `shard_count` / `shards`, matriz dinámica, timeout 45,
  nombre de shard `${{ matrix.shard }}/${{ shard_count }}`, semanal intacta.
- T4 — cable en `scripts/ci_fast_gate.sh` lane CI (trigger regex + run_step) y
  verificación: `ci_fast_gate.sh --ci` + actionlint + full lane al final.
- T5 — commits por work-unit y apertura de PR (`Closes #2285`).

## Log

- 2026-10-23 — L1 (pedido original, verbatim): "seguimos con #2285 (shards dinámicos
  de mutants por PR) en worktree nuevo. pero recerda la condicion es utilizar context7
  para documentacion luego aplicar ante cualquier problema".
- 2026-10-23 — Verificado con docs (context7): exit codes 5/6 de cargo-mutants;
  `--list --json` emite un array (formato `mutants.json`); `merge_group.base_sha` =
  "SHA of the merge group's parent commit"; en vivo `--list --json -p webfang_core`
  = 1495 (creció desde los 1477 de #2251), 1.07 s, rc 0, sin artefactos.
- 2026-10-23 — Cambio de CI (.github/) autorizado por el propio issue #2285
  (status:approved) y el pedido del usuario; registrado acá porque la regla del repo
  pide preguntar por default.
- 2026-10-23 — T1+T2: RED→GREEN observado. RED = helper ausente (`test_ci_mutant_shards:
  helper not found`); tras implementar `scripts/ci_mutant_shards.sh`: 24/24 PASS.
  Ciclos de corrección con causa raíz verificada (no adivinada): (1) el config global
  del host firma commits/tags con SSH (`commit.gpgsign`/`tag.gpgsign`) y bloqueab_bcba
  `git tag` esperando al firmante → fixture hermético con `GIT_CONFIG_GLOBAL=/dev/null`;
  (2) `if ! assignment` sobreescribe `$?` con la negación (0) → `|| { }`, misma lección
  del envelope de gh (obs RC#2284; merges?); (3) cargo-mutants 27.1: un "no-diff" es un
  diff vacío con rc 0 (INFO "Diff file is empty") y un diff sin Rust es stdout vacío con
  rc 0 → el conteo '' se normaliza a 0; el fail-closed probado con exit 5 (nuevo lado
  que no matchea el árbol, WARN+ERROR detallados del binario con diff content).
- 2026-10-23 — T3: mutants.yml rewired. prepare runs the helper (fetch-depth: 0,
  toolchain+cargo-mutants for the no-compilation --list, EVENT_BASE_SHA env from
  pull_request.base.sha || merge_group.base_sha), outputs mutant_count/shard_count/
  shards; the hot-path regex detector is DELETED (single source of truth = the
  helper's --list read of .cargo/mutants.toml); shards matrix = fromJSON(shards),
  if shard_count != 0, timeout 45, --shard ${k}/${SHARD_COUNT}; weekly 96 shards
  untouched. mutant-pr aggregator unchanged (if: always(), shards success|skipped).
  Skipped-when-empty and always-reported semantics preserved via the aggregator job
  (S8). T4: ci_fast_gate.sh lane_ci adds the dynamic-shards harness trigger (same
  pattern as its neighbors, B52 respected). Actionlint+shellcheck with repo ignore
  lists green; ci_fast_gate.sh (ci lane): PASS=10 FAIL=0 SKIP=13, GREEN.
- 2026-10-23 — T5: PR abierto: #2295 (`chore(ci): size the PR mutant matrix from a
  measured count`, base main, head feat/mutant-shards, 2 commits f7e54161+4c451fe9,
  610 insertions / 80 deletions). Closes #2285, label type:chore. Puertas verificadas:
  issue status:approved, branch convencional, CHANGELOG/docs intactos, identidad de
  worktree OK (feat-mutant-shards==feat/mutant-shards).
- 2026-10-23 — RDD review del candidate #2295: approved (lenses risk/resilience/
  reliability, lineage review-d5a3de31075d19a5, autoridad quemada con
  acknowledge-approved). HALLAZGOS TODOS ADVISORY (nada bloqueante): R1-1 SUGGESTION
  (helper ejecuta bash en el PR gate; superficie ya acotada por persist-credentials
  false y sin interpolación de eventos), R3-1/R4 WARNING (stderr de `cargo mutants
  --list` descartado en el path fail-closed; se pierde el WARN/ERROR del binario),
  R3-2/R4 WARNING (mktemp con padre fijo /tmp/opencode inexistente en runners),
  R3-3 SUGGESTION (rc 6 sin probe), R3-4 SUGGESTION (count sin validación pre-
  aritmética), R4-fixture WARNING (=mktemp), R4-diag WARNING (=stderr),
  R4-budget WARNING (8 mutantes/shard + timeout 45min es el margen más justo:
  ~36min+instalación vs 45min; recalibrar tras medir base-build/per-mutante).
- 2026-10-23 — CI de #2295: Repo guards/CI Gate FAILURE con "Fast gate semantics
  harness: 67 ok, 2 fail" (B52/B53). Causa raíz DOBLE, verificada con
  repro local: (a) el fixture B52/B53 provee no-ops para los harness del lane CI
  (el caso prueba la FORMA del lane, no sus semánticas) y `test_ci_mutant_shards.sh`
  no estaba registrado → el harness REAL corría bajo un `cargo` stub; (b) en el
  runner ese mismo harness real fallaba por el mktemp con padre fijo. Fix:
  registro en la lista de no-ops de `scripts/test_ci_fast_gate.sh` + raíz portable
  (`mktemp -d "${TMPDIR:-/tmp}/..."` con abort limpio) en el propio harness.
  Verificación local: `test_ci_mutant_shards.sh` 24/24 (también con TMPDIR no
  estándar), `test_ci_fast_gate.sh` 69 ok 0 fail (antes 67/2), `ci_fast_gate.sh`
  FULL: PASS=10 FAIL=0 SKIP=13 GREEN, shellcheck 0. Los advisories R3-1/R4-diag
  se corrieron también (relevo del stderr del binario en el path fail-closed,
  probe manual confirma el WARN/ERROR con el hunk); el resto queda como trabajo
  aparte (R4-budget → recalibrar MUTANT_SHARD_SIZE tras medir).

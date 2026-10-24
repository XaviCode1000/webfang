# L4 — Residuales de guard post-#2068 — Tasks

Spec: `odd/tasks/guard-residuals.md` (tres residuales de #2070, sus harnesses de negación y
la decisión de no hacer nada en el ítem 4). Plan padre: `odd/tasks/backlog-implementation-plan.md`
(vive en el branch del Lote 2, `fix/merge-when-green-merged-state`).

- [x] T1 · S2-S3 — ítem 1 (#2070): la monotonicidad semver se compara en base 10 (`10#`) en
  los tres componentes — `scripts/bump-support-patch.sh`; harness: caso nuevo en
  `scripts/test_bump_support_patch.sh` (RED: error octal de bash + bump hacia atrás
  commiteado → GREEN 58/0).
- [x] T2 · S2-S3 — ítem 2 (#2070): `harness_wired` ignora las líneas de comentario completo,
  como `guard_refs_in`/`script_refs_in` — `scripts/test_guard_coverage.sh`; harness: caso 13
  en `scripts/tests/test_guard_coverage.sh` (RED: exit 0 → GREEN 13/13).
- [x] T3 · S2-S3 — ítem 3 (#2070): `MAIN_TARGET` se parsea con `parse_envrc_target_decl` —
  `scripts/check_target_isolation.sh`; harness: caso 19 en
  `scripts/tests/test_check_target_isolation.sh` (RED: `reason=main-unbootstrapped` sobre una
  declaración indentada → GREEN PASS).
- [x] T4 · S1-S5 — ítem 4 (#2070) documentado como **"no hacer nada"** (D13): la ventana de
  publicación pendiente es angosta, numérica y se auto-cura con el tag; un expiry agrega
  estado y una falla nueva por un riesgo que hoy no existe.
- [x] T5 · S1-S5 — doc del lote + este espejo + commits de work-unit en `fix/guard-residuals`
  (sin PR: la abre el owner cuando el plan entero esté implementado).

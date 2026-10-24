# L5 — Atribución de commits: identidades exactas — Tasks

Spec: `odd/tasks/ai-attribution-exact.md` (#1995: el allowlist de co-autores pasa de prefijo a
identidad exacta, con el caso de harness que lo prueba). Plan padre:
`odd/tasks/backlog-implementation-plan.md` (vive en el branch del Lote 2,
`fix/merge-when-green-merged-state`).

- [x] T1 · S1-S4 — allowlist de identidades **exactas**, medidas sobre la historia de `main`
  (`release-plz-tu-repo[bot]` x20, `github-actions[bot]` x8; más `renovate[bot]` y
  `dependabot[bot]` como decisión explícita); la comparación pasa a igualdad del nombre
  completo, case-insensitive, con el valor recortado.
- [x] T2 · S1, S4-S5 — 4 casos de rechazo nuevos en `scripts/test_no_ai_attribution.sh`
  (RED pre-fix: `22 passed, 4 failed`, con el guard respondiendo "ningún commit lleva
  atribución de AI" para el nombre humano que empieza con un token → GREEN `26 passed, 0 failed`).
- [x] T3 · S1-S6 — verificación: harness 26/0 (re-corrido por el padre), shellcheck limpio
  con la lista de ignorados de CI, tres sondas de fixture (identidad legítima con espacio
  final → aceptada; con email → aceptada; nombre humano → rechazada). Sin `cargo`.
- [x] T4 · S2-S6 — registro en `odd/tasks/ai-attribution-exact.md` (identidades medidas,
  evidencia RED/GREEN, sondas y riesgos) y este espejo al día.
- [x] T5 · S1-S5 — commits de work-unit en `fix/ai-attribution-exact` (sin PR: la abre el
  owner cuando el plan entero esté implementado).

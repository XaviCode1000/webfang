# L11 — Obscura: el veredicto `Unknown` nombra el mínimo esperado — Tasks

Spec: `odd/tasks/obscura-unknown-verdict.md` (#1710: el veredicto `Unknown` del preflight
nombra el mínimo esperado; sin modo estricto, sin contadores). Plan padre:
`odd/tasks/backlog-implementation-plan.md` (vive en el branch del Lote 2,
`fix/merge-when-green-merged-state`).

- [ ] T1 · S2, S3 — el warning del veredicto `Unknown` nombra el mínimo esperado
  (`MINIMUM_OBSCURA_VERSION_STR`), en español; el veredicto sigue siendo un degradado, no un
  error.
- [ ] T2 · S4 — test in-module `unknown_verdict_names_the_expected_minimum`: RED pre-fix →
  GREEN post-fix.
- [ ] T3 · S8 — verificación: suite de preflight en verde con build acotado (`CARGO_BUILD_JOBS=1`,
  `systemd-run --user --scope -p MemoryMax=2G`), `cargo fmt --all -- --check` y clippy del
  paquete.
- [ ] T4 · S1, S6, S7 — registro en `odd/tasks/obscura-unknown-verdict.md` + este espejo, y
  comentario de cierre parcial en #1710 (el pin 0.2.3, los docs y los contadores quedan
  fuera de este lote y el issue **no** se cierra).
- [ ] T5 · S1-S8 — commit de work-unit en `fix/obscura-unknown-verdict` (sin PR: la abre el
  owner cuando el plan entero esté implementado).

# L14 — Docs y runbooks: la recuperación de WAL/SQLite y la nota de Obscura — Tasks

Spec: `odd/tasks/docs-runbooks-wal.md` (#1609, #2061, #1826, #1710: el runbook de recuperación
de WAL/SQLite que no existe en ningún lado, la nota de la variante de build de Obscura, y el
triage de las tres issues de documentación). Plan padre:
`odd/tasks/backlog-implementation-plan.md` (vive en el branch del Lote 2,
`fix/merge-when-green-merged-state`).

- [ ] T1 · S3, S4 — runbook de recuperación de WAL/SQLite en `docs/src/` con entrada en
  `docs/src/SUMMARY.md`; **cada afirmación con el test que la respalda** citado, y ninguna
  afirmación sin test.
- [ ] T2 · S5 — nota de #1710: `docs/src/cli-reference.md` y el comentario del módulo
  corregidos a "L2 = solo render barato" (decisión del padre), con las cuatro consecuencias:
  hit-rate derivado de los logs por capa, el veredicto `Unknown` nombrando el mínimo, el piso
  0.2.5 y #1710 sin cerrar.
- [ ] T3 · S6, S7 — evidencia para que el padre publique: veredicto por fila de #1609 (leído
  por REST), el argumento de #2061 (rechazar el auto-PR de rotación) y los comentarios
  redactados de #1826 y #1710. El worker **no** escribe en GitHub.
- [ ] T4 · S1-S8 — verificación: los archivos existen, el SUMMARY enlaza, `cargo fmt --all --
  --check` si hubo `.rs`, y ninguna otra página del libro queda con una promesa que el runbook
  contradiga.
- [ ] T5 · S1-S8 — commit de work-unit en `fix/docs-runbooks-wal` (sin PR: la abre el owner
  cuando el plan entero esté implementado).

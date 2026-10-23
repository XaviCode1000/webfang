# L6 — Señal de mutación: conteo medido y matriz rebalanceada — Tasks

Spec: `odd/tasks/mutation-shard-measurement.md` (#1844: el total de mutantes del scope vigente
se mide y la matriz de shards se rebalancea con ese número). Plan padre:
`odd/tasks/backlog-implementation-plan.md` (vive en el branch del Lote 2,
`fix/merge-when-green-merged-state`).

- [x] T1 · S2, S6 — medición con `cargo mutants --list` (cargo-mutants 27.1.0): **1477**
  mutantes en el scope vigente, 0,71 s, cero compilaciones.
- [x] T2 · S3 — decisión con el precedente #1436: 1477/64 = 23,1 por shard (por encima del
  draw de ~20 que tocó el muro de 90 min) → rebalance; 1477/96 = 15,4 por shard.
- [x] T3 · S4, S5 — `.github/workflows/mutants.yml`: matriz 1..96, `--shard k/96`, nombre del
  job, advertencia y comentarios; `.cargo/mutants.toml`: el shard count y el número medido.
  El `~600` estimado se reemplaza por el número medido.
- [x] T4 · S1-S6 — verificación: YAML válido, `actionlint` limpio con la lista de ignorados de
  CI, 96 entradas en la matriz, flag y nombre del job en /96.
- [x] T5 · S1-S6 — commits de work-unit en `chore/mutation-shard-measurement` (sin PR: la abre
  el owner cuando el plan entero esté implementado).

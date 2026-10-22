# Plan de implementación del backlog — Tasks

Spec: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes, reglas de ejecución, mapa de colisiones, decisiones del agente y del owner).

**El espejo es un libro mayor de un solo escritor**: lo actualiza el owner del plan (el agente
que hizo el triage) cuando cada lote aterriza, con el sha de su branch. Ningún lote edita
`odd/backlog-implementation-plan/**`: cada lote deja su propio doc de feature en su branch
(`odd/tasks/<lote>.md` + `odd/<lote>/tasks.md`) y su evidencia ahí. El plan y su espejo viven
**sólo** en el branch de L2 (`66f23d63`): no se duplican en ningún otro branch.

## Ownership (2026-10-22)

| Parte | Dueño |
|---|---|
| Specs, libro mayor, lado issues (comentarios/cierres), reviews | este agente |
| Implementación de L7, L8, L9, L10, L11, L14, L15 | el peer |

## Estado por lote

- [x] **L0** · S1–S5 — cierres por evidencia (#2105, #1996, #1423, #2224) + comentarios (#1936, #1826) + 3 issues nuevas (#2233, #2234, #2235).
- [ ] **L1** · S1–S5 — SSRF: encapsulación IPv4 (#2231, #2232) — **OCUPADO** por el agente del owner (worktree `fix-ssrf-this-network-justification`).
- [x] **L2** · S1–S5 — merge y limpieza post-merge (#1970, #1971, #1972) — 3 commits en `fix-merge-when-green-merged-state` (sin PR); harness 25/0, breaking 26/0, ci 17/0, topology verde.
- [x] **L3** · S1–S5 — el canal del CI Health enmudecido (#2233, #2234, #2162, #1936, #1980).
  - **L3a HECHO**: `1b49ef71` + `eaa172d7` en `fix-ci-health-w03-classifier` (harness 104/0, RED 99/5, semántica de la búsqueda contra la API real; harness partido 468+474; #2162 y #1980 cerrados). **Review nativo APROBADO** (`review-7279a44bd00f4c9f`, 3 hallazgos no bloqueantes).
  - **L3b HECHO**: `b08323cd` en `fix-2234-hermetic-args` — `parse_args` borrado, un solo entry point hermético, test `the_test_parse_surface_ignores_the_ambient_environment`; 2963 tests verdes **con** el env venenoso de la lane + control negativo.
- [x] **L4** · S1–S5 — residuales de guard (#2070: ítems 1-3; el 4º, no hacer nada) — 7 commits en `fix-guard-residuals` (peer; tip `5f19fbb5`). **Review nativo ABIERTO** (`review-ead5e692fc9ea607` + `review-74427c00c7b8343a`): el ack se rechaza por coacción del host aunque el binding sea byte-idéntico al canónico. No bloquea la entrega.
- [x] **L5** · S1–S5 — atribución de commits (#1995) — `0f004bf4` doc + `4a17cb66` código en `fix-ai-attribution-exact` (tip `0f004bf4`).
- [x] **L6** · S1–S5 — señal de mutación (#1844) — `01767192` doc + `26792e6b` código en `chore-mutation-shard-measurement` (tip `01767192`): 1477 mutantes medidos ⇒ 96 shards.
- [x] **L7** · S1–S5 — MCP: superficie verdadera (#1815, #1817, #1837, #2193) — peer: `dccd06da`, `c40fac93`, `80be9903`, `315441ab` (tip `315441ab`).
- [x] **L8** · S1–S5 — falsos verdes de test (#2066, #1852, #1828, #1833, #1834, #1816) — spec `da23c097` (padre) + implementación del peer (tip `99833e11`, 15 commits).
- [x] **L9** · S1–S5 — grafo de build (#1799, #1797, #1796) — spec `eb40ecbb` (padre) + implementación del peer (tip `713502cd`). D9 corregida: `sys-info` devuelve KiB en todas las plataformas.
- [x] **L10** · S1–S5 — crates sin mantenimiento (#1984, #2031) — spec `2f51441a` (padre) + implementación del peer (tip `71a2d02e`, 7 commits).
- [x] **L11** · S1–S5 — obscura: el veredicto `Unknown` nombra el mínimo (#1710) — peer: `0e52f031`, `6b82e087`, `0747fea3` (tip `0747fea3`).
- [ ] **L12** · S1–S5 — duplicación: helpers de arrange + baseline medido (#1757) — espera **#2230** (sigue abierta).
- [ ] **L13** · S1–S5 — entrega macOS (#1821) — **DESCARTADO 2026-10-22** (O3): *"la issue 1821 la dejamos, no la tocamos"*. Sale del plan; la issue queda abierta y sin tocar.
- [ ] **L14** · S1–S5 — docs y runbooks.
  - **Mi parte HECHA**: veredicto por fila de #1609 posteado (25 filas: 12 resueltas, 7 moot, 5 vivas, 1 no accionable) + **#2241** (doc de disclosure), **#2242** (degradación de config invisible), **#2243** (podar el seam muerto) abiertas y aprobadas. #1609 queda como umbrella. Comentarios del lado issues posteados: #2061, #1826, #1710 y el addendum de #1609.
  - **Del peer HECHO**: `f79e42ba`, `08d46907`, `0616c3bf`, `da990e2d`, `cc380657` (tip `cc380657`, 5 commits): runbook WAL/SQLite, nota de #1710, carry-over del wiki, evidencia. Sin PR ni publicación en GitHub; no toca `ci.yml`.
- [x] **L15** · S1–S5 — lane de UB: gate por test + skip-net justificado (#2235) — peer: `6065049e`, `cf4403fa`, `f0e39852`, `4a9cb8c0` (tip `4a9cb8c0`). Miri ya instalado (O8 resuelta).
- [ ] **L16** · S1–S5 — cuando el owner lo ordene: abrir las PRs lote por lote (issue enlazada, un `type:*`, sin atribución de IA) y dejar el merge en sus manos. **Orden fijado 2026-10-22**: `L2 → L3a → L3b → L4/L5/L6 → L8/L9/L10`.

## Decisiones del owner (2026-10-22, segunda tanda)

| Decisión | Resolución |
|---|---|
| ADR-0013 (`#2243`) | **addendum fechado** en el propio ADR; el registro histórico no se reescribe |
| O7 (orden de PRs) | `L2 → L3a → L3b → L4/L5/L6 → L8/L9/L10`, cada slice desde el `main` nuevo |
| O3 (`#1821`, L13) | **descartado**: la issue se deja sin tocar; el lote sale del plan |
| PRs abiertas (2026-10-22) | 2: `#2240` (release v5.0.1, CLEAN) y `#2230` (exporter, BLOCKED) |

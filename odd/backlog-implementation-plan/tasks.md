# Plan de implementación del backlog — Tasks

Spec: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes, reglas de ejecución, mapa de colisiones, 17 decisiones del agente, 7 decisiones del owner).

**El espejo es un libro mayor de un solo escritor**: lo actualiza el owner del plan (el agente
que hace el triage) cuando cada lote aterriza, con el sha de su branch. Ningún lote edita
`odd/backlog-implementation-plan/**`: cada lote deja su propio doc de feature en su branch
(`odd/tasks/<lote>.md` + `odd/<lote>/tasks.md`) y su evidencia ahí. El plan y su espejo viven
**sólo** en el branch de L2 (`66f23d63`): no se duplican en ningún otro branch.

- [x] L0 · S1–S5 — cierres por evidencia (#2105, #1996, #1423, #2224) + comentarios (#1936, #1826) + 3 issues nuevas (#2233, #2234, #2235).
- [ ] L1 · S1–S5 — SSRF: encapsulación IPv4 (#2231, #2232) — **OCUPADO por el agente del owner** (worktree `fix-ssrf-this-network-justification`).
- [x] L2 · S1–S5 — merge y limpieza post-merge (#1970, #1971, #1972) — 3 commits en `fix-merge-when-green-merged-state` (sin PR); harness 25/0, breaking 26/0, ci 17/0, topology verde.
- [ ] L3 · S1–S5 — el canal del CI Health enmudecido (#2233, #2234, #2162, #1936, #1980) — **L3a HECHO**: `1b49ef71` + `eaa172d7` en `fix-ci-health-w03-classifier` (verificado por el padre: harness 104/0, RED 99/5, semántica de la búsqueda contra la API real; harness partido 468+474; #2162 y #1980 cerrados). **L3b** (#2234, hermeticidad) en curso: RED real capturado con el env venenoso, fix delegado.
- [ ] L4 · S1–S5 — residuales de guard (#2070: ítems 1-3; el 4º, no hacer nada) — asignado al peer de coordinación; superficies reales: `scripts/bump-support-patch.sh`, `scripts/test_guard_coverage.sh`, `scripts/check_target_isolation.sh`.
- [ ] L5 · S1–S5 — atribución de commits (#1995) — asignado al peer de coordinación (después de L4).
- [ ] L6 · S1–S5 — señal de mutación (#1844).
- [ ] L7 · S1–S5 — MCP: superficie verdadera (#1815, #1817, #1837, #2193).
- [ ] L8 · S1–S5 — falsos verdes de test (#2066, #1852, #1828, #1833, #1834, #1816) — BLOQUEADO por O1 y O4.
- [ ] L9 · S1–S5 — grafo de build (#1799, #1797, #1796) — BLOQUEADO por O2 y O6.
- [ ] L10 · S1–S5 — crates sin mantenimiento (#1984, #2031) — BLOQUEADO por O2.
- [ ] L11 · S1–S5 — obscura: el veredicto `Unknown` nombra el mínimo (#1710).
- [ ] L12 · S1–S5 — duplicación: helpers de arrange + baseline medido (#1757) — después de #2230.
- [ ] L13 · S1–S5 — entrega macOS: notarización no bloqueante, `xattr` intacto (#1821) — BLOQUEADO por O3.
- [ ] L14 · S1–S5 — docs y runbooks: #1609 (veredicto por fila + ≤2 slices), #2061 (runbook, sin bot), runbook WAL/SQLite (#1826).
- [ ] L15 · S1–S5 — lane de UB: gate por test + skip-net justificado (#2235).
- [ ] L16 · S1–S5 — cuando el owner lo ordene: abrir las PRs lote por lote (issue enlazada, un `type:*`, sin atribución de IA) y dejar el merge en sus manos.

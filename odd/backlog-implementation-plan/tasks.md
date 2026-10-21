# Plan de implementación del backlog — Tasks

Spec: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes, reglas de ejecución, mapa de colisiones, 17 decisiones del agente, 7 decisiones del owner).

- [x] L0 · S1–S5 — cierres por evidencia (#2105, #1996, #1423, #2224) + comentarios (#1936, #1826) + 3 issues nuevas (#2233, #2234, #2235).
- [ ] L1 · S1–S5 — SSRF: encapsulación IPv4 (#2231, #2232) — **OCUPADO por el agente del owner** (worktree `fix-ssrf-this-network-justification`).
- [ ] L2 · S1–S5 — merge y limpieza post-merge (#1970, #1971, #1972) — en curso en `fix-merge-when-green-merged-state`.
- [ ] L3 · S1–S5 — el canal del CI Health enmudecido (#2233, #2234, #2162, #1936, #1980).
- [ ] L4 · S1–S5 — residuales de guard (#2070: ítems 1-3; el 4º, no hacer nada).
- [ ] L5 · S1–S5 — atribución de commits (#1995).
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

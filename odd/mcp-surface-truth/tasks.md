# L7 — MCP: la superficie verdadera — Tasks

Spec: `odd/tasks/mcp-surface-truth.md` (#1815, #1817, #1837, #2193: la superficie MCP que el
repo anuncia tiene que ser la que el repo sostiene). Plan padre:
`odd/tasks/backlog-implementation-plan.md` (vive en el branch del Lote 2,
`fix/merge-when-green-merged-state`).

- [x] T1 · S2 — la key `mcp` de `webfang_cli` queda como `mcp = []` (marcador explícito), no se
  borra: `release.yml`, `docs.yml`, `check_compatibility.sh`, `gen-cli-reference.sh` y los
  third-party notices siguen funcionando (D6).
- [x] T2 · S3, S7 — el path gate usa `eq_ignore_ascii_case` (ASCII-narrow) y el caso nombrado
  `components_start_with_never_accepts_a_case_variant_the_filesystem_keeps_distinct` prueba que
  no acepta la variante que el filesystem mantiene distinta; la limitación (root macOS
  no-ASCII) queda documentada (D7).
- [x] T3 · S4 — el conteo de 36 tools MCP y el de crates en **una sola fuente**, con la
  aserción del conteo exacto en `server.rs`.
- [x] T4 · S1, S5 — `match_url_pattern` documenta los modos que existen (o el doc comment se
  corrige a los que existen).
- [x] T5 · S6 — la constante de shutdown pasa a `pub` y el mensaje de termination source deja
  de duplicarse (#2193).
- [x] T6 · S1-S9 — verificación: tests de `webfang_mcp` en verde, `cargo fmt --all -- --check`,
  clippy del paquete, y el handshake que pina el conteo (36) sigue verde con la misma cifra.
- [ ] T7 · S8, S9 — commit de work-unit en `fix/mcp-surface-truth` (sin PR: la abre el owner
  cuando el plan entero esté implementado). El build va con jobs=1, `systemd-run --scope -p
  MemoryMax=3G` y **un build a la vez en la máquina** (se espera la ventana, detrás de L11).

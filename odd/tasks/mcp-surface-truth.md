# L7 — MCP: la superficie verdadera (#1815, #1817, #1837, #2193)

Plan padre: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes; vive en el branch
del Lote 2, `fix/merge-when-green-merged-state`, y viaja con él a `main`).
Este doc es la especificación de **un** lote: la superficie MCP que el repo **anuncia** tiene
que ser la superficie que el repo **sostiene**. Sin PR: el trabajo queda commiteado en el
worktree.

Rama: `fix/mcp-surface-truth` · Worktree: `~/Projects/Rust/webfang-worktrees/fix-mcp-surface-truth`
Issues: #1815, #1817, #1837, #2193 (los tres primeros con `status:approved`).

---

## Specs

**S1.** El plan padre define el lote (verbatim de su tabla):

> **L7** | MCP: superficie verdadera | #1815, #1817, #1837, #2193 | `match_url_pattern` documenta
> lo que hace; `mcp = []` (la key **no** se borra); el conteo 36 en una sola fuente;
> `eq_ignore_ascii_case` en el path gate; la constante de shutdown `pub` |
> `path_gate.rs::components_start_with_never_accepts_a_case_variant_the_filesystem_keeps_distinct`
> | +18/−10 | worker | PENDIENTE

**S2.** `mcp = []` y **no** borrar la key (D6 del plan padre): la key `mcp` de `webfang_cli` es
load-bearing — la usan `release.yml`, `docs.yml`, `check_compatibility.sh`,
`gen-cli-reference.sh` y los third-party notices. Borrarla rompe cinco consumidores; dejarla
como marcador vacío hace lo mismo de forma explícita.

**S3.** `eq_ignore_ascii_case` (D7 del plan padre), y no el case-folding Unicode: el par que
sobre-acepta con Unicode es `ß`/`ẞ` (Unicode los pliega; NTFS y APFS los distinguen).
ASCII-narrow es estrictamente más estricto que ambos filesystems ⇒ falla cerrado. La
limitación se documenta: un root de macOS con nombre no-ASCII queda rechazado.

**S4.** El conteo de herramientas MCP (36) vive en **una sola fuente**, y el conteo de crates
también (AC de #1817: "Crate and MCP tool counts are correct everywhere, and `server.rs:1141`
asserts the exact pinned count"). Hoy el conteo está pinado en
`crates/webfang_mcp/tests/stdio_handshake.rs`; la superficie lo replica en otro lado, y dos
números que deben coincidir son dos números que se divergen.

**S5.** `match_url_pattern` **documenta lo que hace** (AC de #1815: "Path / path-glob / host
pattern modes are implemented, or the doc comment describing them is corrected"). Un doc
comment que promete modos que no existen es la misma clase de defecto que un conteo mentiroso.

**S6.** La constante de shutdown `pub` (#2193: el mensaje de registro de termination source
rechazado duplica el de los otros dos sitios): una constante usada por tres sitios no es un
detalle de un mensaje, es la fuente de la duplicación.

**S7.** El test nombrado por el plan es
`path_gate.rs::components_start_with_never_accepts_a_case_variant_the_filesystem_keeps_distinct`:
un caso que prueba que el path gate **no** acepta una variante de mayúsculas que el sistema de
archivos mantiene distinta. RED contra el código pre-fix, GREEN después.

**S8.** **El build de este lote es NORMAL** (verificado, y corrige una premisa del plan): el
workspace **no tiene** feature `ai` en ninguna crate, y **no tiene** dependencia `ort` — el
embedding local ONNX fue eliminado en ADR-0004 paso 6 (lo dice el propio comentario de
`webfang_mcp`: "The local ONNX embedding adapter is gone (ADR-0004 paso 6)"). El feature set
real de `webfang_core` es `default = []` (explícito y vacío) más `loom-model`, `persistence`,
`console`, `dev-tracing`, `adaptive-selectors`, `mcp` y `chromium`. No hay ONNX, ni `ort`, ni
un runtime que compilar: un build de `webfang_mcp` es un build de crate normal. Aun así: un
build a la vez en la máquina (regla 3 del plan padre) y el protocolo acotado (jobs=1,
`systemd-run --scope -p MemoryMax=3G`).

**S9.** Paths de este lote: `crates/webfang_mcp/**` y `crates/webfang_cli/Cargo.toml` (la key
`mcp` del CLI). No toca `docs/`, ni `CHANGELOG.md`, ni `.github/workflows/ci.yml`.

---

## Tasks

- [ ] T1 · S2, S9 — la key `mcp` de `webfang_cli` queda como `mcp = []` (marcador explícito), no
  se borra: sus cinco consumidores siguen funcionando.
- [ ] T2 · S3, S7 — el path gate compara con `eq_ignore_ascii_case` (ASCII-narrow) y el caso
  nombrado prueba que no acepta la variante que el filesystem mantiene distinta; la
  limitación (root macOS no-ASCII) queda documentada.
- [ ] T3 · S4 — el conteo de 36 tools (y el de crates) en **una sola fuente**, con la aserción
  del conteo exacto en `server.rs`.
- [ ] T4 · S1, S5 — `match_url_pattern` documenta lo que hace: los modos que existen, o el
  doc comment corregido a los que existen.
- [ ] T5 · S6 — la constante de shutdown pasa a `pub` y el mensaje de termination source deja
  de duplicarse.
- [ ] T6 · S1-S9 — verificación: tests de `webfang_mcp` en verde (con el build acotado de S8),
  `cargo fmt --all -- --check`, clippy del paquete, y **diferencial**: el handshake que pinea
  el conteo sigue verde con la misma cifra.

Ruta: **worker** (`gentle-ai-worker`): cinco ítems independientes en un crate, cada uno con su
test. Lanza cuando haya ventana de build (un build a la vez en la máquina).

---

## Log

L1 — Petición original del owner (verbatim, del lote padre):

> coordena con el otro agente para implementar el plan ODD del backlog completo →
> odd/tasks/backlog-implementation-plan.md (+ espejo odd/backlog-implementation-plan/tasks.md),
> commiteado en el branch del Lote 2 para que viaje con él y no quede huérfano en main (justo
> la regla que el propio lote establish).

> recuerda que debeis tu y el otro agente tener cuidado siempre en no ejecutar comandos
> pesados , para no saturar mi sistema, y el trabajo se guarda en worktree sin abrir pr's
> hasta que no este todo el plan implementado.

L2 — Reparto con el otro agente (sesión `01a1607b`): él toma **L3 y sólo L3**. L7, L8 y L15
quedaron de mi lado. L7 no colisiona con L3 (`crates/webfang_mcp/**` + el `Cargo.toml` del
CLI, contra `ci-health.yml`/`test_ci_health_close.sh`/`ci-health-close.sh`).

L3 — **Corrección de una premisa del plan** (verificada contra los manifiestos): el plan
trataba L7 como un lote de build pesado por "AI local". No lo es. `webfang_core` declara
`default = []` (explícito y vacío) y **ninguna** feature `ai`; `webfang_mcp` no tiene `ort` ni
onnxruntime; el embedding local fue eliminado en ADR-0004 paso 6. El build de este lote es un
build de crate normal. La consecuencia práctica: L7 no necesita esperar a una ventana
"de máquina libre" como se suponía, sólo a la ventana de **un build a la vez**.

L4 — Correcciones del owner del plan al plan que afectan a este lote: el plan nombraba
`scripts/tests/test_guard_coverage.sh` donde en realidad viven dos archivos (L4 ya lo
resolvió); para L7 el plan no nombraba paths de archivo, así que este doc los fija (S9).

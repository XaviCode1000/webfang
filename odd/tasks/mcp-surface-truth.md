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

- [x] T1 · S2, S9 — la key `mcp` de `webfang_cli` queda como `mcp = []` (marcador explícito), no
  se borra: sus cinco consumidores siguen funcionando.
- [x] T2 · S3, S7 — el path gate compara con `eq_ignore_ascii_case` (ASCII-narrow) y el caso
  nombrado prueba que no acepta la variante que el filesystem mantiene distinta; la
  limitación (root macOS no-ASCII) queda documentada.
- [x] T3 · S4 — el conteo de 36 tools (y el de crates) en **una sola fuente**, con la aserción
  del conteo exacto en `server.rs`.
- [x] T4 · S1, S5 — `match_url_pattern` documenta lo que hace: los modos que existen, o el
  doc comment corregido a los que existen.
- [x] T5 · S6 — la constante de shutdown pasa a `pub` y el mensaje de termination source deja
  de duplicarse.
- [x] T6 · S1-S9 — verificación: tests de `webfang_mcp` en verde (con el build acotado de S8),
  `cargo fmt --all -- --check`, clippy del paquete, y **diferencial**: el handshake que pinea
  el conteo sigue verde con la misma cifra.

Ruta: **worker** (`gentle-ai-worker`): cinco ítems independientes en un crate, cada uno con su
test. Lanza cuando haya ventana de build (un build a la vez en la máquina).

---

## Log

L0 — Estado de los ítems al cerrar la implementación (el detalle de la evidencia va en L5):
T1 ✅ · T2 ✅ · T3 ✅ · T4 ✅ · T5 ✅ · T6 ✅. T7 (el commit) lo cierra el owner: el worker no
commitea.

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

L5 — **Implementación del lote en el worktree `fix/mcp-surface-truth`** (sin commit: el owner
commitea el corte; T5 va en su propio work-unit porque toca `webfang_core`). Estado por ítem:

- **T1 (S2, S9) — HECHO.** `crates/webfang_cli/Cargo.toml`: `mcp = ["webfang_core/mcp"]` →
  `mcp = []`. La key NO se borra: `crates/webfang_core/tests/common/cli_harness.rs` construye el
  binario del CLI con `--features mcp` (y `release.yml`, `docs.yml`, `check_compatibility.sh`,
  `gen-cli-reference.sh` y los third-party notices hacen lo mismo), así que borrarla rompe esos
  consumidores. `webfang_core/mcp` no gatea ningún sitio de PRODUCCIÓN; los que quedan son las
  suites de test de `webfang_mcp` (con su feature PROPIO) y ese harness.
- **T2 (S3, S7) — HECHO.** `path_gate.rs::components_start_with` compara con `eq_ignore_ascii_case`
  sobre los BYTES del componente (`OsStr::as_encoded_bytes`), no con el `to_lowercase` de Unicode:
  el folding Unicode no es el modelo de caso de NTFS/APFS (`ß` U+00DF y `ẞ` U+1E9E son una
  variante para Unicode y dos nombres para el filesystem). ASCII-narrow es estrictamente más
  estrecho que ambos ⇒ falla cerrado. La limitación (un root de macOS con nombre no-ASCII sólo casa
  con su grafía exacta) queda escrita en el doc comment.
- **T3 (S4) — HECHO.** El conteo vive en UNA sola fuente: `webfang_mcp::mcp_server::MCP_TOOL_COUNT`
  (`pub`, 36). `server.rs` aserta el conteo EXACTO (antes: piso `>= 35`); el handshake stdio, el
  gate de procedencia y el `long_about` de los dos binarios lo leen desde ahí. Los "35" sueltos:
  `docs/manual/PLAN_PRUEBAS_REALES.md` (161 y la fila 6.4) → 36; `scripts/build-llm-artifact.sh`
  deja de NOMBRAR un número (un número escrito a mano es una segunda fuente); `docs/src/overview.md`
  lo corrigió L14 en su rama y NO se toca aquí.
- **T4 (S1, S5) — HECHO.** La descripción de `match_url_pattern` dice lo que hace: el patrón se
  compara contra la URL ENTERA como una cadena (glob anclado si trae metacaracter, subcadena si
  no) y NO existen modos host-only ni path-only. Gate nuevo en `mcp_schema_compat_test.rs`.
- **T5 (S6) — HECHO** (el owner amplió la superficie a
  `crates/webfang_core/src/application/crawler/engine.rs`): `TERMINATION_REGISTRATION_REJECTED`
  pasa a `pub` y el servidor MCP la adopta ⇒ un texto para un hecho, tres sitios (engine, CLI, MCP).
- **T6 (S1-S9) — HECHO.** Evidencia abajo.

Evidencia (protocolo acotado de S8: `$(rustup which cargo)`, `CARGO_BUILD_JOBS=1`,
`CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=~/.cache/cargo-target/fix-mcp-surface-truth`,
`systemd-run --user --scope -p MemoryMax=3G`; el seed respondió `seed: cold reason=no-seed`):

- **RED** — `cargo nextest run -p webfang_mcp --lib components_start_with_never_accepts_a_case_variant_the_filesystem_keeps_distinct`
  (la producción seguía con el folding Unicode):

      thread 'mcp_server::path_gate::tests::components_start_with_never_accepts_a_case_variant_the_filesystem_keeps_distinct'
      panicked at crates/webfang_mcp/src/mcp_server/path_gate.rs:834:9:
      a non-ASCII case variant must not be accepted: the comparison is ASCII-narrow, so a name
      the filesystem may keep distinct is rejected instead of accepted (fail-closed, #1837)

  ⇒ falla por el motivo del defecto (el folding Unicode acepta la variante), no por un stub.
- **GREEN** — el mismo comando tras el cambio: `1 passed, 367 skipped`.
- **Handshake** (pina el conteo) — `cargo nextest run -p webfang_mcp --features mcp --test stdio_handshake`:
  9/9 verdes, incluido `stdio_tools_list_reports_the_pinned_tool_registry` (misma cifra: 36).
- **Crate completo** — `cargo nextest run -p webfang_mcp --features mcp`: `637 tests run: 637 passed,
  0 skipped`. Con `--features mcp` corren los 23 ficheros de test que la feature PROPIA de
  `webfang_mcp` gatea; sin ella no existen. (Dos suites CLI↔MCP necesitan además el binario
  `webfang` en el target dir: sin `cargo build -p webfang_cli` fallan por eso —"webfang binary not
  found"— y no por el cambio.)
- **Clippy** — `cargo clippy -p webfang_mcp -p webfang_cli --all-targets --all-features -- -D warnings
  -W clippy::cognitive_complexity -W clippy::too_many_lines`: rc=0.
- **Fmt** — `cargo fmt --all -- --check`: rc=0.
- **Diferencial** — el CLI compilado con `--features mcp` y el compilado con las features por
  defecto dan stdout, stderr y código de salida IDÉNTICOS (la opción es inerte, como antes);
  `cargo tree -p webfang_cli` da un árbol byte-idéntico con `--features mcp`, con
  `--features webfang_core/mcp` (el efecto del manifiesto viejo) y sin features; el `--help` de los
  dos binarios MCP conserva la línea "Exposes 36 tools …" byte-idéntica.

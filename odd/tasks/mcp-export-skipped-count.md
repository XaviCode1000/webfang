# El resultado MCP del export expone las líneas saltadas por no-UTF-8 (issue #2238)

El conteo `skipped_non_utf8` existe en el export, pero no viaja a la
superficie MCP: un cliente que exporta un JSONL dañado no tiene forma de
enterarse salvo por el ERROR del log.

## Specs

S1. El resultado del tool de export del servidor MCP lleva el conteo de
    líneas saltadas por no-UTF-8. Es **aditivo**: los clientes que ya leen el
    resultado no se rompen.

S2. El ERROR estructurado sigue como estaba (ya se emite, con
    `log_scrape_error`, stage `jsonl_resume_index`, `ErrorKind::InvalidData`)
    ⇒ este slice **no** toca el logging.

S3. El conteo de tools MCP **no cambia** (36 tools, 9 categorías, pineado por
    `crates/webfang_mcp/tests/stdio_handshake.rs`): agregar un campo a un
    resultado no cambia el conteo, pero hay que **verificarlo** ⇒ el test
    que lo pinea tiene que seguir verde.

S4. El test que fija el campo nuevo: un export con una línea no-UTF-8, por
    el camino MCP, que devuelve el conteo. Sin ese test el campo es una
    promesa.

S5. La CLI **no** cambia: su nivel de consola por defecto es `warn` y el
    conteo es INFO ⇒ hace falta `-v` o `--trace-file`. Eso es intencional y
    queda documentado, no "arreglado".

## Tasks

- [x] T1 · S1, S2, S5 — `crates/webfang_mcp/src/mcp_server/handlers/export.rs`:
      el resultado del tool de export lleva el conteo de saltados; el
      provenance sidecar y el resto de la forma del resultado se mantienen
      (el sidecar tiene su propia forma: no se le agrega el campo).
- [x] T2 · S3, S4 — el test que fija el campo en el camino MCP + el test de
      que el conteo de tools no cambia.
- [x] T3 · S1–S5 — verificación: los tests de `webfang_mcp` verdes (incluido
      `stdio_handshake.rs`), `cargo check`, `cargo clippy --all-targets
      --all-features -- -D warnings`, `cargo fmt --all -- --check`, `cargo
      doc` con `RUSTDOCFLAGS=-D warnings`.

## Log

L1 — Petición del owner (verbatim, del issue #2238): "el resultado del tool
de export del MCP lleva `documents`, no el conteo de saltados. Un cliente MCP
que exporta un JSONL dañado no tiene forma de enterarse de que hubo líneas
saltadas, salvo por el ERROR en el log." Y: "Sumar `skipped_non_utf8` (y
opcionalmente `documents_skipped`) al resultado del tool de export del servidor
MCP. Es **aditivo**: no rompe a los clientes que ya leen el resultado."

L2 — Riesgo declarado por la issue, verbatim: "**Riesgo: ítem 3 (el resultado
del tool es un contrato que otros consumen) — agregar un campo es aditivo, pero
hay que fijarlo con un test.**" ⇒ de ahí sale S4.

L3 — Lo que la issue ya resuelve y NO hay que rehacer: el ERROR estructurado
se emite ✓; el nivel de consola del CLI es intencional ✓; el conteo de tools
no cambia por agregar un campo, pero hay que verificarlo ⇒ S3.

L4 — Mapa medido (por el padre): el handler del tool de export es
`crates/webfang_mcp/src/mcp_server/handlers/export.rs`; el conteo vive en
`crates/webfang_core/src/application/export_factory.rs` (el summary del
export lo expone y el CLI lo imprime en su resumen).

L5 — Decisión del padre: la PR es `Closes #2238`, label `type:chore` (la
issue lo tiene como `type:chore`) ⇒ el prefijo del título tiene que derivar
esa label: `chore(...)`.

L6 — Forma EXACTA del campo (T1): `skipped_non_utf8`, un NÚMERO JSON, que
viaja en `structuredContent` (la ranura estándar de MCP para el resultado
legible por máquina) de los CUATRO tools de export (`export_file`,
`export_jsonl`, `export_vector`, `process_export_pipeline`). Es aditivo en los
DOS sentidos: `content` no se toca y `structuredContent` es una clave NUEVA de
un objeto NUEVO ⇒ un cliente que lee el texto sigue leyendo lo mismo. Además,
cuando el conteo es > 0 el texto gana UNA línea en español ("Se saltaron
N línea(s) del archivo de exportación que no eran UTF-8 válido
(skipped_non_utf8)"), porque muchos clientes sólo exponen el texto al modelo:
sin esa línea el conteo volvería a ser invisible para ellos. Con conteo 0 el
texto es EXACTAMENTE el de antes ⇒ el camino limpio no se mueve.

L7 — De dónde sale el conteo (T1): el export phase ya contaba las líneas
no-UTF-8 al indexar el JSONL existente (#1812: `read_jsonl_checksum_index` →
`build_content_hash_index` → `CommitSession::open`) y ese conteo vive en el
ERROR estructurado (`stage = "jsonl_resume_index"`, `ErrorKind::InvalidData`),
que NO se tocó (S2). El handler MCP cuenta las MISMAS líneas, con las MISMAS
semánticas de línea (lectura en streaming; se quitan los terminadores `\n` y
`\r`; una línea vacía no es línea; una línea que no decodifica como UTF-8 es
el daño), sobre el MISMO archivo — `{output_dir}/{filename}.jsonl`, el que
indexa el export phase. La lectura es best-effort: un archivo ilegible cuenta
0 y nunca convierte un export exitoso en un error.

L8 — Lo que NO se tocó (S2, S5): el ERROR estructurado del export phase sigue
igual; la CLI no cambia (su nivel de consola por defecto es `warn` y el conteo
es INFO ⇒ hace falta `-v` o `--trace-file`: es intencional y queda documentado,
no "arreglado"); el sidecar de procedencia (`write_provenance_sidecar`) tiene
su propia forma y NO recibe el campo; el conteo de tools no cambia.

L9 — RED observado (T2, antes de implementar): el test
`export_jsonl_reports_the_skipped_non_utf8_lines` falló con `left: None,
right: Some(1)` — el resultado del tool no traía `structuredContent`, o sea el
campo era una promesa. GREEN: el target nuevo
`mcp_export_skipped_count_test` corre 4 tests, todos verdes — el export por el
camino MCP con una línea no-UTF-8 devuelve el conteo, el camino limpio
devuelve 0 sin mover el texto, `export_file` devuelve el mismo conteo, y el
registro de tools sigue en 36.

L10 — Evidencia de la verificación (T3, literal):
- `cargo nextest run -p webfang_mcp --features mcp` → `Summary [51.998s] 639
  tests run: 639 passed, 0 skipped` (con `--features mcp`: sin la feature el
  binario de test se compila vacío ⇒ verde por vacuidad).
- `cargo nextest run -p webfang_mcp --features mcp --test stdio_handshake` →
  `Summary [0.063s] 9 tests run: 9 passed, 0 skipped`, con
  `stdio_tools_list_reports_the_35_tool_registry` VERDE ⇒ el pin de 36 tools
  sigue intacto (S3).
- `cargo check --workspace --all-features` → `Finished dev profile` (exit 0).
- `cargo clippy --all-targets --all-features -- -D warnings -W
  clippy::cognitive_complexity -W clippy::too_many_lines` → `Finished dev
  profile` (exit 0).
- `cargo fmt --all -- --check` → exit 0, sin salida.
- `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features
  --no-deps` → `Finished dev profile` (exit 0).
- `cargo nextest run -p webfang_core --features mcp` → NO se corrió: el cambio
  es íntegramente del lado MCP (el diff no toca `crates/webfang_core/`).

L11 — Fallos ambientales (NO del cambio): en la primera corrida de la suite
MCP, `cli_vs_mcp_export_jsonl_record_parity` y
`cli_vs_mcp_sitemap_run_record_shape_parity` fallaron con "webfang binary not
found — build it first (cargo build -p webfang_cli)": el binario `webfang` lo
produce el crate hermano `webfang_cli` y no estaba en el `CARGO_TARGET_DIR`
recién sembrado. Con `cargo build -p webfang_cli` (binario de debug, sin tocar
código) la suite completa quedó verde.

L12 — Límite de slice: la rama `feat/mcp-export-skipped-count` acumula 559
líneas authored en 2 commits desde `origin/main` (por encima del corte de
~400 líneas). La estrategia de entrega de este feature es UNA sola PR
(`Closes #2238`): el slice lo forman el commit de implementación y este
commit de evidencia, y no hay un segundo slice pendiente. La PR se abre
desde `main` (no apilada) ⇒ la secuencia de entrega no depende de que
este lote quepa en un corte de 400 líneas.

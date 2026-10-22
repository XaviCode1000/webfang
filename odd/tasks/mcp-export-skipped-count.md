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

- [ ] T1 · S1, S2, S5 — `crates/webfang_mcp/src/mcp_server/handlers/export.rs`:
      el resultado del tool de export lleva el conteo de saltados; el
      provenance sidecar y el resto de la forma del resultado se mantienen
      (el sidecar tiene su propia forma: no se le agrega el campo).
- [ ] T2 · S3, S4 — el test que fija el campo en el camino MCP + el test de
      que el conteo de tools no cambia.
- [ ] T3 · S1–S5 — verificación: los tests de `webfang_mcp` verdes (incluido
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

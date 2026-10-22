# MCP export skipped count — Tasks

- [x] T1 · S1, S2, S5 — el resultado de los cuatro tools de export del servidor
      MCP lleva el conteo `skipped_non_utf8` (número JSON en
      `structuredContent` + una línea de texto cuando hay daño); el ERROR
      estructurado, la CLI y el sidecar de procedencia NO se tocan.
- [x] T2 · S3, S4 — `crates/webfang_mcp/tests/mcp_export_skipped_count_test.rs`
      (4 tests): el export por el camino MCP con una línea no-UTF-8 devuelve el
      conteo, el camino limpio devuelve 0 sin mover el texto, `export_file`
      devuelve el mismo conteo, y el registro de tools sigue en 36.
- [x] T3 · S1–S5 — verificación: suite MCP completa con `--features mcp` (639
      tests verdes), `stdio_handshake` con el pin de 36 tools verde, `cargo
      check`, `cargo clippy --all-targets --all-features`, `cargo fmt --all --
      --check` y `cargo doc` con `RUSTDOCFLAGS=-D warnings`; la evidencia está
      en el `## Log` de la spec (`odd/tasks/mcp-export-skipped-count.md`).

# Duplicación: helpers de arrange compartidos — Tasks

Spec: `odd/tasks/duplication-arrange-helpers.md`. Rama: `fix/duplication-arrange-helpers`.
Worktree: `/home/xavi/Projects/Rust/webfang-worktrees/fix-duplication-arrange-helpers`.
Origen: #1757 (fila **L12** del plan de backlog). Test nombrado del lote:
`scripts/check_duplication.sh` (+ su harness de negación `scripts/test_duplication.sh`).
**No cierra #1757**: su AC 1 es un alcance distinto (self-clones de producción) y queda medida
en el Log de la spec (P3, P4, P9). Sin PR (decisión del owner).

- [ ] T1 · S1 — `crates/webfang_core/tests/common/sitemap_arrange.rs` (nuevo) +
  `crates/webfang_core/tests/common/mod.rs` (`pub mod sitemap_arrange;`): los helpers `parser()`,
  `serve_and_parse(mock, mount_path, response, entry_path)`,
  `serve_many_and_parse(mock, mounts, entry_path)` y el helper del arrange de sitemap-index, con
  el **mismo cuerpo** que hoy vive en `sitemap_integration.rs`. Prueba: los tres binarios de test
  verdes; el clon medido de S1 desaparece del informe de jscpd.
- [ ] T2 · S1 — `crates/webfang_core/tests/sitemap_integration.rs`: se borran los helpers locales
  y se usan los del módulo compartido; los cuatro tests de sitemap-index pasan a usar el helper
  de índice. Prueba: `cargo nextest run -p webfang_core --test sitemap_integration` verde, **sin**
  cambio de snapshot.
- [ ] T3 · S1 — `crates/webfang_core/tests/sitemap_behavioral.rs` y
  `crates/webfang_core/tests/orchestrator_unit_tests.rs`: sus 8 + 5 tests pasan a usar el arrange
  compartido; aserciones y nombres intactos; el test que no desactiva el entry guard sigue sin
  desactivarlo. Prueba: `cargo nextest run -p webfang_core --test sitemap_behavioral` y
  `cargo nextest run -p webfang_core --test orchestrator_unit_tests` verdes.
- [ ] T4 · S2 — `crates/webfang_mcp/tests/common/mod.rs`: añadir `RelTempDir`, `normalized_records`
  y `walk_dir` (una vez), y el `webfang_binary()` que **delegue** en
  `webfang_test_utils::webfang_path()`. Prueba: los cinco ficheros MCP de T5 compilan y sus
  helpers nuevos existen en el módulo compartido.
- [ ] T5 · S2 — `crates/webfang_mcp/tests/mcp_behavioral_test.rs`,
  `cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs`,
  `export_coverage_test.rs`, `scraping_coverage_test.rs`: se borran las copias locales
  (`init_ssrf_disabled`, `start_test_server`, `start_seeded_server`, `redact_path`, `RelTempDir`,
  `webfang_binary`, `normalized_records`, `walk_dir`) y se usan los compartidos; el comentario de
  la cabecera de `mcp_behavioral_test.rs` que reserva el dedup para "the arrange-helpers slice" se
  borra con ellos. Prueba: `cargo nextest run -p webfang_mcp --test mcp_behavioral_test`,
  `--test cli_mcp_export_jsonl_parity_test`, `--test sitemap_crawl_run_staleness_test`,
  `--test export_coverage_test`, `--test scraping_coverage_test` verdes, **sin** cambio de
  snapshot; el binario que ejecutan es el de `webfang_test_utils::webfang_path()`.
- [ ] T6 · S3 — `scripts/quality-baselines.json`: `duplicated_lines_rust` ← la **medición** del
  candidato con el comando del gate; `notes` ← la entrada con comando, antes, después, delta,
  versión del detector y motivo; `updated` ← la fecha. Prueba: la medición re-hecha sobre el
  árbol final **iguala** el valor commiteado, y el gate sale verde.
- [ ] T7 · S4 — `scripts/check_duplication.sh`: la lectura del informe deja de tener el cero por
  defecto (sección `rust` ausente, número no entero y corpus de cero ficheros ⇒ fallo con
  mensaje que nombre la causa); el pin del detector (`jscpd 5.4.0`) intacto. Prueba: `bash
  scripts/check_duplication.sh` verde sobre el árbol, y los tres casos de fallo por vacuidad
  reproducibles sobre un árbol de `mktemp`.
- [ ] T8 · S4 — `scripts/test_duplication.sh` (nuevo): harness de negación, hermético (stub de
  `jscpd` en el `PATH`), con marcador `# NEGATION:` por caso, control positivo por cada
  aserción negativa, los siete casos de S4 y salida `PASS=<n> FAIL=<n>`. Prueba: **RED** contra
  el guard pre-fix por la razón del defecto (los casos 2 y 3 dan verde por vacuidad) y **GREEN**
  después de T7; el meta-guard lo acepta (harness verde con el guard intacto, rojo con el guard
  sustituido por `exit 0`).
- [ ] T9 · S4 — `scripts/guard_coverage_baseline.txt`: borrar la línea
  `no-harness check_duplication.sh …`. Prueba: `bash scripts/test_guard_coverage.sh` verde **sin**
  `stale-baseline`.
- [ ] T10 · S4 — `.github/workflows/ci.yml`, job `repo-guards`: un step que corra `bash
  scripts/test_duplication.sh`, con el estilo de sus vecinos (nombre + comentario con el issue y
  el porqué + la nota de que no corre cargo). Prueba: el meta-guard ve el harness cableado; el
  job `code-quality` no cambia.
- [ ] T11 · S1–S4 — verificación: `bash scripts/test_duplication.sh`; `bash
  scripts/check_duplication.sh`; el comando de medición de S3 (con el número antes y después);
  `bash scripts/test_guard_coverage.sh`; `cargo nextest run -p webfang_core --test
  sitemap_integration --test sitemap_behavioral --test orchestrator_unit_tests`; `cargo nextest run
  -p webfang_mcp --test mcp_behavioral_test --test cli_mcp_export_jsonl_parity_test --test
  sitemap_crawl_run_staleness_test --test export_coverage_test --test scraping_coverage_test`;
  `cargo check` + `cargo clippy --all-targets --all-features -- -D warnings -W
  clippy::cognitive_complexity -W clippy::too_many_lines` + `cargo fmt --all -- --check` + `env
  "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps`.

## Estado (2026-10-22)

- Lote **L12** del plan de backlog (`odd/tasks/backlog-implementation-plan.md`), rama
  `fix/duplication-arrange-helpers`, base `main@89ff9d46` (con #2230 ya mergeado ⇒ el bloqueo que
  arrastraba el lote está levantado).
- Este archivo es **sólo especificación**: no se escribió código de producción, no se corrió
  `cargo`, no se commiteó nada. Sin PR (decisión del owner).
- **Medición de partida** (jscpd 5.4.0, `--min-tokens 50 --format rust --mode mild`, el comando
  del gate): `main@89ff9d46` = **9009** líneas duplicadas / 839 clones / 479 ficheros rust. El
  baseline commiteado es **9272** ⇒ **está rancio en 263 líneas** (en `b1673234`, el commit que lo
  fijó, la medición era exactamente 9272). El delta de este lote es el que se mida sobre el
  candidato, y el baseline baja a ese número medido.
- **Test nombrado del lote**: `scripts/check_duplication.sh` + su harness de negación
  `scripts/test_duplication.sh` (el meta-guard `scripts/test_guard_coverage.sh` exige el
  harness: marcador `# NEGATION:`, cableado en un workflow, verde con el guard intacto y rojo con
  el guard sustituido por `exit 0`).
- **Premisas de #1757 corregidas por la spec** (verificadas en el árbol, no en la issue): el
  baseline vigente es **9272**, no 7683; el "7602" de la AC 2 es de la **escala del detector
  anterior** (el 2026-10-06 el detector cambió y la escala se recalibró 7683 → 13310) ⇒ el
  criterio comparable es la medición del detector vigente; `jsonl_exporter.rs` tiene **96** líneas
  de self-clones (no ~109) y `checkpoint.rs` **22** (no ~27); `fingerprint.rs` **9** (confirmado);
  `batch_flow.rs` tiene **0** self-clones y `discovery_phase.rs` **6** (no ~40); y el **43 %** de
  la duplicación del árbol (**3864** de 9009 líneas) está **en ficheros de test**, que es donde
  el repo tiene la campaña de helpers de arrange ya establecida (#1877, #1888, #1889, #1890,
  #1944) y donde el propio código de `mcp_behavioral_test.rs` reserva el trabajo: *"server-bootstrap
  dedup belongs to the arrange-helpers slice, not this one"*.
- **Lo que este lote NO hace**: no deduplica los self-clones de producción de la AC 1
  (`jsonl_exporter.rs`, `checkpoint.rs`) ⇒ **no cierra #1757**; no sube el baseline en ningún caso;
  no toca `Cargo.toml` de ninguna crate; no toca `docs/` ni `CHANGELOG.md`.

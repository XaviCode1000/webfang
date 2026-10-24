# El test de los shims retired parsea con el entorno ambiente — Tasks

- [x] T1 · S4 — RED real con el env del job `poisoned-env` (verbatim del workflow); si el test no falla, el premise es falso y se documenta antes de tocar código.
- [x] T2 · S1, S2 — el test in-module parsea con `parse_args_hermetic`; nombre y aserción intactos.
- [x] T3 · S3 — el gemelo de `tests/max_chars_bound_test.rs` queda consistente (o el motivo escrito de por qué no hay nada que cambiar).
- [x] T4 · S1, S4 — GREEN: el mismo comando con el mismo env venenoso pasa.
- [x] T5 · S1–S5 — `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, verde con y sin el env venenoso, suite del crate en verde.

Spec: `odd/tasks/ci-health-hermetic-args.md`. Cierra #2234.

## Evidencia

- **T1 (RED).** Con el env del job `poisoned-env` (verbatim del workflow), el test in-module
  `cli::args::ai::spec_parity_tests::the_retired_shim_env_vars_are_ignored` falla con
  `bare invocation must parse: "error: invalid value 'POISON_TRASH' for '--url <URL>':
  URL invalida: relative URL without a base"`. El fallo es del helper no hermético
  (`parse_args`), que absorbe el fallback de env `WEBFANG_URL`; el premise de #2234 queda
  confirmado.
- **T2 (fix de clase).** `cli::args::test_support::parse_args` (el no hermético) se elimina y
  `parse_args_hermetic` queda como **único** entry point de parseo del crate, con el cuerpo
  inlineado. Los 21 call sites de `ai.rs`, `export.rs` y `crawler.rs` pasan a
  `parse_args_hermetic`. El nombre del test y su aserción no se tocan.
- **T3 (gemelo).** `crates/webfang_core/tests/max_chars_bound_test.rs` **no se toca**: su
  `the_retired_shim_env_vars_are_ignored` spawna el binario con `t.scraper_cmd()`, y el
  harness (`crates/webfang_core/tests/common/cli_harness.rs`) aplica `sanitize_env` →
  `strip_poisoned_env`, que quita `WEBFANG_*` / `AI_MODEL_ID` del entorno del hijo. El gemelo
  ya era hermético por construcción.
- **Test nuevo del lote.** `cli::args::tests::the_test_parse_surface_ignores_the_ambient_environment`:
  envenena `WEBFANG_URL=POISON_TRASH`, parsea vacío por el único entry point del crate y
  aserta que la URL no aparece. Usa `webfang_test_utils::env_set` / `env_remove` (lock
  transitorio) porque el lock de env no es reentrante: un `EnvGuard` vivo durante el parseo
  se cuelga. No es vacuo: restaurado el entry point a su forma no hermética (experimento
  temporal, revertido), el test falla con el mismo panic del RED.
- **T4 (GREEN).** `cargo nextest run -p webfang_core --lib --all-features` con el env
  venenoso: `2963 tests run: 2963 passed, 4 skipped` (exit 0). Sin env venenoso: `2963
  passed, 4 skipped` (exit 0).
- **T5 (verificación).** `cargo nextest run -p webfang_core --test max_chars_bound_test
  --all-features` (env venenoso): `26 tests run: 26 passed` (exit 0). `cargo nextest run -p
  webfang_test_utils`: `28 tests run: 28 passed` (exit 0). `cargo fmt --all -- --check`: exit
  0. `cargo clippy -p webfang_core --all-targets --all-features -- -D warnings -W
  clippy::cognitive_complexity -W clippy::too_many_lines`: exit 0.
- **S5.** El defecto de fondo (el clasificador W-03 reporta el rojo como "known FP") es de L3a
  y no se toca aquí; lo que L3b deja explícito es que el rojo no era un falso positivo del
  clasificador sino un test que dependía del shell de quien lo corría.

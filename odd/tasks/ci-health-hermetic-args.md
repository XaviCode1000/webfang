# El test de los shims retired parsea con el entorno ambiente

Objetivo: un test que depende del shell de quien lo corre no es un test. El test in-module
`the_retired_shim_env_vars_are_ignored` parsea con el helper **no hermético** y por eso
revienta dentro de la lane de descubrimiento de bugs, donde el entorno está deliberadamente
envenenado. El fix no cambia la expectativa: cambia el helper.
Rama: `fix/2234-hermetic-args`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-2234-hermetic-args`.
Cierra: #2234. Gemelo: `crates/webfang_core/tests/max_chars_bound_test.rs`.

---

## Specs

**S1.** El test parsea de forma hermética, con el helper que ya existe en el crate
(de #2234):

> El test (y su gemelo) parsean de forma hermética, con el helper que ya existe en el crate.

> Con el entorno venenoso de la lane Bug Discovery ambos tests pasan.

El helper existe y su contrato está escrito (de `crates/webfang_core/src/cli/args/mod.rs`):

> Parse a full `Args` with ambient clap env fallbacks cleared.

y su gemelo no hermético, que es el que el test está usando por error:

> Parse a full `Args` from synthetic argv (non-hermetic: ambient `WEBFANG_*` env fallbacks
> leak in — prefer [`parse_args_hermetic`]).

**S2.** El nombre del test y su aserción se conservan: el fix es de hermeticidad, no de
expectativa (de #2234):

> El nombre del test y su aserción (los shims retired se ignoran) se conservan: el fix es de
> hermeticidad, no de expectativa.

La aserción que hay que preservar (de `crates/webfang_core/src/cli/args/ai.rs`):

> The retired env doors are IGNORED, not rejected: an env var is not a flag, so clap cannot
> answer it. What this pins is that ignoring it is SILENT and leaves the budget alone — a
> stale `WEBFANG_MAX_TOKENS` must not be converted into a character budget.

**S3.** El gemelo queda consistente con el fix (de #2234):

> El gemelo `crates/webfang_core/tests/max_chars_bound_test.rs` (mismo nombre de test, mismos
> marcadores `RETIRED_AI_MODEL_NOTICE_MARKER` / `RETIRED_MAX_TOKENS_NOTICE_MARKER`) tiene que
> quedar consistente con el fix.

**S4.** El RED se demuestra con el entorno venenoso real de la lane, no con un entorno
inventado (de #2234 + la disciplina del repo):

> `.github/workflows/bug-discovery.yml` envenena el entorno del job: `WEBFANG_URL=POISON_TRASH`,
> `WEBFANG_OUTPUT=POISON_TRASH`, `WEBFANG_THRESHOLD=0.99`, `WEBFANG_SELECTOR=POISON`,
> `AI_MODEL_ID=POISON`.

> Consecuencia: el test falla dentro de la lane de descubrimiento, y un developer con
> `WEBFANG_THRESHOLD` exportado en su shell lo tiene rojo en local — un test que depende del
> shell de quien lo corre.

El RED tiene que salir del comando que la lane corre, con ese env exacto:

> cargo nextest run --workspace --all-features --no-fail-fast

**S5.** El defecto de fondo que lo tapaba se arregla en el mismo lote, o queda explícito que
lo tapa otro (de #2234):

> El defecto de fondo está tapado: el clasificador W-03 del observer reporta ese rojo como
> "known FP" (issue del clasificador), así que el fallo real no se ve.

El lote del clasificador (L3a) es el que destapa; L3b lo deja explícito en su Log.

---

## Tasks

- [x] T1 · S4 — RED real: correr `cargo nextest run -p webfang_core --lib --all-features` con
  el env del job `poisoned-env` (verbatim del workflow) y **capturar el fallo** del test.
  Si el test NO falla con ese env, el premise de #2234 es falso: documentarlo y reescribir la
  spec antes de tocar código (no hay fix sin RED).
- [x] T2 · S1, S2 — el test in-module parsea con `parse_args_hermetic`; el nombre y la
  aserción no se tocan.
- [x] T3 · S3 — el gemelo de `tests/max_chars_bound_test.rs` queda consistente (verificar si
  ya es hermético por construcción y dejar el motivo escrito si no hay nada que cambiar).
- [x] T4 · S1, S4 — GREEN: el mismo comando con el mismo env venenoso pasa.
- [x] T5 · S1–S5 — verificación: `cargo fmt --all -- --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, el test en verde **con y sin** el env venenoso, y la suite
  del crate en verde.

## Log

L1 — Petición del owner (verbatim):

> tengo muchas y  varias issues abiertas y necesito un plan para ir implementando utliza la
> skill adecuada para triage de issues

L2 — Corrección del owner sobre el estado del sistema (verbatim):

> debes tener en cuenta que nuesto sistema ha cambiado sobre todo en lo relacionado con
> modelos de *AI LOCAL* Y LAS ISSUES pueden decir cosas o indormacion antigua o falsa,

> utiliza siempre tu y ordenar a  todos los workers las potentes tools *codedb* y *codegraph*
> en lugar de las otras tools

L3 — Reparto y formato de entrega (verbatim):

> ya tengo un agente trabajando en lote k1 en
> /home/xavi/Projects/Rust/webfang-worktrees/fix-ssrf-this-network-justification

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

L4 — Evidencia del triage (CodeDB 0.2.5849 + CodeGraph sobre `main@3a785d8e`):
- `the_retired_shim_env_vars_are_ignored` existe **dos veces**: el in-module
  (`crates/webfang_core/src/cli/args/ai.rs`, módulo `spec_parity_tests`) y el gemelo de
  integración (`crates/webfang_core/tests/max_chars_bound_test.rs`). Sólo el in-module llama
  `parse_args`; el gemelo **spawna el binario** con las variables puestas a propósito, que es
  otra cosa y ya es hermética por construcción.
- El comentario del propio test dice "`parse_args_hermetic` clears the ambient `WEBFANG_*`
  variables" y acto seguido llama `parse_args`: el código contradice su propio comentario.
- `parse_args` es el helper no hermético del crate (`cli/args/mod.rs`); `parse_args_hermetic`
  es `with_clap_env_cleared(|| parse_args(extra))` y lo usan los tests de superficie de
  `crawler.rs`, `export.rs`, `llm.rs` y `ai.rs` (excepto este).
- #2234 está `open` con `type:chore` y `status:approved`.

L5 — T1 · RED real (evidencia capturada con el env del job `poisoned-env`, verbatim):

    WEBFANG_URL=POISON_TRASH WEBFANG_OUTPUT=POISON_TRASH WEBFANG_SINGLE_PAGE=true \
    WEBFANG_THRESHOLD=0.99 WEBFANG_SELECTOR=POISON AI_MODEL_ID=POISON \
    WEBFANG_OBSIDIAN_WIKI_LINKS=true OTEL_EXPORTER_OTLP_ENDPOINT=http://poison:4317 \
    cargo nextest run -p webfang_core --lib --all-features

    running 1 test
    test cli::args::ai::spec_parity_tests::the_retired_shim_env_vars_are_ignored ... FAILED
    thread 'cli::args::ai::spec_parity_tests::the_retired_shim_env_vars_are_ignored' panicked at
    crates/webfang_core/src/cli/args/ai.rs:488:38:
    bare invocation must parse: "error: invalid value 'POISON_TRASH' for '--url <URL>':
    URL invalida: relative URL without a base"

El fallo es el del helper, no del test: `parse_args` (el no hermético) absorbe el fallback de
env `WEBFANG_URL` y el parseo revienta antes de llegar a la aserción.

L6 — T2 · El fix es de clase, no de caso: se borra el helper no hermético, no se agrega un flag.

- `cli::args::test_support::parse_args` (el no hermético) **eliminado**. Su único caller vivo
  era `parse_args_hermetic`, que lo envolvía en `with_clap_env_cleared`; no queda caller de
  producción ni de test.
- `parse_args_hermetic` conserva el cuerpo inlineado (argv `vec!["webfang"]` + extra,
  `crate::Args::try_parse_from`, error mapeado a `String`) y ahora es **el único** entry point
  de parseo del crate: la hermeticidad es estructural, no una convención que un test pueda
  incumplir por error.
- Los 21 call sites de `ai.rs`, `export.rs` y `crawler.rs` pasan a `parse_args_hermetic`.
  `crawler.rs` (defaults de superficie) deja de envolver a mano con `with_clap_env_cleared`:
  el guard manual era la forma manual de conseguir la hermeticidad que ahora es la norma.
- El nombre del test (`the_retired_shim_env_vars_are_ignored`) y su aserción **no se tocan**:
  el fix es de hermeticidad, no de expectativa (S2).

L7 — T3 · El gemelo ya era hermético por construcción: **no se tocó**.

`crates/webfang_core/tests/max_chars_bound_test.rs` (`the_retired_shim_env_vars_are_ignored`) no
parsea en proceso: spawna el binario con `t.scraper_cmd()`, y el harness
(`crates/webfang_core/tests/common/cli_harness.rs`) aplica `sanitize_env` →
`strip_poisoned_env`, que quita todo `WEBFANG_*` y `AI_MODEL_ID` del entorno del hijo antes
de arrancarlo. El gemelo es hermético por construcción (spawnea con el entorno limpio y pone
a propósito sólo las variables retired que quiere ejercitar), así que queda consistente con el
fix sin cambios: 26/26 verdes con el env venenoso (ver L9).

L8 — Test nuevo del lote: `the_test_parse_surface_ignores_the_ambient_environment`
(`cli::args::tests`).

Envenena `WEBFANG_URL=POISON_TRASH` (la forma exacta en que la lane envenena), parsea vacío
por el único entry point del crate y aserta que la URL no aparece en el resultado
(`crawler.url` y `positional_url` quedan en `None`).

- El lock de env **no es reentrante**: `EnvGuard` lo toma mientras vive y `parse_args_hermetic`
  lo toma otra vez, así que sostener un `EnvGuard` durante el parseo se cuelga. El test usa
  `webfang_test_utils::env_set` / `env_remove` (toman el lock de forma transitoria), captura
  el `Result` del parseo **antes** de restaurar el entorno y restaura el valor original
  (ausente o no) para no dejar el proceso distinto de como se lo encontró.
- El test no es vacuo: es el guard de la clase. Con el entry point de parseo restaurado a su
  forma no hermética (experimento temporal, revertido), el test falla con el mismo panic del
  RED: `bare invocation must parse: "error: invalid value 'POISON_TRASH' for '--url <URL>'"`.

L9 — T4, T5 · GREEN y verificación (todos con `CARGO_TARGET_DIR=$HOME/.cache/cargo-target/fix-2234-hermetic-args`,
`CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=1`).

- `cargo nextest run -p webfang_core --lib --all-features` **con el env venenoso**:
  `Summary [58.102s] 2963 tests run: 2963 passed, 4 skipped` (exit 0). Incluye
  `cli::args::ai::spec_parity_tests::the_retired_shim_env_vars_are_ignored` y
  `cli::args::tests::the_test_parse_surface_ignores_the_ambient_environment`, ambos PASS.
- `cargo nextest run -p webfang_core --lib --all-features` **sin env venenoso**:
  `Summary [57.708s] 2963 tests run: 2963 passed, 4 skipped` (exit 0).
- `cargo nextest run -p webfang_core --test max_chars_bound_test --all-features` con el env
  venenoso: `Summary [248.270s] 26 tests run: 26 passed (4 slow, 4 flaky), 0 skipped` (exit 0).
- `cargo nextest run -p webfang_test_utils`: `Summary [0.280s] 28 tests run: 28 passed, 0 skipped`
  (exit 0) — incluye el scan de invariantes de env (`env_invariant_scan.rs`).
- `cargo fmt --all -- --check`: exit 0, sin diferencias.
- `cargo clippy -p webfang_core --all-targets --all-features -- -D warnings -W
  clippy::cognitive_complexity -W clippy::too_many_lines`: exit 0, sin warnings.

L10 — Nota de alcance (S5, lote L3a).

El defecto de fondo —el clasificador W-03 del observer reporta el rojo como "known FP"— sigue
siendo de L3a; este lote no lo toca. Lo que L3b deja explícito es el motivo: el rojo no era
un falso positivo del clasificador, era un test que dependía del shell de quien lo corría. Con
el helper no hermético eliminado, la clase del defecto desaparece: no queda ningún entry point
de parseo en el crate que pueda absorber el entorno ambiente.


# Grafo de build: una sola fuente de RAM, sin `built`, y el benchmark sin `chromium`

Cierra el lote **L9** del plan de backlog: el grafo arrastra dos crates para leer RAM
(`sys-info` y `sysinfo`), un `once_cell` que std ya trae, un `built`/`git2`/`libgit2-sys`
que sólo alimenta una función sin call sites, y un benchmark que enciende `chromium` para
todo el workspace. Rama: `fix/build-graph-deps`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-build-graph-deps`. Cierra: #1799, #1797, #1796.

**Convención de citas**: los bloques `>` son citas **literales** de la issue (traen los
números de línea que tenía el issue el 2026-10-03; son texto ajeno, no una referencia al
árbol). Las referencias al código de este repo van **por path + símbolo**, nunca por
número de línea.

---

## Specs

**S1.** La lectura de RAM tiene **una sola fuente** y **una sola unidad**, y la conversión
de unidades es explícita y verificable (verbatim de #1799):

> (Commit 1) Migrar `crates/webfang_core/src/infrastructure/autotuning.rs:102`
> (`sys_info::mem_info()`) a `sysinfo::System` y quitar `sys-info` de la feature
> `persistence` de `webfang_core`.

> Ojo unidades, verificado en código: `ram_budget_from(total_kib)` (`autotuning.rs:78-81`)
> espera **KiB** (`kib*1024*7/10`) mientras `sysinfo::total_memory()` devuelve **bytes** — el
> call-site debe dividir por 1024.

> Copiar el patrón de `detector.rs:99-104` (`System::new()` + `refresh_memory()`, con su
> gate `#[cfg(not(miri))]`).

> Mantener el gate `#[cfg(feature = "persistence")]` de `detect_ram_budget` tal cual:
> quitarlo cambiaría el comportamiento sin esa feature y va en otro issue.

> Commit 1: `sys-info` fuera de `Cargo.lock`; tests de `autotuning` verdes (incluido el de
> 32 GiB).

Código a cambiar:

- `crates/webfang_core/src/infrastructure/autotuning.rs`
  - `ram_budget_from_bytes(total_bytes: Option<u64>) -> u64` — **única** función pública
    que recibe la lectura del sistema. Convierte bytes→KiB de forma explícita
    (`bytes / 1024`) y aplica la regla del 70 %.
  - `ram_budget_from_kib(total_kib: Option<u64>) -> u64` — **privada**: la regla del 70 %
    (`kib * 1024 * 7 / 10`, truncada) que hoy vive en `ram_budget_from`.
  - `ram_budget_from(total_kib)` — **se borra** del espacio público. Su nombre no dice la
    unidad y ése es exactamente el defecto: el valor del sistema se le pasaba como KiB sin
    que el nombre lo anunciara. Sin call sites en el árbol (verificado con `codedb`), el
    repo no publica a crates.io (`release-plz.toml`: *"This repo does not publish to
    crates.io"*), así que no hay obligación de semver que lo frene.
  - `detect_ram_budget()` — la lectura pasa a `sysinfo::System::new()` +
    `refresh_memory()` + `total_memory()` (**bytes**, documentados en `sysinfo`), con el
    gate `#[cfg(not(miri))]` copiado de `domain/budget/detector.rs`
    (`SystemDetector::total_ram_bytes`): `sysinfo` llama `sysconf` en el constructor de
    `System`, y Miri no puede ejecutarlo.
  - Los dos gates se conservan: `#[cfg(feature = "persistence")]` (de la issue) **y**
    `#[cfg(not(miri))]` (del patrón de `detector.rs`). Sin `persistence`, el presupuesto
    sigue siendo el fallback de 2 GiB: eso no se toca (va en otro issue).
  - Docs a actualizar: la cabecera del módulo (menciona `sys_info::mem_info()`), el doc de
    `FALLBACK_RAM_BUDGET_BYTES` y el doc de la función de fallback.
- `crates/webfang_core/Cargo.toml` — `persistence` queda
  `["dep:rusqlite", "dep:deadpool-sqlite"]`; se borra la dependencia opcional
  `sys-info`.
- `Cargo.toml` (workspace) — se borra `sys-info = "0.9"`.
- `Cargo.lock` — se regenera: `sys-info` sale (y con él nada más: `sysinfo` ya era
  dependencia directa de `webfang_core`).

**Test nombrado del lote** (el que nombra el plan, L9):
`autotuning.rs::ram_budget_from_bytes_converts_to_kib`. Además, el test existente
`test_ram_budget_from_32gib_machine` sigue verde con la entrada expresada en **bytes**
(32 GiB) y la misma aserción de resultado (70 % truncado), y
`test_ram_budget_from_none_falls_back_to_2gib` pasa a la función nueva.

**Lo que este S1 NO es**: un fix de un bug de macOS. Ver la premisa falsa P1 del Log:
`sys-info` 0.9.1 devuelve **KiB en todas las plataformas**, macOS incluida. El cambio es
higiene de grafo + endurecimiento del contrato de unidades (documentado en `sysinfo`,
documentado en ninguna parte en `sys-info`).

**S2.** `once_cell` sale del grafo: std lo trae desde 1.80 y el MSRV del repo es 1.88
(verbatim de #1799):

> (Commit 2) Migrar los 3 usos de `once_cell` a std (`infrastructure/http/waf_engine.rs:33`
> y `domain/scraper_port/dom_pruner.rs:14` → `std::sync::LazyLock`;
> `tests/tokio_console_smoke_test.rs:63` → `std::sync::OnceLock`) y quitar `once_cell` de
> `[workspace.dependencies]` y de `webfang_core`.

> Commit 2: cero `use once_cell` en `crates/` (src y tests); `once_cell` fuera de los
> manifiestos. Explícito: seguirá en `Cargo.lock` vía `ahash`/`indicatif`/`criterion`/`dashmap`
> (verificado con `cargo tree -i`) — eso es esperado y no es el objetivo.

Código a cambiar:

- `crates/webfang_core/src/infrastructure/http/waf_engine.rs` — `use once_cell::sync::Lazy;`
  → `use std::sync::LazyLock;` y los `static` (`WAF_AC`) pasan a `LazyLock<AhoCorasick>`
  con `LazyLock::new(...)`. Sustitución **directa**, sin wrapper ni alias.
- `crates/webfang_core/src/domain/scraper_port/dom_pruner.rs` — ídem (`DISPLAY_NONE_RE`,
  `VISIBILITY_HIDDEN_RE`, `EMPTY_WRAPPER_PATTERNS`).
- `crates/webfang_core/tests/tokio_console_smoke_test.rs` —
  `once_cell::sync::OnceCell::new()` → `std::sync::OnceLock::new()`.
- `crates/webfang_core/Cargo.toml` — se borra `once_cell = { workspace = true }`.
- `Cargo.toml` (workspace) — se borra `once_cell = "1"`.

`once_cell` **permanece en `Cargo.lock`**: lo siguen trayendo `ahash`, `indicatif`,
`criterion`, `dashmap`, `sysinfo`, `insta`, `tempfile` y compañía. Eso es lo que la issue
pide explícitamente y lo que el harness de T7 afirma como control positivo.

**S3.** `built` (y con él `git2` y `libgit2-sys`) sale del grafo, y la función que lo
consumía se borra con él (verbatim de #1797):

> Eliminar `built` de `[build-dependencies]` de `webfang_core`; generar
> `GIT_COMMIT_HASH_SHORT` (`git rev-parse --short HEAD`) y `BUILT_TIME_UTC` con
> `cargo:rustc-env` en un `build.rs` propio, con fallback a `"unknown"` cuando no haya
> `.git` (tarball, release-plz).

> `built`, `git2`, `libgit2-sys` fuera de `Cargo.lock`.

> Test `crates/webfang_core/tests/behavioral/cli/core_test.rs:19`
> (`version_contains_version_string`) verde, incluido el caso sin `.git`.

**La primera cita no se implementa tal cual, y el motivo está registrado**: la función
que alimentaría ese `build.rs` —`webfang_core::version_string()`— tiene **0 call sites**
(verificado con `codedb`), y el `build.rs` que el issue pide escribir sería metadata falsa
(hash y fecha de build que nadie lee) reproducida con un arista de build propia. La
decisión D18 del plan ya lo decidió: *dropear `version_string()` (0 call sites) y **no**
escribir `build.rs`*. Sin `build.rs` nuevo, el perfil de release sigue siendo el de Cargo
y no queda ninguna arista que nadie pueda reproducir.

Lo que sí se implementa de #1797:

- `crates/webfang_core/build.rs` — se borra la llamada `built::write_built_file()`. El
  archivo **ya existe** y **se queda**: su otra misión (emitir `cargo:rustc-cfg=loom`
  bajo la feature `loom-model`) no tiene nada que ver con `built`, y el `cargo:rerun-if-changed=build.rs`
  que ya emite también se conserva.
- `crates/webfang_core/src/lib.rs` — se borra el módulo `built_info`
  (`include!(concat!(env!("OUT_DIR"), "/built.rs"))`) y la función `version_string()`.
- `crates/webfang_core/Cargo.toml` — se borra la sección `[build-dependencies]` con
  `built = { version = "0.7", features = ["git2", "chrono"] }`.
- `Cargo.lock` y `fuzz/Cargo.lock` — se regeneran. `fuzz/` no es miembro del workspace
  pero depende de `webfang_core` por path, así que arrastra el `built` → `git2` →
  `libgit2-sys`; su lock se regenera con `cargo metadata --manifest-path fuzz/Cargo.toml`
  (resuelve, no compila).
- `deny.toml` — se purgan los dos ignores `RUSTSEC-2026-0183` y `RUSTSEC-2026-0184`. Su
  razón escrita dice, textualmente, *"reachable only through the built build-dependency of
  webfang_core"*: con `built` fuera del grafo la justificación queda sin objeto y el
  ignore se vuelve una promesa falsa. Es el mismo protocolo de purga de ignores
  obsoletos que el propio `deny.toml` documenta (issue #727). La reentrada se obedece
  el "RE-ADD PROTOCOL" del mismo archivo. **No** se purga si `git2` siguiera
  en alguno de los dos lock: se verifica antes.

El test de la issue sigue verde sin tocarlo: `core_test.rs::version_contains_version_string`
comprueba que `--version` contenga `CARGO_PKG_VERSION`, y el `--version` de la CLI lo
produce clap (`#[command(name = "webfang", version)]`), nunca `version_string()`.

Efecto colateral verificado (citado por la issue): el hash hoy queda stale porque
`built` 0.7.7 no emite ningún `cargo:rerun-if-changed` y el `build.rs` del repo sí emite
`cargo:rerun-if-changed=build.rs`, lo que **sobrescribe** el comportamiento por defecto
de cargo. Al borrar `built`, el problema desaparece porque el consumidor del hash
desaparece.

**S4.** El benchmark declara su propia feature `chromium` y el grafo default del
workspace deja de arrastrar el navegador (verbatim de #1796):

> En `crates/webfang_benchmark/Cargo.toml`, quitar `features = ["chromium"]` de la
> dependencia `webfang_core` y exponer `[features] chromium = ["webfang_core/chromium"]` en
> el propio benchmark. Invocar como `cargo run|test -p webfang_benchmark --features
> chromium` (nunca `--workspace --features`).

> `cargo tree --offline --workspace -e normal --prefix none | grep -c '^chromiumoxide'` → 0
> (sin espacio final: cuenta también `chromiumoxide_cdp`/`_types`; hoy da 6).

> `cargo nextest run -p webfang_benchmark` verde con y sin `--features chromium`: el brazo
> Full de `e2e_smoke_test.rs:91` queda gateado o marcado como requerido por la feature.

> `benches.yml` y demás invocaciones (`rg -l 'bench_live|bench_tier_a|webfang_benchmark'
> .github/ docs/ scripts/`) pasan `--features chromium` donde necesiten tier browser.

Código a cambiar:

- `crates/webfang_benchmark/Cargo.toml` — se quita `features = ["chromium"]` de la
  dependencia `webfang_core` y se añade `[features] chromium = ["webfang_core/chromium"]`.
  La feature **reenvía** (`webfang_core/chromium`), no **activa** el navegador: si el
  benchmark encendiera `chromiumoxide` por su cuenta, el grafo default seguiría
  contaminado y el lote no habría arreglado nada.
- `crates/webfang_benchmark/src/runner.rs` — `STRATEGIES` incluye `JsStrategy::Full`
  **sólo** con la feature `chromium`. Medir un tier que no se puede ejecutar produce una
  fila de reporte con errores, no una medición: el benchmark no debe correr el brazo que
  su build no soporta.
- `crates/webfang_benchmark/tests/e2e_smoke_test.rs` — las aserciones del conjunto de
  tiers (`outcomes.len()` y el orden determinista) siguen a la feature: con la feature,
  `[Static, Hybrid, Full]`; sin ella, `[Static, Hybrid]`.

La feature nueva es el **único** estado que este lote agrega, y está justificada: sin
ella el benchmark pierde el tier de navegador para siempre (el `build.rs` de `chromium`
no se puede reescribir sin reescribir el motor) o el workspace sigue arrastrando
`chromiumoxide` en cada build (que es el defecto que la issue reporta). La alternativa
"borrar el tier Full" también es una opción, y queda **documentada como la salida** si
el owner decide que el tier no vale la feature: en ese caso T6 se reduce a borrar
`JsStrategy::Full` de `STRATEGIES` y el brazo del test.

Sobre la cita de `benches.yml`: verificada — `benches.yml` **no** invoca
`webfang_benchmark` (su scope lo deriva `scripts/derive-bench-scope.py` de los targets
`bench` de `cargo metadata`, y el benchmark no tiene ninguno). Tampoco hay ninguna otra
invocación de `bench_live`/`bench_tier_a`/`webfang_benchmark` en `.github/`, `docs/` o
`scripts/` (los hits son comentarios y el guard de dirección de dependencias). ⇒ **No hay
ninguna invocación que actualizar**; lo que queda es el comando manual
`cargo nextest run -p webfang_benchmark --features chromium`, documentado en el PR.

La cita de "hoy da 6" no se reproduce en el árbol actual: el lock tiene **4** crates
`chromiumoxide*` (`chromiumoxide`, `chromiumoxide_cdp`, `chromiumoxide_pdl`,
`chromiumoxide_types`). El mecanismo (unificación de features con `resolver = "2"`) sí es
el descrito; el número es una medición de 2026-10-03. Ver Log P2.

---

## Tasks

- [ ] T1 · S1 — `crates/webfang_core/src/infrastructure/autotuning.rs`: agregar
  `ram_budget_from_bytes` (público, bytes→KiB explícito) y `ram_budget_from_kib`
  (privado, la regla del 70 %); borrar `ram_budget_from` del espacio público;
  `detect_ram_budget` lee con `sysinfo::System::new()` + `refresh_memory()` +
  `total_memory()` bajo `#[cfg(not(miri))]` y mantiene el gate
  `#[cfg(feature = "persistence")]`; actualizar los docs de módulo, constante de
  fallback y mensaje de `warn!`. **Prueba**: test nombrado
  `ram_budget_from_bytes_converts_to_kib` (32 GiB en bytes → 70 %; equivalencia con la
  regla en KiB; truncamiento y saturación) +
  `test_ram_budget_from_32gib_machine` (misma aserción, entrada en bytes) +
  `test_ram_budget_from_none_falls_back_to_2gib` sobre la función nueva; verde con
  `cargo nextest run -p webfang_core --lib autotuning`.
- [ ] T2 · S1 — `crates/webfang_core/Cargo.toml` (feature `persistence` y dependencia
  opcional `sys-info`) y `Cargo.toml` del workspace (`sys-info = "0.9"`): borrar
  `sys-info`; regenerar `Cargo.lock`. **Prueba**: `grep -c '^name = "sys-info"' Cargo.lock`
  → 0 (lo afirma T7).
- [ ] T3 · S2 — `crates/webfang_core/src/infrastructure/http/waf_engine.rs` y
  `crates/webfang_core/src/domain/scraper_port/dom_pruner.rs`: `once_cell::sync::Lazy` →
  `std::sync::LazyLock`; `crates/webfang_core/tests/tokio_console_smoke_test.rs`:
  `once_cell::sync::OnceCell` → `std::sync::OnceLock`; borrar `once_cell` de
  `crates/webfang_core/Cargo.toml` y de `[workspace.dependencies]`. **Prueba**: cero
  `use once_cell` en `crates/` (src y tests) y `once_cell` fuera de los manifiestos
  (T7); los tests de `waf_engine`, `dom_pruner` y `tokio_console_smoke_test` en verde.
- [ ] T4 · S3 — `crates/webfang_core/build.rs`: borrar la llamada a
  `built::write_built_file()` y conservar el cfg de `loom` y el `rerun-if-changed`;
  `crates/webfang_core/src/lib.rs`: borrar el módulo `built_info` y `version_string()`;
  `crates/webfang_core/Cargo.toml`: borrar `[build-dependencies] built`; regenerar
  `Cargo.lock` y `fuzz/Cargo.lock`. **Prueba**: `built`, `git2` y `libgit2-sys` fuera de
  los dos lock (T7); `core_test.rs::version_contains_version_string` en verde; el
  `--version` de la CLI sigue imprimiendo la versión del paquete.
- [ ] T5 · S3 — `deny.toml`: purgar los ignores `RUSTSEC-2026-0183` y
  `RUSTSEC-2026-0184` **sólo si** `git2` ya no está en `Cargo.lock` ni en
  `fuzz/Cargo.lock`; su razón escrita queda sin objeto. **Prueba**:
  `bash scripts/check_advisory_policy.sh` en verde (si `cargo-deny`/`cargo-audit` no
  están instalados, el paso se reporta como pendiente: un guard que no se puede correr
  no se maquilla en verde).
- [ ] T6 · S4 — `crates/webfang_benchmark/Cargo.toml`: quitar `features = ["chromium"]` de
  la dependencia `webfang_core` y declarar `[features] chromium =
  ["webfang_core/chromium"]`; `crates/webfang_benchmark/src/runner.rs`: `STRATEGIES`
  incluye `JsStrategy::Full` sólo con la feature;
  `crates/webfang_benchmark/tests/e2e_smoke_test.rs`: las aserciones del conjunto de
  tiers siguen a la feature. **Prueba**: `cargo nextest run -p webfang_benchmark` verde
  **sin** la feature y **con** `--features chromium`; el comando de la AC
  `cargo tree --offline --workspace -e normal --prefix none | grep -c '^chromiumoxide'`
  → 0, y el mismo comando con `-p webfang_benchmark --features chromium` → ≥ 1 (control
  positivo: la feature sigue existiendo y sigue dando el tier de navegador).
- [ ] T7 · S1–S4 — `scripts/test_build_graph.sh` (nuevo): harness de negación del
  grafo, con control positivo por cada check (un harness que puede dar verde por
  construcción no es evidencia). Checks: `sys-info` fuera de `Cargo.lock`; `built`,
  `git2` y `libgit2-sys` fuera de `Cargo.lock` **y** de `fuzz/Cargo.lock`; `once_cell`
  fuera de todo manifiesto pero **presente** en `Cargo.lock` (control positivo);
  cero `use once_cell` en `crates/**/*.rs`; el benchmark declara
  `chromium = ["webfang_core/chromium"]` y **no** pide `features = ["chromium"]` en su
  dependencia; `grep -c '^chromiumoxide'` → 0 en el árbol default del workspace y ≥ 1
  con `-p webfang_benchmark --features chromium`; `deny.toml` sin
  `RUSTSEC-2026-0183`/`RUSTSEC-2026-0184`. Salida con la convención del repo
  (`PASS=<n> FAIL=<n>`). **Prueba**: el harness es **RED contra el árbol pre-fix** por la
  razón del defecto (los crates están en el grafo), no por un stub que no entiende el
  pedido; **GREEN** después de T1–T6.
- [ ] T8 · S1–S4 — cablear T7 en `.github/workflows/ci.yml`, job `repo-guards`, junto a
  los demás pasos `scripts/test_*.sh`. **Nota de coordinación**: el mapa de colisiones del
  plan lista `ci.yml` para L2, L3, L4 y L13, y **no** para L9 → este paso crea un
  quinto escritor de `ci.yml` y necesita la serialización que el owner fije (o aplica la
  regla del propio plan: cada lote commitea *su* step en su rama y el step se re-aplica
  sobre el `main` nuevo al abrir la PR). Un harness que ningún workflow nombra es
  decoración.
- [ ] T9 · S1–S4 — verificación: `cargo nextest run -p webfang_core --lib autotuning`;
  `cargo nextest run -p webfang_benchmark` con y sin `--features chromium`;
  `bash scripts/test_build_graph.sh`; `bash scripts/check_advisory_policy.sh`;
  `cargo check` + `cargo clippy --all-targets --all-features -- -D warnings -W
  clippy::cognitive_complexity -W clippy::too_many_lines` + `cargo fmt --all -- --check` +
  `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps`.

---

## Log

L1 — **Qué se leyó, antes de escribir nada**:

- El plan madre `odd/tasks/backlog-implementation-plan.md` (checkout
  `fix-merge-when-green-merged-state`), leído entero hasta `## Log`. De ahí salen la
  fila L9, las decisiones D9/D18 y el estado de bloqueos O2/O6.
- Las tres issues, texto **verbatim** vía REST (la API GraphQL de este repo está floja):
  `curl -sS https://api.github.com/repos/XaviCode1000/webfang/issues/<n> | jq -r .body`
  para #1799, #1797 y #1796.
- El código real, en el checkout principal `main@3a785d8e` con `codedb` 0.2.5849
  (`codedb <raíz-absoluta> callers|word|status`) y lectura directa de los archivos.
  El worktree de este lote está en el mismo commit (`3a785d8e`), así que el código
  leído es el que se va a tocar.

L2 — **Petición original del owner (verbatim)**:

> tengo muchas y  varias issues abiertas y necesito un plan para ir implementando utliza la
> skill adecuada para triage de issues

L3 — **Correcciones del owner (verbatim)**:

> debes tener en cuenta que nuesto sistema ha cambiado sobre todo en lo relacionado con
> modelos de *AI LOCAL* Y LAS ISSUES pueden decir cosas o indormacion antigua o falsa,

> utiliza siempre tu y ordenar a  todos los workers las potentes tools *codedb* y *codegraph*
> en lugar de las otras tools

> te dejo la responsabilidad de decision a ti como experto, y si tienes que investigar en
> internet para buscar la mejor opcion o mejor alternativa tienes mi permiso total.

> pero tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro
> producto.

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

> y crea on plan odd para ir implementando todo

Decisión del owner que desbloquea este lote (verbatim, 2026-10-22, pregunta Q2 = O2):

> Los tres: sysinfo, keyring, serde_yaml

Y la salida de O6 (verbatim):

> **O6 salió de la tabla del owner** (2026-10-22): dropear `version_string()` (0 call sites) y
> **no** escribir `build.rs` es una decisión técnica dentro del código, no un pedido de
> permiso sobre deps. Queda como D18.

Ninguna de las tres issues habla de IA local, así que la advertencia del owner sobre el
ADR-0004 paso 6 no cambia ninguna premisa de este lote: no hay nada que descartar por
ese lado.

L4 — **La fila L9 del plan (verbatim)**:

> | **L9** | Grafo de build | #1799, #1797, #1796 | borrar `sys-info` (KiB en Linux,
> **bytes** en macOS) y `once_cell`; dropear `version_string()` (0 call sites) y no
> escribir `build.rs`; el benchmark declara su propia feature `chromium` |
> `autotuning.rs::ram_budget_from_bytes_converts_to_kib` | −20/+40 | worker |
> **DESBLOQUEADO** (O2 resuelta 2026-10-22) |

L5 — **Evidencia verificada con `codedb` / `codegraph` sobre `main@3a785d8e`** (símbolo,
path y call sites; lo que existe y lo que no):

- `codedb <root> status` → `files 1065 indexed`, `head 3a785d8ef3c0` (el índice del
  checkout principal, como manda la política de worktrees).
- `ram_budget_from` → **4 call sites, todos en
  `crates/webfang_core/src/infrastructure/autotuning.rs`**: `detect_ram_budget` y tres
  tests del módulo. **Cero** consumidores fuera del archivo.
- `detect_ram_budget` → **3 call sites, todos en el mismo archivo**:
  `resolve_ram_budget` y dos tests. `resolve_ram_budget` es la que usan el CLI y el
  pipeline; **su firma no cambia**, así que la onda de choque del rename es cero.
  `codegraph impact ram_budget_from` confirma el radio: los 5 símbolos afectados
  (`ram_budget_from`, `detect_ram_budget`, `resolve_ram_budget` y dos tests) están
  **todos** dentro de `crates/webfang_core/src/infrastructure/autotuning.rs`.
- `version_string` → **0 call sites** (`codedb <root> callers version_string`; `codegraph
  impact version_string` → "1 affected symbols", o sea sólo ella misma). El único uso de
  la metadata de build es `built_info::GIT_COMMIT_HASH_SHORT` /
  `built_info::BUILT_TIME_UTC` dentro de esa misma función, en
  `crates/webfang_core/src/lib.rs`. El `--version` de la CLI lo produce clap
  (`#[command(name = "webfang", version)]` en `crates/webfang_core/src/cli/args/mod.rs`),
  nunca `version_string()`.
- `sys_info` → **8 hits, todos en `crates/webfang_core/src/infrastructure/autotuning.rs`**
  (doc de módulo, doc de constante, doc de función y la llamada
  `sys_info::mem_info()`). `sys-info` no aparece en ningún otro manifest del workspace:
  `crates/webfang_core/Cargo.toml` lo declara como dependencia opcional bajo la feature
  `persistence`, y `Cargo.toml` (workspace) lo declara en `[workspace.dependencies]`.
- `once_cell` → **3 usos en código**, exactamente los que dice la issue:
  `crates/webfang_core/src/infrastructure/http/waf_engine.rs` y
  `crates/webfang_core/src/domain/scraper_port/dom_pruner.rs` (`sync::Lazy`) y
  `crates/webfang_core/tests/tokio_console_smoke_test.rs` (`sync::OnceCell`). Ningún otro
  crate del workspace lo declara.
- `sysinfo` (el crate sano) → **ya es dependencia directa** de `webfang_core` y ya se usa
  en `domain/budget/detector.rs`, `infrastructure/downloader/resource_governor.rs` e
  `infrastructure/downloader/system_ram_probe.rs`. El patrón a copiar está en
  `detector.rs::SystemDetector::total_ram_bytes` (`System::new()` + `refresh_memory()` +
  `#[cfg(not(miri))]`). ⇒ **No entra ninguna dependencia nueva**: el swap es una
  eliminación, no una adición.
- Grafo del lock: `built` 0.7.7 → `git2` 0.20.4 → `libgit2-sys` 0.18.5+1.9.4. El único
  dependiente de `built` es `webfang_core`; el único dependiente de `git2` es `built`.
  ⇒ quitar `built` se lleva los tres.
- `once_cell` en el lock: lo traen `ahash`, `indicatif`, `criterion`, `dashmap`, `sysinfo`,
  `insta`, `tempfile`, `async-std` y compañía ⇒ **permanece en el lock**, tal como dice la
  AC de la issue.
- `chromiumoxide*` en el lock: 4 crates (`chromiumoxide`, `chromiumoxide_cdp`,
  `chromiumoxide_pdl`, `chromiumoxide_types`). La feature `chromium` de `webfang_core` es
  `chromium = ["dep:chromiumoxide"]`; `webfang_cli` y `webfang_mcp` la reenvían pero **no
  la activan** por defecto. El único que la activaba sin querer era
  `crates/webfang_benchmark/Cargo.toml` (`webfang_core = { …, features = ["chromium"] }`).
- `codegraph` sobre el benchmark: `STRATEGIES` tiene **1 caller** (`run_all`) y
  `run_all` tiene dos consumidores (`crates/webfang_benchmark/src/bin/bench_tier_a.rs` y
  `crates/webfang_benchmark/tests/e2e_smoke_test.rs`) ⇒ gatear el tier Full toca **dos**
  archivos, no más.
- Miri: el lane `Miri (core)` interpreta el universo de tests de `webfang_core`, así que
  los tests in-module de `autotuning.rs` **corren** bajo Miri. `sysinfo` llama `sysconf`
  en el constructor de `System` (`sysinfo-0.30.13`, `src/unix/linux/system.rs`:
  `SystemInfo::new`), que Miri no puede ejecutar ⇒ el gate `#[cfg(not(miri))]` no es
  opcional: sin él, el lane de Miri se rompe.
- `deny.toml`: los dos ignores `RUSTSEC-2026-0183` y `RUSTSEC-2026-0184` existen y su
  razón escrita dice *"reachable only through the built build-dependency of
  webfang_core"*. Con `built` fuera del grafo, la promesa queda vacía.
- `fuzz/Cargo.lock` **también** contiene `built`, `git2` y `libgit2-sys` (fuzz depende de
  `webfang_core` por path y no es miembro del workspace) ⇒ hay que regenerarlo, y el
  guard `scripts/check_advisory_policy.sh` audita **los dos** lockfiles.
- `built` 0.7.7 **no emite ningún** `cargo:rerun-if-changed` (grep en su `src/`), y el
  `build.rs` del repo sí emite `cargo:rerun-if-changed=build.rs` ⇒ el hash queda stale.
  Premisa de la issue **confirmada**.
- `docs/book/webfang-docs-llm/` (donde aparece `version_string()` en la documentación
  generada) **no está versionado** (`git ls-files` no lo lista): es un artefacto que
  `scripts/build-llm-artifact.sh` regenera en el workflow de docs ⇒ borrar la función no
  deja documentación stale en el árbol.

L6 — **Premisas de las issues que el código contradice** (no se toman como verdad):

- **P1 — la unidad de `sys-info` en macOS (FALSA, y es la premisa central de #1799 y de
  la decisión D9 del plan).** La issue afirma que "`sys-info` 0.9 devuelve KiB en Linux y
  **bytes** en macOS" y el plan lo repite ("hoy el budget de RAM ya se sobreestima en
  macOS"). Verificado en el código fuente de `sys-info` 0.9.1 (la versión que resuelve el
  lock): **KiB en todas las plataformas**. En `c/darwin.c`, `get_mem_info()` hace
  `size /= 1024` sobre `HW_MEMSIZE` y devuelve KiB; en `c/windows.c`,
  `stat.ullTotalPhys / 1024`; en `c/freebsd.c`, `hw.realmem / 1024`; en Linux, el
  `MemTotal` de `/proc/meminfo` (kB); en Solaris/illumos, `pages.physmem * pagesize /
  1024`. ⇒ **No hay bug de sobreestimación en macOS**: el código de hoy es aritméticamente
  correcto en todas las plataformas. El problema real es otro y es peor que una
  sobreestimación: `sys-info` **no documenta la unidad** (`/// Total physical memory.`),
  y por eso el nombre del parámetro (`total_kib`) es lo único que la fija. La migración
  a `sysinfo` (que **sí** documenta "Returns the RAM size in bytes") convierte una
  convención invisible en un contrato explícito. Consecuencia para el RED: **no existe un
  RED de comportamiento en Linux** (pre-fix y post-fix dan el mismo número en esta
  máquina); el RED honesto del lote es de **grafo** (T7) y el test nombrado es un **pin de
  contrato**, no una reproducción de un bug.
- **P2 — "hoy da 6" (caducado).** El lock actual tiene 4 crates `chromiumoxide*`, no 6.
  El mecanismo (unificación de features con `resolver = "2"`) sigue siendo el descrito;
  el número es una medición de 2026-10-03 y hay que re-medirlo al implementar.
- **P3 — el `build.rs` propio (rechazada por decisión del owner, D18).** La issue pide
  un `build.rs` que genere `GIT_COMMIT_HASH_SHORT` y `BUILT_TIME_UTC`; la función que
  lo alimentaría tiene 0 call sites. Implementarlo sería escribir metadata falsa con un
  arista de build propia. Decisión del owner: dropear la función y **no** escribir el
  `build.rs`. La mitad de la AC que sí se cumple es "`built`, `git2`, `libgit2-sys` fuera
  de `Cargo.lock`" (y de paso, las dos advisories de UB de `git2` desaparecen).
- **P4 — "el test `test_ram_budget_from_32gib_machine` ya fija la conversión KiB→bytes"
  (matiz).** Fija la conversión *interna* (KiB→bytes al aplicar el 70 %), no la de la
  frontera (bytes→KiB al leer del sistema). Ese matiz es exactamente el agujero por el que
  pasó el valor equivocado: el test fijaba la mitad de la conversión que sí funcionaba.
  Por eso el test nuevo se llama `ram_budget_from_bytes_converts_to_kib` y asserta la
  equivalencia entre las dos mitades.
- **P5 — "el hash queda stale hoy" (CONFIRMADA, no falsa).** Ver L5: `built` 0.7.7 no
  emite `rerun-if-changed` y el `build.rs` del repo sí → stale. El defecto desaparece
  porque el consumidor desaparece.

L7 — **Decisiones que tomo en este lote, y por qué** (todas dentro del código; ninguna
pide permiso al owner):

1. **Un solo contrato de unidad en la superficie pública.** `ram_budget_from_bytes` es
   la única función pública que recibe la lectura del sistema; la regla en KiB queda
   privada (`ram_budget_from_kib`). Se borra el nombre ambiguo `ram_budget_from` en vez de
   dejar dos funciones paralelas que se distinguen sólo por la unidad del argumento: dos
   funciones que se distinguen por una unidad invisible son la forma exacta del defecto
   que se está cerrando. (0 call sites; el repo no publica a crates.io ⇒ sin obligación
   de semver.)
2. **El gate `#[cfg(not(miri))]` se suma al gate de `persistence`.** El issue pide
   copiar el patrón de `detector.rs`, y ese patrón incluye el gate de Miri. Sin él, el
   lane `Miri (core)` se rompe al leer `sysinfo` (L5).
3. **Se purgan los dos ignores de `deny.toml`.** Su razón escrita se refiere
   exclusivamente a `built`; dejarlos sería dejar una justificación falsa en el archivo
   que es la fuente de verdad de la política de advisories. Con el "RE-ADD PROTOCOL" del
   propio archivo, re-entrar es un acto explícito, no un olvido.
4. **El benchmark no corre un tier que no puede ejecutar.** `STRATEGIES` incluye
   `JsStrategy::Full` sólo con la feature `chromium`: medir un brazo que va a fallar
   produce una fila de reporte con errores, no una medición.
5. **La feature `chromium` del benchmark reenvía, no activa.** `chromium =
   ["webfang_core/chromium"]`. Si activara el navegador por sí misma, el grafo default
   seguiría contaminado y el lote no habría arreglado nada (es la lección de D8 en L8:
   con `default-features = false` y con la unificación de features, la unica forma de
   que el grafo default quede limpio es que nadie pida la feature).
6. **Un harness de shell para el lote.** Tres de los cuatro cambios son de grafo y no
   tienen comportamiento observable ⇒ sin harness no hay RED/GREEN. T7 lo cubre, con
   control positivo por check (un harness que puede dar verde por construcción no es
   evidencia).

L8 — **Colisiones y riesgos que el owner debe conocer**:

- `Cargo.toml` (workspace + crates): el mapa de colisiones del plan ya lo serializa
  **L9 → L10**. L10 (borrar `serde_yaml`, salir de `keyring`) entra después.
- `.github/workflows/ci.yml`: el mapa lista L2, L3, L4 y L13; **L9 no está**. T8 crea un
  quinto escritor de ese archivo. O el owner serializa, o se aplica la regla del plan
  ("cada lote commitea *su* step en su branch; al abrir la PR el step se re-aplica sobre
  el `main` nuevo").
- `deny.toml`: el mapa no lo lista. L10 también toca la política de advisories (D12
  habla de `serde_yaml` y de `paste`) ⇒ posible solape de entradas, a serializar.
- `scripts/duplicate-crate-baseline.txt`: `git2`/`libgit2-sys` no están en el baseline
  (no hay duplicados de esos crates), así que la convergencia del grafo no exige tocarlo.
  El propio script reporta las entradas que sobran como advertencia, no como falla.
- Riesgo de Miri: si el gate `#[cfg(not(miri))]` se olvida, el lane `Miri (core)` se
  rompe; por eso T1 lo exige explícitamente.
- Riesgo de `cargo audit`: si `git2` siguiera en `fuzz/Cargo.lock` (por no regenerar ese
  lock), purgar los ignores dejaría el Security Audit en rojo ⇒ T4 y T5 van juntos, y T5
  verifica la premisa antes de purgar.

L9 — **Estado de la entrega**:

- Worktree: `/home/xavi/Projects/Rust/webfang-worktrees/fix-build-graph-deps`, rama
  `fix/build-graph-deps`, base `3a785d8e`.
- Este documento es **sólo especificación**: no se escribió código de producción, no se
  corrió `cargo`, no se commiteó nada. El árbol del worktree tiene exactamente los dos
  archivos nuevos de este lote.
- Sin PR (decisión del owner: *"no abras pr's conserva el trabajo en worktree comiteado.
  ya mas tarde abrimos las pr's."*).
- Verificación de **esta** entrega: `git status --short` → sólo `?? odd/build-graph-deps/`
  y `?? odd/tasks/build-graph-deps.md` (los dos archivos nuevos, nada más);
  `bash -n scripts/ci-health-close.sh` → exit 0 (script de otro lote, no tocado);
  el documento tiene `## Specs` → `## Tasks` → `## Log` en ese orden, y sus 24 bloques
  `>` se compararon carácter a carácter contra el texto de las tres issues (REST) y del
  plan: **0 citas no literales**.

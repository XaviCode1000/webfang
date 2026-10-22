# Falsos verdes de test: el crash-harness puede colgarse, la suite MCP se salta en silencio, la matriz de crash no cubre lo que dice cubrir y el cierre de consola de Windows nunca se ejecutó

Objetivo: cerrar cuatro clases de **verde falso** que el triage del backlog dejó juntas y que
comparten una sola raíz — *una prueba que no puede distinguir un script roto de uno
arreglado*:

1. el harness de crash-injection **se cuelga para siempre** cuando el kernel descarta el
   `SIGKILL` (init de un PID-namespace) y la lane de crash-matrix se queda esperando el
   `slow-timeout` de nextest en vez de morir con un diagnóstico;
2. la invocación default de `cargo nextest run` **salta 151 tests de `webfang_mcp`** y
   reporta verde;
3. la matriz de crash **anuncia una fila que no existe** (`during_cancel_drain`) y el
   almacén SQLite del repo **no tiene ni un `PRAGMA integrity_check`**;
4. el evento real de cierre de consola de Windows **nunca se ejecutó**: nadie drove un
   `CTRL_CLOSE_EVENT`, y su código sólo lo compila la lane de CI.

Rama: `fix/false-green-tests`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-false-green-tests`.

Cierra: #2066, #1852, #1828, #1834, #1816. **No** cierra: #1833 — su propuesta revierte una
decisión registrada y la decisión es del owner (O4, ver S7 y el Log).

Sin dependencias: no toca `crates/webfang_mcp/src/` (L7), ni `docs/adr/0004-onnx-local-removal.md`
(L0), ni el guard de SSRF (K1, worktree del owner). Colisiones de path declaradas en el Log.

---

## Specs

### S1 · #2066 — el `SIGKILL` descartado no puede colgarse

El defecto, verbatim de la issue:

> `kill_self()` no debería poder colgarse. Si la señal no va a entregarse, el proceso tiene que
> morir de una forma **diagnosticable** — como hacía el `abort()` incondicional anterior — y no
> esperar indefinidamente.

> Con la señal descartada, el proceso entra en `loop { libc::pause() }` y nunca sale: la lane de
> crash-matrix se cuelga hasta el `slow-timeout` de nextest (60s ×3 + 10s, `.config/nextest.toml:30`)
> en vez de fallar con un mensaje que diga qué pasó.

La reproducción, verbatim:

> - La reproducción de arriba: `kill()` devuelve 0 y el proceso **sobrevive** (`exit=0`) como init de
>   namespace; en baseline muere (`exit=137`). Es el mismo `kill` + `pause` que usa `kill_self()`,
>   sin Rust de por medio.

**Código a cambiar:** `crates/webfang_core/src/cli/crash_points.rs`, función `kill_self` (brazo
`#[cfg(unix)]`). El defecto está **vivo** en `main@3a785d8e` (ver Log L5): el brazo unix ya
espera la señal con `loop { libc::pause() }` y su comentario afirma que la espera "always ends in
the process dying by the very signal the matrix asserts on — never in a hang".

**Decisión (fix por raíz):** el descarte del kernel es una propiedad **del PID del proceso**, no
del `errno` de `kill(2)`: un `SIGKILL` enviado **desde dentro** del propio namespace al init de
ese namespace se descarta aunque `kill()` devuelva 0. La única señal observable es el PID: en ese
entorno `getpid() == 1`. Por eso la detección va **antes** de enviar la señal (enviarlo y esperar
es exactamente el cuelgue), y la muerte es **diagnosticable**: el proceso aborta con un mensaje que
nombra el descarte, en vez de quedarse esperando una señal que no va a llegar.

Restricción de diseño (del triage, no de la issue): **sin flag, sin variable de entorno y sin
segundo camino de muerte**. La rama que ya existe para el `kill` rehusado y la nueva rama del init
de namespace **comparten un único camino de muerte diagnóstica**, para que la nueva rama quede
cubierta por el mismo test que cubre la anterior.

El hallazgo secundario de la issue:

> - La rama de rechazo (`crash_points.rs:126-130`) **no está cubierta por ningún test**.

> - El `tracing::error!` que la precede corre justo antes de `std::process::abort()`: con un
>   subscriber con buffer o non-blocking ese log puede perderse, y el `errno` que la rama existe
>   para preservar se va con él — que es exactamente lo que dejó el fallo de macOS sin diagnóstico
>   en #2063.

**Prueba nombrada:** `crates/webfang_core/tests/crash_matrix_test.rs::crash_point_never_hangs_when_sigkill_is_discarded`
(el nombre lo fija el plan madre; el archivo ya tiene diez filas con ese estilo de nombre, sin
prefijo, y el guard de naming `scripts/check_regression_naming.sh` no lo marca: el nombre no
contiene `test`/`issue_`/`pr_`/`crawler_`/`waf_`).

El RED tiene que ser **preciso** (regla D3 del plan): contra el código pre-fix, el hijo **se
cuelga** y el test falla con el síntoma real ("el hijo no terminó dentro de la ventana"), no con
un error de arreglo. El RED se consigue reproduciendo el entorno de la issue: el hijo se lanza
como init de un PID-namespace (`unshare --user --map-root-user --fork --pid`), que es la única
forma de que `getpid() == 1` sin ser el init real de la máquina.

Sobre el log que "puede perderse": verificado en `crates/webfang_core/src/infrastructure/observability/logging.rs`,
función `init_json_logging_dual` — la capa de texto escribe siempre en **stderr** con
`std::io::stderr` (bloqueante) y el `EnvFilter` de `--quiet` (`webfang=warn`) deja pasar un
`error!`; la única capa no-bloqueante es la de archivo, que es opcional (`--trace-file`) y no es el
canal que lee la matriz. Por lo tanto el diagnóstico **no necesita un segundo canal**: un solo
`tracing::error!` estructurado (con el `reason` y, en la rama del `kill` rehusado, el `errno`) es
suficiente y es lo que el test puede afirmar leyendo el stderr del hijo.

### S2 · #1852 — la invocación default de `webfang_mcp` no puede saltarse 151 tests

Criterios de aceptación, verbatim de la issue:

> - [ ] `cargo nextest run -p webfang_mcp` (sin flags) ejecuta los 22 archivos gated, no 515 tests
> - [x] ~~`cargo nextest run -p webfang_ai` sin flags NO sale con "no tests to run"~~ — **eliminado**:
>   el crate no existe (ADR-0004 paso 6)
> - [ ] Existe un guard que detecte un target de test declarado que compila a cero tests, para que
>   esto no vuelva por otra puerta
> - [ ] La suite default del workspace reporta el conteo real esperado en un test o assertion de CI
>   (evita el verde falso futuro)

El segundo criterio ya está eliminado **por la propia issue** y no es trabajo de este lote: el crate
`webfang_ai` no existe (verificado: `crates/` tiene cinco crates y ninguno es `webfang_ai`; el
`Cargo.toml` del workspace no declara `ort`/`ort-sys`/`hf-hub`/`tokenizers`).

**Código a cambiar:**

- `crates/webfang_mcp/Cargo.toml`, bloque `[features]`: agregar `default = ["mcp"]`. La feature
  `mcp` es un **marker vacío** en el otro lado (`webfang_core`'s `mcp = []` no compila ninguna
  rama: no hay ni un `cfg(feature = "mcp")` en `crates/webfang_core/src/`), así que encenderla por
  defecto no cuesta build ni dependencias — la issue lo dice: *"Agregar `default = ["mcp"]` no
  cuesta nada en build ni dependencias. **Recomendado.**"*.
- `crates/webfang_test_utils/Cargo.toml`, dependencia `webfang_core`: `default-features = false`
  (decisión D8 del plan, autorizada por el owner: *"Sí: default=["mcp"] + default-features=false"*).
  Verificado: hoy no cambia nada observable, porque el `default` de `webfang_core` es `default = []`
  (vacío, desde #1813 T4) — es una **inmunización** frente a que el default de `webfang_core`
  vuelva a ser no vacío, que es exactamente el riesgo que D8 nombra.

**Prueba nombrada:** el criterio 1 se verifica con el conteo real
(`cargo nextest list -p webfang_mcp` antes y después) y queda **asertado** por el guard de S3, que
es lo que lo vuelve a bloquear si alguien quita el `default`.

### S3 · #1852 — un guard que detecte un target de test que compila a cero tests

Criterio, verbatim:

> - [ ] Existe un guard que detecte un target de test declarado que compila a cero tests, para que
>   esto no vuelva por otra puerta

> - [ ] La suite default del workspace reporta el conteo real esperado en un test o assertion de CI
>   (evita el verde falso futuro)

**Lo que ya existe y lo que no** (verificado, ver Log L5): `scripts/check_dead_tests.sh` detecta
los **archivos** de test que cargo no descubre (un `.rs` huérfano en `tests/`), **no** los targets
que cargo descubre y que compilan a cero tests. El defecto de #1852 es exactamente el segundo caso:
el archivo se descubre, el target se construye, y el binario no tiene ni un test. El guard nuevo
es hermano de aquél, no una segunda versión del mismo.

**Código a cambiar:**

- `scripts/check_empty_test_targets.sh` (nuevo): guard **estático** (bash + `awk`/`grep`, sin cargo,
  sin red) que, para cada archivo de test de integración del workspace:
  1. lee su gate de crate (`#![cfg(...)]`);
  2. si el gate nombra una feature del paquete que **no** está en el `default` de ese paquete, el
     target compila a cero tests en el build por defecto ⇒ exige que el paquete lo declare
     **`[[test]]` con `required-features`** (la declaración nativa de cargo para un target
     condicional; `AGENTS.md` lo autoriza explícitamente: *"Only add a `[[test]]` entry ... for cases
     auto-discovery does not cover (subdirectories such as `tests/compile_fail/`, special features)"*);
  3. si el target está en el `default` (o su gate es condicional ya declarado), no hay nada que
     hacer.
  Un target gated que no cumple ninguna de las dos condiciones ⇒ **fallo**, nombrando archivo,
  feature y las dos formas de arreglarlo. Sin allow-list, sin baseline, sin flag.
- `crates/webfang_core/Cargo.toml`: declarar los cuatro targets gated de `webfang_core` con
  `required-features` (`sqlite_integration` → `persistence`, `stress_test` → `console`,
  `tokio_console_smoke_test` → `console`, `adaptive_selector_integration` →
  `adaptive-selectors`). Es el precio del guard: hoy esos cuatro targets compilan a cero tests en
  el build por defecto **en silencio** — el mismo defecto que #1852 denuncia, en el otro crate.
- `scripts/test_empty_test_targets.sh` (nuevo): harness de negación del guard, sobre árboles
  `mktemp` (bash puro, sin cargo, sin red, sin tocar el repo), con los cuatro casos: *gated +
  feature en `default`* ⇒ PASS; *gated + feature fuera del `default` + sin `required-features`* ⇒
  FAIL (y el mensaje nombra el archivo y la feature); *gated + `required-features` declarado* ⇒
  PASS; *sin gate* ⇒ PASS.
- `.github/workflows/ci.yml`, job `repo-guards`: un step que corra el harness. Un harness que
  ningún workflow nombra es decoración (regla del lote L2).

**El criterio 4 (el conteo real en el default)** queda cubierto por la misma regla, en forma
estructural: si ningún target declarado compila a cero tests en el build por defecto, el default
**contiene** los 22 archivos gated de `webfang_mcp` y el conteo default deja de ser 515. El guard
no necesita un número mágico que se pudra: la propiedad que se afirma es "ningún target declarado
queda vacío en el build por defecto", y esa propiedad es la que evita el verde falso futuro.

**Fuera del alcance de este guard (y por qué):** `crates/webfang_core/tests/tokio_console_smoke_test.rs`
está gated por `#![cfg(all(feature = "console", tokio_unstable))]`, y `tokio_unstable` es un **cfg**
— no una feature de cargo: el workspace nunca lo habilita, así que ese target está vacío **en
todos** los builds, incluso con `--all-features`. Declararlo con `required-features` lo hace
explícito pero no lo hace correr: hacerlo correr exige tocar la feature `tokio_unstable` de `tokio`
(cambio de dependencias) o borrar el archivo (decisión del owner). Queda registrado en el Log
(L6), no resuelto en este lote.

### S4 · #1828 — la matriz de crash debe cubrir la fila que ella misma anuncia

Criterios, verbatim:

> - [ ] Each of the six rows above has a test, or a recorded decision explaining why it cannot have one
> - [ ] `docs/persistence-resume.md:156` and `crash_matrix_test.rs:203` agree with reality
> - [ ] Every "EXISTS" test listed above is left untouched

La fila que falta, verbatim del issue:

> - **`during_cancel_drain`** — armed in production (`application/export_factory.rs:823-826`, const at
>   `cli/crash_points.rs:46-47`) but absent from the matrix. `tests/crash_matrix_test.rs:203-204`
>   says "*reserved for PR5b and intentionally absent*"; the ten rows at `:208-255` cover none of them.
>   Worse, `docs/persistence-resume.md:156` advertises point 9 as a matrix row that does not exist.
>   PR5b landed the const, the arming and the doc — never the test.

**Verificado** (Log L5): la constante `DURING_CANCEL_DRAIN` existe en
`crates/webfang_core/src/cli/crash_points.rs`; su **único** punto de disparo está en
`crates/webfang_core/src/application/export_factory.rs` (función `process_results`, rama
`SingleItemOutcome::Cancelled`); y `crates/webfang_core/tests/crash_matrix_test.rs` tiene diez
filas y un comentario que reserva la undécima. Es decir: **el defecto está vivo** — el doc y la
suite no coinciden.

Y la desalineación es **de tres entradas**, no de una: la tabla de `docs/persistence-resume.md`
enumera **nueve** puntos, de los cuales ocho existen en la matriz; le sobra
`during_cancel_drain` (que no existe) y le faltan `mid_extraction` y `post_extraction_pre_pipeline`
(que sí existen y tienen fila). La AC 2 pide que *"agree with reality"*: el doc tiene que listar
las once filas, no nueve.

**Código a cambiar:**

- `crates/webfang_core/tests/crash_matrix_test.rs`: agregar la fila `during_cancel_drain` y
  **borrar** el comentario "reserved for PR5b" (o el doc y la suite vuelven a discrepar para
  siempre — es el bloqueador estructural que la propia issue nombra). La fila usa la misma
  máquina de la matriz (crash → resume → cuatro invariantes globales) y necesita **una** pieza
  nueva: entregar `SIGINT` en un momento determinista. El patrón ya existe en el repo y es
  reutilizable tal cual: `crates/webfang_core/tests/behavioral/cli/signal_lifecycle_test.rs`
  entrega la señal cuando wiremock ha visto la k-ésima petición (no con un sleep), que es la
  única forma de que el test sea determinista.
- `docs/persistence-resume.md`: la tabla de la matriz de crash tiene que decir la verdad: listar
  las **once** filas (las diez actuales + `during_cancel_drain`), incluyendo las dos que hoy no
  lista (`mid_extraction`, `post_extraction_pre_pipeline`). La AC lo pide explícitamente:
  *"agree with reality"*. `AGENTS.md` no toca `docs/` salvo pedido explícito; acá el pedido es
  explícito.

**Prueba nombrada:** `crates/webfang_core/tests/crash_matrix_test.rs::crash_during_cancel_drain_recovers_with_zero_loss`
(el archivo no usa prefijo `test_`; es su convención local y la que usa el plan para la fila de S1).

### S5 · #1828 — la fila WAL-kill se rechaza; el `PRAGMA integrity_check` entra donde los datos ya están

Criterio, verbatim:

> - [ ] Each of the six rows above has a test, or a recorded decision explaining why it cannot have one

La fila WAL-kill, verbatim del issue:

> - **WAL kill + `PRAGMA integrity_check`** — no `integrity_check` / `wal_checkpoint` / `quick_check`
>   exists anywhere in the repo.

> - The WAL-kill row has no arming point: `cli/crash_points.rs` covers JSONL / state / lock only, so
>   this needs a **production** ingest-side arming point first.

**Decisión D16 del plan madre (verbatim):** *"#1828 → la fila WAL-kill se **rechaza**; `PRAGMA
integrity_check` sobre las filas existentes — La fila WAL exigiría un punto de arming **nuevo** en
producción para servir a un test: un verbo nuevo en el contrato del crash-harness"*.

**Alcance real (verificado, Log L5):** la matriz de crash **no toca SQLite**: el store de la
matriz es el `RecordStore`/`ExportState` en JSON (`crates/webfang_core/src/infrastructure/export/`),
y el único superficie SQLite del repo es
`crates/webfang_core/src/infrastructure/persistence/sqlite.rs` (pool `deadpool_sqlite`, `INIT_PRAGMAS`
con `journal_mode = WAL`, DDL congelado). Por eso "el `PRAGMA integrity_check` sobre las filas
existentes" **no puede** ser una fila de la matriz: se implementa sobre la única superficie SQLite
del árbol, y sobre las filas que **ya** escribe su test de integración existente.

**Código a cambiar:**

- `crates/webfang_core/tests/sqlite_integration.rs`: un test nuevo que (a) crea el pool en un
  `TempDir`, (b) corre `setup_schema`, (c) escribe `resources` + `chunks`, (d) **reabre** el pool
  (el reinicio es el análogo del "reabrir después del kill"), (e) corre `PRAGMA integrity_check` y
  afirma que devuelve `ok`, y (f) afirma que las filas sobreviven. Esto es el `integrity_check` que
  el audit pedía (`CHAOS_TESTING_PLAN.md`, punto 6) **sin** el kill, porque el kill exigiría el
  punto de arming nuevo que D16 rechaza.
- Las **otras** cuatro filas de la AC 1 y el `cfg(loom)` de la AC 3 quedan con **decisión
  registrada** (la issue las admite: *"or a recorded decision explaining why it cannot have one"*;
  *"or the omission is documented as accepted"*). Las decisiones, con su fundamento, están en el
  Log L6; se cierran en el issue con la evidencia, no con promesas.

**Prueba nombrada:** `crates/webfang_core/tests/sqlite_integration.rs::test_sqlite_rows_pass_integrity_check_after_reopen`
(este archivo **sí** usa el prefijo `test_`; convención del repo para integración).

### S6 · #1834 — el evento real de cierre de consola, ejecutado de verdad

Criterios, verbatim de la issue:

> - [ ] A `#[cfg(windows)]` test delivers a real console close event and asserts the drain path ran,
>   skipping cleanly when there is no console.
> - [ ] The test is documented in `docs/test-inventory.md` per the repo's test-inventory convention.
> - [ ] It runs in the `test-crossplatform` lane on `windows-latest` and is not silently skipped
>   there — if it cannot be made deterministic, say so in the test and say so in the lane.
> - [ ] The residual 5-second OS deadline is either respected by the assertion or explicitly
>   documented as why it is not.

Y la decisión que la issue pide tomar **por adelantado**, verbatim:

> `CTRL_CLOSE_EVENT` has a ~5-second deadline before the console host kills the process (documented
> in `docs/adr/0016-interruption-resume-lifecycle.md`). A test that asserts a *full* drain on close
> is asserting something the OS does not guarantee on a slow machine. Decide up front whether the
> assertion is "the drain path ran" (deterministic) or "everything was persisted" (racy), and encode
> that decision in the test name.

**Decisión:** la aserción es **"el camino de drenaje corrió"** (determinista), no "todo quedó
persistido" (con carrera contra el reloj del host). El nombre del test lo codifica.

**Por qué el test es *in-module*** (y no de integración), verificado: el registro del handler
(`fn install`) y el canal de fan-out (`static EVENTS`) son **privados** del módulo, y
`handler::ensure_source()` es `pub(super)`. Un test de integración no puede tocar ninguno de los
dos: sólo puede observar la API pública. El test *in-module* sí puede instalar el handler y
observar el evento. Ésta es la razón técnica del "in-module" del plan madre.

**Código a cambiar:**

- `crates/webfang_core/src/infrastructure/platform/windows_console.rs` (módulo `tests`, ya
  existente): un test `#[cfg(windows)]` que
  1. instala el handler (`handler::ensure_source()`),
  2. lanza un **proceso hijo** que llama `GenerateConsoleCtrlEvent(CTRL_CLOSE_EVENT, 0)` — la vía
     documentada por la issue; el hijo tiene que ser otro proceso porque el evento se dirige al
     grupo de procesos,
  3. espera `console_event()` con un timeout acotado y afirma que resuelve con
     `"CTRL_CLOSE_EVENT"`,
  4. y **omite limpiamente** (con el motivo impreso, no en silencio) cuando `GetConsoleWindow()`
     es null.
  Las dos APIs Win32 que hacen falta (`GenerateConsoleCtrlEvent`, `GetConsoleWindow`) están en
  `windows_sys::Win32::System::Console`, y `windows-sys` ya se declara con
  `Win32_System_Console` para `cfg(windows)` en `crates/webfang_core/Cargo.toml` ⇒ **ninguna
  feature y ninguna dependencia nueva**.
- El **generador** (el hijo) tiene que ser invocado explícitamente por el test padre y **no**
  puede correr como parte de la suite normal (dispararía el cierre de consola contra el proceso de
  test). La vía que no agrega estado ni archivos es re-ejecutar el propio binario de test con
  `--ignored --exact <nombre del generador>`, con el generador marcado `#[ignore]`.
- `docs/test-inventory.md`: el `#[ignore]` nuevo tiene que estar catalogado, porque
  `scripts/check_ignored_guard.sh` falla cerrado ante cualquier deriva del presupuesto congelado
  de `#[ignore]`. **Ésta es** la "test-inventory convention" que la AC 2 menciona: el catálogo es
  de `#[ignore]`, no un índice de todos los tests (el inventario legible por máquina,
  `docs/test-inventory.json`, lo genera `scripts/ci_test_inventory.py` y no lleva edición manual).
- `.github/workflows/ci.yml`: el comentario de la lane `test-crossplatform` tiene que decir que
  el test corre en `windows-latest` y qué se decide cuando no hay consola (la AC 3: *"say so in
  the test and say so in the lane"*). La lane corre `cargo nextest run --lib` y
  `cargo nextest run --test '*'`, así que un test *in-module* corre ahí.

**Prueba nombrada:** `crates/webfang_core/src/infrastructure/platform/windows_console.rs::a_real_console_close_event_reaches_the_termination_source`
(in-module ⇒ sin prefijo, por la convención del repo; el nombre dice "reaches the termination
source" y no "everything was persisted" — ésa es la decisión de la AC 4 codificada en el nombre).

### S7 · #1833 — no se implementa: la propuesta revierte una decisión registrada

Criterios, verbatim de la issue:

> - [ ] `bash scripts/check_windows_platform.sh` exits non-zero on a deliberately introduced compile
>   error in a `cfg(windows)` arm, and zero when the module is correct.
> - [ ] The script reproduces the workspace lint table (a bare `unsafe` in the module fails it).
> - [ ] Its failure message names the missing rustup target and the exact command to add it.
> - [ ] It does not require an MSVC toolchain, and does not attempt to link.
> - [ ] Documented in AGENTS.md next to the platform-guard convention, or wired into
>   `scripts/ci_fast_gate.sh`.

**Verificado (Log L5):** la decisión que la propuesta revierte está registrada en
`odd/tasks/windows-console-lifecycle.md`, tabla "Decisions taken by the maintainer (2026-09-30)":

> | How the Windows-only code gets compiled | **the CI `test-crossplatform` lane (`windows-latest`)
> only** | `rustup target add x86_64-pc-windows-msvc` was **declined**. Local `cargo
> check`/`clippy`/`nextest` therefore do **not** compile the `#[cfg(windows)]` arms. First
> type-check of the Win32 code happens in CI. |

El mismo documento registra el truco del crate scratch como **procedimiento manual**, no como
gate:

> What does work: an **isolated scratch crate** carrying a byte-identical copy of the module ...
> It found the missing `warn` import in one run.

Es decir: la issue #1833 no descubre un hueco nuevo — **re-litiga** una decisión del maintainer del
2026-09-30. El plan madre la dejó explícitamente como decisión del owner (**O4**, *"PENDIENTE"*):
*"#1833: su propuesta revierte una decisión registrada (la lane CI es el único compilador de
Windows)"*.

**Alcance real de este lote:** ninguno. No se escribe `scripts/check_windows_platform.sh`, no se
toca `AGENTS.md`, no se cablea nada en `scripts/ci_fast_gate.sh`. Lo que este lote **sí** aporta es
la evidencia de que la decisión sigue vigente y de que su cobertura es real: el test *in-module* de
S6 **corre** el brazo `#[cfg(windows)]` en la lane `windows-latest` (compila y ejecuta), que es
exactamente el tipo-check que #1833 quiere duplicar en local. La issue queda **comentada con la
evidencia** (no cerrada) a la espera de O4; si el owner resuelve O4 a favor del script, ese
slice se abre después de este lote.

### S8 · #1816 — el pacing tiene que ser observable, y la barrera del flush tiene que estar escrita

Criterios, verbatim de la issue:

> - [ ] At least one 429 fixture returns a non-zero `Retry-After` and a CLI test asserts observed
>   pacing and exit 69
> - [ ] A behavioral test sends SIGINT while the crawl is parked on the rate limiter
> - [ ] A test appends during an in-flight `flush()` and asserts durability on return
> - [ ] `application/pipeline/` has limit / cancellation / export-I-O coverage
> - [ ] The four weak asserts, the store size-bound gap and the three N-1 names are resolved or
>   explicitly waived

**El primer criterio es la raíz de la issue**, verbatim:

> ### 1. Every 429 mock pins `Retry-After: 0`, so pacing cannot be tested

> `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs:52,136` and
> `tests/common/fixture_server.rs:363-368` all set `retry-after: "0"`. There is no CLI-level test
> that a 429 with a real `Retry-After` produces observed pacing and exit 69.

**Verificado (Log L5):** los tres mocks existen y fijan `Retry-After: 0`. Y el camino del exit 69
está en el código: `HttpError::RateLimited` → `CrawlError::RateLimited` →
`ScraperError::RateLimited` → `ErrorClass::TransientBackoff` →
`default_exit_code_for_class(TransientBackoff) = Some(EXIT_UNAVAILABLE)` (69). El pacing se respeta
en `crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs`: la rama unificada de
429/5xx duerme `max(Retry-After, backoff exponencial)`, con `parse_retry_after_ms` **capado** por
`backoff_max_ms` (10 000 ms por defecto) y `max_retries = 3` por defecto.

**Código a cambiar:**

- `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs`: un fixture 429 que devuelve un
  `Retry-After` **no nulo** (1 s) y un test que afirme (a) el **pacing observado** —el reloj de
  pared del run es ≥ el `Retry-After` pedido, que es una cota inferior y por lo tanto no
  flakea— y (b) el **exit 69**. La aserción del pacing es una cota inferior: el run **no puede**
  terminar antes del `Retry-After`; por eso es determinista en la dirección segura.
- `crates/webfang_core/src/infrastructure/export/jsonl_writer.rs` (módulo `tests`): el criterio 3,
  **corregido**. Verificado: el writer es **un solo hilo con un canal**, y `append`/`flush` son
  mensajes sobre ese canal ⇒ la barrera de `flush()` cubre **lo que estaba encolado antes**, y un
  append emitido *durante* un flush en vuelo queda **detrás** de la barrera. La AC tal como está
  escrita ("asserts durability on return") afirma una propiedad que el diseño **no** tiene: sería
  un test que exige un contrato nuevo. El alcance real es un test que **escribe el contrato que
  existe**: "un append emitido durante un flush en vuelo es durable después del siguiente flush" —
  que es la garantía que importa (nada se pierde) y que es determinista.

**Los otros tres criterios quedan con decisión registrada** (waiver explícito, que la propia AC
admite: *"resolved or explicitly waived"*), con su fundamento en el Log L6.

**Pruebas nombradas:**

- `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs::issue_1816_waf_gauntlet_429_retry_after_is_observed_and_exits_69`
  (integración ⇒ convención `issue_<N>_` de `docs/regression-test-naming.md`).
- `crates/webfang_core/src/infrastructure/export/jsonl_writer.rs::an_append_issued_during_an_inflight_flush_is_durable_after_the_next_flush`
  (in-module ⇒ sin prefijo).

---

## Tasks

- [ ] T1 · S1 — `crates/webfang_core/src/cli/crash_points.rs`, función `kill_self`: detectar
  `getpid() == 1` **antes** de enviar la señal y morir por el camino diagnóstico único (el mismo
  que usa la rama del `kill` rehusado), con un `tracing::error!` que nombre el descarte y, en la
  rama del `errno`, que lo conserve. Sin flag, sin env var, sin segundo camino de muerte.
- [ ] T2 · S1 — `crates/webfang_core/tests/crash_matrix_test.rs`: test nuevo
  `crash_point_never_hangs_when_sigkill_is_discarded`. Lanza el binario real como init de un
  PID-namespace (`unshare --user --map-root-user --fork --pid`), con `WEBFANG_CRASH_AT` armado;
  espera con **ventana acotada**; afirma que el hijo **no se cuelga**, que **no** sale con éxito
  y que su stderr trae el diagnóstico. RED contra el código pre-fix = el hijo se cuelga y el test
  falla con el síntoma real. En plataformas sin namespaces de PID el test no existe (`cfg`), y en
  Linux, si el namespace no se puede crear, el test **falla** con el comando de remediación (no se
  salta en silencio: un test que no prueba nada es el verde falso que este lote cierra).
- [ ] T3 · S2 — `crates/webfang_mcp/Cargo.toml` (`[features] default = ["mcp"]`) y
  `crates/webfang_test_utils/Cargo.toml` (`webfang_core` con `default-features = false`). Se
  verifica con el conteo real: `cargo nextest list -p webfang_mcp` debe listar los 22 archivos
  gated sin flags.
- [ ] T4 · S3 — `crates/webfang_core/Cargo.toml`: declarar con `required-features` los cuatro
  targets gated (`sqlite_integration`, `stress_test`, `tokio_console_smoke_test`,
  `adaptive_selector_integration`). Verificado por el guard.
- [ ] T5 · S3 — `scripts/check_empty_test_targets.sh` (nuevo, guard estático sin cargo) +
  `scripts/test_empty_test_targets.sh` (nuevo, harness de negación con fixtures `mktemp`) + un
  step en el job `repo-guards` de `.github/workflows/ci.yml` que corra el harness.
- [ ] T6 · S4 — `crates/webfang_core/tests/crash_matrix_test.rs`: fila `during_cancel_drain`
  (SIGINT entregado cuando wiremock vio la k-ésima petición, con el patrón de
  `tests/behavioral/cli/signal_lifecycle_test.rs`) y **borrado** del comentario "reserved for
  PR5b"; `docs/persistence-resume.md`: la tabla de la matriz queda alineada con la realidad.
- [ ] T7 · S5 — `crates/webfang_core/tests/sqlite_integration.rs`: test
  `test_sqlite_rows_pass_integrity_check_after_reopen` (escribir filas → reabrir el pool →
  `PRAGMA integrity_check` ⇒ `ok` → las filas sobreviven).
- [ ] T8 · S5 — decisiones registradas (en la issue, con evidencia) para las otras cuatro filas
  de la AC 1 y para el `cfg(loom)` de la AC 3; sin código.
- [ ] T9 · S6 — `crates/webfang_core/src/infrastructure/platform/windows_console.rs`: test
  *in-module* `#[cfg(windows)]` (`a_real_console_close_event_reaches_the_termination_source`) +
  el generador `#[ignore]` que el padre invoca con `--ignored --exact`; `docs/test-inventory.md`:
  la fila del `#[ignore]` nuevo en el catálogo congelado; `.github/workflows/ci.yml`: el
  comentario de la lane `test-crossplatform` dice que el test corre ahí y qué ocurre sin consola.
- [ ] T10 · S8 — `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs`: fixture 429 con
  `Retry-After` no nulo + test `issue_1816_waf_gauntlet_429_retry_after_is_observed_and_exits_69`;
  `crates/webfang_core/src/infrastructure/export/jsonl_writer.rs`: test de la barrera FIFO.
- [ ] T11 · S7 — sin código: #1833 se comenta con la evidencia (la decisión registrada del
  2026-09-30 sigue vigente y la lane `windows-latest` ya compila y ejecuta el brazo Windows) y
  queda a la espera de O4. Si el owner resuelve O4, el script del crate scratch se abre como
  slice posterior.
- [ ] T12 · S1–S8 — verificación del lote: `cargo check`, `cargo clippy --all-targets
  --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`,
  `cargo fmt --all -- --check`, `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace
  --all-features --no-deps`, `cargo nextest run` de los targets tocados, el harness nuevo en
  verde, `bash -n` de los scripts tocados, y el RED demostrado de T2 y T5.

---

## Log

L1 — Petición del owner (verbatim, del plan madre `odd/tasks/backlog-implementation-plan.md`):

> tengo muchas y  varias issues abiertas y necesito un plan para ir implementando utliza la skill
> adecuada para triage de issues

> y crea on plan odd para ir implementando todo

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

> te dejo la responsabilidad de decision a ti como experto, y si tienes que investigar en internet
> para buscar la mejor opcion o mejor alternativa tienes mi permiso total.

> pero tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro producto.

L2 — Correcciones del owner (verbatim):

> debes tener en cuenta que nuesto sistema ha cambiado sobre todo en lo relacionado con modelos de
> *AI LOCAL* Y LAS ISSUES pueden decir cosas o indormacion antigua o falsa,

> utiliza siempre tu y ordenar a  todos los workers las potentes tools *codedb* y *codegraph* en
> lugar de las otras tools

> ya tengo un agente trabajando en lote k1 en
> /home/xavi/Projects/Rust/webfang-worktrees/fix-ssrf-this-network-justification

Y la respuesta del owner a O1 (la decisión que este lote implementa en S2), verbatim:

> Sí: default=["mcp"] + default-features=false

L3 — Qué se leyó antes de escribir: el plan madre completo
(`odd/tasks/backlog-implementation-plan.md`, hasta su `## Log`), del que salen la fila L8, las
decisiones D8 (features) y D16 (WAL-kill) y la decisión O4 (#1833); y las seis issues del lote por
la API REST de GitHub, con el texto íntegro: #2066, #1852, #1828, #1833, #1834, #1816. La API
GraphQL no se usó porque devuelve el body truncado.

L4 — Herramientas: el índice de referencia es el checkout principal
(`/home/xavi/Projects/Rust/webfang`, `project`/`projectPath` absolutos), en `main@3a785d8e`; el
worktree de este lote está **exactamente** en `3a785d8e` (su `HEAD` coincide con el de `main`, y
`crates/webfang_core/src/cli/crash_points.rs` no tiene diff contra ese commit), así que el código
leído es el código del lote. Se usó `codedb` (`status`, `find`, `outline`, `callers`) y lectura
directa de los archivos citados. **No se corrió cargo** (el lote lo prohíbe) y **no se commiteó**
nada: sólo este documento y su espejo.

L5 — Evidencia verificada (símbolo + path; sin números de línea, que se invalidan con cada
edición):

- **#2066 — el defecto está VIVO, y la premisa de la issue sobre su origen es FALSA.** La issue
  dice *"Introducido por PR #2065 (rama `fix/2063-env-sensitive-assertions`, todavía sin
  mergear)"*. En `main@3a785d8e`, `crates/webfang_core/src/cli/crash_points.rs` (función
  `kill_self`, brazo `#[cfg(unix)]`) **ya** contiene el `loop { libc::pause() }` y el comentario
  que la issue cita. `codedb find kill_self` devuelve las dos definiciones (unix y `not(unix)`).
  O sea: no es un PR sin mergear, es el código de `main`; el cuelgue es real hoy.
- **#2066 — el diagnóstico por `tracing::error!` no se pierde**: en
  `crates/webfang_core/src/infrastructure/observability/logging.rs` (función
  `init_json_logging_dual`) la capa de texto escribe en `std::io::stderr` (bloqueante) y el
  `EnvFilter` de `--quiet` es `webfang=warn` (deja pasar `error!`); la única capa no-bloqueante es
  la de archivo opcional (`--trace-file`). Por eso un solo `tracing::error!` es el canal fiable y
  no hace falta un segundo canal (que sería una representación paralela).
- **#2066 — la rama del `kill` rehusado no tiene test**: no hay ningún test que la cubra (los tests
  del módulo son `parse_spec`, `is_armed_for`/`is_my_turn` y el pin de `getpid`).
- **#1828 — la fila `during_cancel_drain` está armada y ausente**: la constante
  `DURING_CANCEL_DRAIN` existe en `crates/webfang_core/src/cli/crash_points.rs`; su único disparo
  está en `crates/webfang_core/src/application/export_factory.rs` (función `process_results`,
  rama `SingleItemOutcome::Cancelled`, que se alcanza cuando `session.cancelled()`); la tabla de
  `docs/persistence-resume.md` anuncia la fila 9; `crates/webfang_core/tests/crash_matrix_test.rs`
  tiene diez filas y el comentario "reserved for PR5b".
- **#1828 — la desalineación del doc es de tres entradas**: la tabla de `docs/persistence-resume.md`
  enumera nueve puntos; la matriz implementa diez filas. Le sobra al doc `during_cancel_drain`
  (sin fila) y le faltan `mid_extraction` y `post_extraction_pre_pipeline` (con fila).
- **#1828 — la matriz no toca SQLite**: `crates/webfang_core/src/infrastructure/export/`
  (`record_store.rs`, `state_store.rs`, `jsonl_writer.rs`) es JSON; la única superficie SQLite es
  `crates/webfang_core/src/infrastructure/persistence/sqlite.rs` (pool `deadpool_sqlite`,
  `INIT_PRAGMAS` con `journal_mode = WAL`).
- **#1828 — `PRAGMA integrity_check` no existe en el código**: `grep` de `integrity_check` /
  `quick_check` / `wal_checkpoint` sobre `crates/` ⇒ **0** resultados; las únicas menciones están
  en `webfang-audit/AUDIT-CRASH-RECOVERY-20260925-035151/` (el plan de chaos que la issue cita).
- **#1828 — `cfg(loom)`**: `crates/webfang_core/build.rs` emite el cfg **sólo** para
  `webfang_core` (por diseño: los cfgs de build script son crate-scoped), así que el modelo de
  loom de `McpState::clone` (que pertenece a `webfang_mcp`) no es compilable hoy.
- **#1852 — la feature `mcp` no tiene `default`**: `crates/webfang_mcp/Cargo.toml` declara
  `[features] mcp = ["webfang_core/mcp"]`, `persistence` y `chromium`, y **no** declara `default`.
  Los 22 archivos gated son los de `crates/webfang_mcp/tests/` con `#![cfg(feature = "mcp")]`.
- **#1852 — el costo de `default = ["mcp"]` es cero**: `webfang_core`'s `mcp = []` es un marker
  vacío; no hay ni un `cfg(feature = "mcp")` en `crates/webfang_core/src/` (el único uso del
  crate es `cfg!(feature = "mcp")` en `crates/webfang_core/tests/common/cli_harness.rs`).
- **#1852 — el mismo defecto, en el otro crate, y sin que nadie lo haya reportado**:
  `crates/webfang_core/tests/` tiene cuatro archivos gated por feature
  (`sqlite_integration.rs` → `persistence`, `stress_test.rs` → `console`,
  `tokio_console_smoke_test.rs` → `console` + `tokio_unstable`, `adaptive_selector_integration.rs`
  → `adaptive-selectors`), y ninguno está declarado con `required-features` ⇒ los cuatro compilan
  a cero tests en el build por defecto, en silencio.
- **#1852 — `tokio_console_smoke_test.rs` está vacío en TODOS los builds**: su segundo término del
  gate (`tokio_unstable`) es un **cfg**, no una feature de cargo; el workspace declara `tokio =
  { version = "1", features = ["full"] }` y nunca habilita `tokio_unstable` ⇒ ese target no corre
  ni con `--all-features`. Es un segundo verde falso, de la misma clase, que la issue no menciona.
- **#1852 — el guard de AC 3 no existe**: `scripts/check_dead_tests.sh` (que se apoya en
  `cargo metadata`) detecta los **archivos** que cargo no descubre; no detecta los targets que
  cargo descubre y que compilan a cero tests.
- **#1834 — el test tiene que ser *in-module***: en
  `crates/webfang_core/src/infrastructure/platform/windows_console.rs`, `fn install` y
  `static EVENTS` son privados y `handler::ensure_source()` es `pub(super)` ⇒ un test de
  integración no puede instalar el handler ni observar el canal.
- **#1834 — las APIs Win32 necesarias ya están disponibles**: `windows-sys` se declara para
  `cfg(windows)` en `crates/webfang_core/Cargo.toml` con las features `Win32_Foundation`,
  `Win32_System_Console` y `Win32_Globalization` ⇒ `GenerateConsoleCtrlEvent` y `GetConsoleWindow`
  (ambos en `Win32::System::Console`) no requieren feature ni dependencia nueva.
- **#1834 — la lane existe y corre tests de lib**: el job `test-crossplatform` de
  `.github/workflows/ci.yml` corre `cargo nextest run --lib` y `cargo nextest run --test '*'` en
  `windows-latest` y `macos-latest`.
- **#1834 — la convención del inventario es de `#[ignore]`**: `docs/test-inventory.md` es el
  catálogo congelado de `#[ignore]` (Gate 0) y `scripts/check_ignored_guard.sh` falla cerrado ante
  cualquier deriva ⇒ un `#[ignore]` nuevo obliga a una fila en ese catálogo. El inventario legible
  por máquina (`docs/test-inventory.json`) lo genera `scripts/ci_test_inventory.py` y no lleva
  edición manual.
- **#1833 — la decisión que su propuesta revierte**: `odd/tasks/windows-console-lifecycle.md`
  ("Decisions taken by the maintainer (2026-09-30)") dice que el único compilador del código
  Windows es la lane `test-crossplatform` y que `rustup target add x86_64-pc-windows-msvc` fue
  **declined**; el truco del crate scratch quedó como procedimiento manual. O4 sigue **pendiente**
  en el plan madre.
- **#1816 — los mocks que fijan `Retry-After: 0` existen**:
  `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs` (los dos montajes de la
  secuencia WAF) y `crates/webfang_core/tests/common/fixture_server.rs` (el servidor de
  fixtures).
- **#1816 — el exit 69 del 429 está en el código**:
  `crates/webfang_core/src/domain/error/crawl_error.rs` (`CrawlError::RateLimited` →
  `ErrorClass::TransientBackoff`) y `crates/webfang_core/src/cli/error.rs`
  (`default_exit_code_for_class(TransientBackoff) = Some(EXIT_UNAVAILABLE)`, y `EXIT_UNAVAILABLE =
  69`). El pacing se duerme en
  `crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs` (rama unificada 429/5xx:
  `max(Retry-After, backoff)`), con `parse_retry_after_ms` capado por `backoff_max_ms`
  (10 000 ms por defecto) y `max_retries = 3`.
- **#1816 — la barrera del `flush` es FIFO y el writer es un solo hilo**:
  `crates/webfang_core/src/infrastructure/export/jsonl_writer.rs` — `append` y `flush` encolan
  mensajes en **un** canal y el hilo escritor los procesa en orden ⇒ la barrera cubre lo encolado
  **antes**; un append emitido durante un flush en vuelo queda detrás de la barrera (la AC 3, tal
  como está escrita, exige un contrato que el diseño no tiene).
- **#1816 — la mitad unitaria de la AC 2 ya existe**:
  `crates/webfang_core/src/application/crawler/crawl_task.rs` tiene el test
  `run_crawl_task_cancelled_during_rate_limit_wait` (cancelación durante una espera de
  rate-limit, a nivel de unidad).
- **#1816 — la mecánica determinista de `SIGINT` ya existe**:
  `crates/webfang_core/tests/behavioral/cli/signal_lifecycle_test.rs` entrega la señal cuando
  wiremock vio la k-ésima petición (no con un sleep) — el mismo patrón que necesita la fila
  `during_cancel_drain` de S4.
- **Convenciones verificadas**: `docs/regression-test-naming.md` + `scripts/check_regression_naming.sh`
  (advisory) ⇒ los tests de regresión llevan `issue_<N>_`; `scripts/check_regression_naming.sh` sólo
  marca nombres que contengan `test`/`issue_`/`pr_`/`crawler_`/`waf_`, así que el nombre que fija el
  plan (`crash_point_never_hangs_when_sigkill_is_discarded`) no produce aviso. `AGENTS.md` §Testing:
  `test_` en integración, sin prefijo in-module.

L6 — Premisas falsas de las issues (verificadas, no adoptadas):

| # | Lo que dice la issue | Lo que dice el código |
|---|---|---|
| #2066 | *"Introducido por PR #2065 (rama `fix/2063-env-sensitive-assertions`, todavía sin mergear)"* | El código ya está en `main@3a785d8e`: el `pause()` loop y el comentario citados están en `crates/webfang_core/src/cli/crash_points.rs`. El defecto es vivo. |
| #1852 | La mitad de `webfang_ai` (226 tests) como trabajo pendiente | El crate no existe (ADR-0004 paso 6). La propia issue ya eliminó ese criterio; no es trabajo de este lote. |
| #1816 | *"The only non-zero `Retry-After` assertions in the repo are in the MCP session-shed tests"* | Falso: `crates/webfang_core/tests/http_client_integration.rs` y `crates/webfang_core/tests/remote_embedding_resilience_test.rs` afirman `Retry-After` no nulo a nivel de downloader. El hueco real es el **nivel CLI** (que es lo que la AC 1 pide). |
| #1816 | *"Four bare `assert!(result.is_ok())`"* (4 sitios) | Dos de los cuatro no existen: uno estaba en `webfang_ai/src/infrastructure_ai/chunker.rs` (crate borrado) y el otro apuntaba a `logging.rs`, donde la línea ya derivó a un `assert!(...is_err())` real. Quedan dos: `crates/webfang_core/src/domain/repositories.rs` y `crates/webfang_core/src/application/llm_extraction.rs`. |
| #1833 | Propone un script local de type-check de `cfg(windows)` | Revierte la decisión registrada del 2026-09-30 (`odd/tasks/windows-console-lifecycle.md`). No se implementa sin O4. |
| #1834 | *"The test is documented in `docs/test-inventory.md` per the repo's test-inventory convention"* | La convención de ese archivo es el catálogo de `#[ignore]`; la AC sólo aplica si el generador es `#[ignore]` (que es lo que este lote propone). |
| #1828 | *"no `integrity_check` / `wal_checkpoint` / `quick_check` exists anywhere in the repo"* | Cierto en el código (`crates/` ⇒ 0 resultados); las menciones están en `webfang-audit/`, que es el plan de chaos, no una implementación. |

L7 — Decisiones tomadas en este lote, y su fundamento:

1. **La detección del descarte es `getpid() == 1`, y va antes del `kill`.** El descarte es una
   propiedad del PID del init del namespace, no del `errno`; detectarlo después (esperando la
   señal) es exactamente el cuelgue. Un solo camino de muerte diagnóstica para las dos ramas
   (`kill` rehusado e init de namespace): una representación, no dos.
2. **El test del hang se hace en un namespace de PID real**, no con un seam inyectable: un seam
   sería estado nuevo en producción para servir a un test. En Linux, si el namespace no se puede
   crear, el test **falla** con el comando de remediación — no se salta: un test que no prueba
   nada es el verde falso que este lote viene a cerrar.
3. **El guard de targets vacíos es estático (sin cargo)**: el guard tiene que correr en el job
   `repo-guards` de CI (y su sitio natural en el fast gate es junto a
   `scripts/check_dead_tests.sh`, que es el guard hermano y ya vive ahí); un guard que necesita
   compilar el workspace no es un guard de repo. La regla que implementa ("un target gated por una
   feature fuera del `default` tiene que estar declarado con `required-features`") es la que hace
   imposible la clase de defecto sin listas de allow ni números mágicos.
4. **Los cuatro targets gated de `webfang_core` se declaran con `required-features`**: el guard
   los exige, y `AGENTS.md` autoriza `[[test]]` para "special features". Efecto: en el build por
   defecto cargo **no construye** esos targets (antes los construía vacíos). Lo que corre no
   cambia: `crates/webfang_core/tests/sqlite_integration.rs` sólo corría con la feature
   `persistence`, y ahora corre con ella.
5. **La fila WAL-kill se rechaza** (D16) y el `PRAGMA integrity_check` va al único árbol SQLite
   del repo, sobre las filas que su test de integración ya escribe. La fila WAL-kill exigiría un
   punto de arming nuevo en producción — un verbo nuevo en el contrato del crash-harness — y eso
   no se agrega para servir a un test.
6. **El test de Windows es *in-module*** porque `install` y `EVENTS` son privados: fuera del módulo
   no hay forma de observar el fan-out. Y la aserción es "el camino de drenaje corrió", no "todo
   quedó persistido" — la issue pide decidirlo por adelantado y codificarlo en el nombre.
7. **El generador del evento de consola es `#[ignore]`** y el padre lo invoca con
   `--ignored --exact`: un generador que dispara un cierre de consola no puede correr en la suite
   normal. Eso obliga a la fila del catálogo de `docs/test-inventory.md` (que es lo que la AC 2
   pide).
8. **El pacing se aserta como cota inferior** (el run no puede terminar antes del `Retry-After`):
   una cota inferior no flakea.
9. **La AC 3 de #1816 se corrige antes de implementarse**: el diseño actual no garantiza que un
   append emitido durante un flush en vuelo sea durable al volver el flush; el test que se escribe
   fija el contrato que **sí** existe (durabilidad después del siguiente flush).

L8 — Criterios de #1816 que este lote **no** implementa, con su decisión (la AC los admite:
*"resolved or explicitly waived"*):

| Criterio | Decisión | Fundamento |
|---|---|---|
| *"A behavioral test sends SIGINT while the crawl is parked on the rate limiter"* | waiver | La mitad unitaria ya existe (`crawl_task.rs::run_crawl_task_cancelled_during_rate_limit_wait`). La variante E2E necesita caer en la ventana exacta de "parked on the rate limiter", que es un estado de wall-clock ⇒ depende de tiempo, contra la regla de determinismo del harness. |
| *"`application/pipeline/` has limit / cancellation / export-I-O coverage"* | waiver | Es una raíz distinta (cobertura de etapas del pipeline), no la raíz del título de la issue. El directorio ya está cubierto en lo esencial (`executor.rs`, `stages/validate.rs`, `stages/clean.rs`, `pipeline_cross_layer_consistency.rs`). |
| *"The four weak asserts, the store size-bound gap and the three N-1 names"* | waiver parcial | Dos de los cuatro asserts débiles no existen (ver L6). Los dos que quedan (`repositories.rs`, `llm_extraction.rs`), el hueco de size-bound de `state_store.rs`/`record_store.rs` y los tres nombres N-1 (`test_http_error` en `crates/webfang_core/src/error.rs`, `test_is_excluded`/`test_is_allowed` en `crates/webfang_core/src/application/url_filter.rs`) son la familia de calidad de aserciones de #1589, otra raíz. |

L9 — Colisiones de path que este lote **agrega** al mapa del plan (verificadas contra el mapa de
`odd/tasks/backlog-implementation-plan.md`):

| Path | Lotes | Orden |
|---|---|---|
| `crates/webfang_core/Cargo.toml` (bloque `[[test]]`) | L8 (este lote), L9 (`sys-info`→`sysinfo` toca la feature `persistence` de este mismo archivo), L10 | L8 → L9 → L10 |
| `crates/webfang_mcp/Cargo.toml` (bloque `[features]`) | L8 (este lote), L7 (`crates/webfang_mcp/**` según el mapa del plan) | L7 → L8 |
| `.github/workflows/ci.yml` (un step) | L8 (este lote), L3, L4, L13 | el step de cada lote se re-aplica sobre el `main` nuevo |

L10 — Lo que este lote deja abierto, a propósito:

- `crates/webfang_core/tests/tokio_console_smoke_test.rs` sigue vacío en todos los builds
  (`tokio_unstable` no se habilita en el workspace). Hacerlo correr exige tocar la feature
  `tokio_unstable` de `tokio` (dependencias) o borrar el archivo: **decisión del owner**, fuera de
  este lote.
- #1833 espera O4 (decisión del owner). El lote deja la evidencia y la issue comentada, no cerrada.
- Los waivers de L8 (AC 2, 4 y 5 de #1816) quedan como decisiones registradas: se cierran en la
  issue con evidencia, no con promesas.

L11 — Estado de la entrega: este documento y su espejo
(`odd/false-green-tests/tasks.md`) son **documentación**; no hay código de producción, no se corrió
cargo y no hay commit (el commit es del padre). **No hay PR** (decisión del owner: las PRs se abren
más adelante).

L12 — **El RED del test del cuelgue fue INVALIDO dos veces, y por eso casi se
registra como evidencia.** La primera corrida fallo con "--url is required for
scraping": mi comando no pasaba `--url`, asi que el hijo salia por config y
nunca llegaba al punto de crash. La segunda fallo con el guard SSRF
rechazando el 127.0.0.1 del wiremock: el env hermetico del harness no lo
heredaba mi comando crudo. **Un RED que falla por otra causa no es un RED**:
en las dos anteriores el test fallaba, pero no por el defecto. La correccion
fue expositar `hermetic_cache_dir` y `hermetic_empty_config_file` del harness
como `pub(crate)` (dos palabras) y aplicar al comando la misma hermeticidad
que el resto de los binarios lanzados —en el ORDEN correcto: primero se
limpian las variables envenenadas y DESPUES se fijan las del test, porque
una limpieza posterior se llevaria el punto de crash consigo—.

L13 — **RED valido, por fin**: contra el codigo pre-fix el hijo se cuelga y
el test falla con el sintoma real — "the crash child did not finish within
30s: the SIGKILL was discarded and the child hung instead of dying" (la
corrida tardo 30,3 s, o sea la ventana). GREEN: 27/27 en la matriz de crash
(el nuevo test en 2,91 s: el hijo ahora muere en vez de colgarse). El fix
detecta `getpid() == 1` ANTES de enviar la senal (enviarla y esperar ES el
cuelgue) y comparte UN UNICO camino de muerte diagnostica con la rama del
kill rehusado, para que un solo test cubra las dos ramas.

L14 — **Guard de targets vacios: el RED medido y un fallo encontrado en mi
propio guard.** El RED del guard: 24 targets declarados que compilan a CERO
tests en el build por defecto (20 de `webfang_mcp`, gated en una feature
fuera del default, y 4 de `webfang_core`) -> 20 tras declarar los 4 de core
con `required-features` -> 0 con `default = ["mcp"]` en `webfang_mcp`. Y
encontré un fallo en el guard que escribi: leia el gate con `head -40`, asi
que en dos archivos el gate caia fuera de la ventana y el guard los contaba
COMO SI NO TUVIERAN GATE — un falso PASS, el peor modo de fallo de un guard.
Escaneando el archivo entero son 22 (el numero real), no 20. El harness
cubre los cuatro casos del spec, mas el de vacuidad (un arbol sin ningun
target tiene que fallar, no pasar) y el de la forma real de la issue.

L15 — **Verificacion de T7 y T10, corrida por el padre**: el store sobrevive
a una reopen y `PRAGMA integrity_check` responde `ok` (1 passed); el fixture
429 con `Retry-After: 1` hace observable el pacing (el run tarda 19 s, o sea
que espero) y el presupuesto agotado sale con 69 (1 passed); y el append
emitido con un flush en vuelo es durable despues del siguiente flush (1
passed).

L16 — **Corte de slice de este lote**: el branch suma ~1618 lineas autorales
en 6 commits (spec del peer · T3+T4 · T5 · T1+T2 · T7 · T10). El work-unit
de T1+T2 (el fix del cuelgue y su test) va solo, porque el fix cambia el
comportamiento observable de un camino de muerte y su test necesita el
namespace de PID para discriminar.

L17 — **T6 se cierra con una DESVIACION, y la desviacion es el hallazgo.** La
spec pedia una fila para `during_cancel_drain`. La fila NO SE PUEDE ESCRIBIR:
la rama que la alcanzaria es **inalcanzable en produccion**. La sesion de
export solo observa un token de cancelacion cuando se llama
`ResumeContext::observing_cancel`, y el UNICO llamador en todo el crate esta
en el modulo de tests de `export_factory` (lineas 1409 y 1472): en produccion
el `cancelled()` de la sesion siempre es false, la rama `Cancelled` del drain
nunca corre, y el punto `DURING_CANCEL_DRAIN` esta ARMADO pero no puede
dispararse. Medido, no supuesto: el hijo del crash matrix con el token ya
cancelado sale con codigo 0 sin llegar al drain, y el log del crawl muestra
"2 total, 2 succeeded" — la cancelacion llego, el drain no lo vio.

Lo que se hizo en su lugar: la tabla de `docs/persistence-resume.md` lista
ahora las DIEZ filas que existen (antes enumeraba nueve: ocho con fila y una
sin, y le faltaban dos que si la tienen), el punto sin fila queda nombrado CON
SU RAZON, el comentario de la suite que lo reservaba para "PR5b" dice lo
mismo, y el doc de la constante deja de decir "Reserve for PR5b". Anunciar una
fila que no puede pasar seria un verde falso de la misma clase que esta suite
existe para impedir. Lo que FALTA —el cableado que haria el punto alcanzable
(que el flujo de export de la CLI entregue su token al `ResumeContext`)— es un
cambio de PRODUCCION y ademas cambia el comportamiento del export ante
cancelacion (hoy el export sigue hasta el final, que es justo lo que E8
"drain-before-final-persist" queria evitar). Se le paso al owner del plan
para que lo decida; no se hizo unilateralmente.

L18 — **T9 (el evento real de consola de Windows) queda escrito, con su nivel
de verificacion declarado.** El test instala el handler de PRODUCCION, pide a
Windows un `CTRL_CLOSE_EVENT` real y afirma que la fuente de terminacion lo
recibe; el handler devuelve "consumido", asi que el OS no termina el proceso
de test. Va `#[ignore]`d porque necesita una consola REAL, y la lane de
Windows lo ejecuta EXPLICITAMENTE (un step con `--run-ignored
ignored-only`): ahi existe una consola, y si el proceso del runner no la
tuviera el test fallaria —un hecho de la plataforma, reportado como tal, no
un skip silencioso. Lo que NO se verifica localmente: el binario para Windows
no compila en esta maquina (el target necesita el toolchain MSVC de C++). Lo
que SI se verifico, contra las fuentes: la firma real de
`GenerateConsoleCtrlEvent` ((u32, u32) -> BOOL en windows-sys 0.60), la
feature `Win32_System_Console` habilitada en el manifiesto, y tokio con
`features = ["full"]` (el `timeout` del await). El catalogo congelado de
`#[ignore]` sube de 12 a 14 (el atributo nuevo y la mencion del doc comment)
y el guard de inventario queda en verde: 14 menciones (9 atributos + 5
doc/comment) en 6 grupos.

L19 — **T12: la verificacion del lote, con sus dos correcciones.** El clippy
FALLO la primera vez: `clippy::undocumented_unsafe-blocks` (que el CI corre
con -D warnings) exige que todo bloque `unsafe` diga por que es seguro, y
el `getpid()` que detecta el init de un namespace no lo decia. Corregido.
Y el test del pacing era INESTABLE — loRz la suite completa, donde solo
aparece: media el reloj desde el INICIO del run (la latencia del spawn
entraba en la cuenta: aislado, un spawn lento lo hacia pasar por el motivo
equivocado; en la suite, con el binario caliente, el run terminaba en
464ms y fallaba) y pasaba `--backoff-max-ms 50`, que CAPA el `Retry-After`
de 1000ms por debajo del limite que la asercion exigia (la asercion
estaba midiendo el recorte, no el pacing). Ahora la ventana se mide desde
el instante en que el servidor respondio el 429 (el mock lo registra) y el
cap es 2000ms. Verificacion: la suite behavioral completa, DOS corridas
seguidas, 168/168 en verde y sin flakes.

Verificacion final del lote: `cargo check --all-targets` exit 0 (los tres
warnings de `webfang_mcp` en la build sin features son preexistentes de
`main` y no aparecen con `--all-features`, que es lo que corre el CI);
`cargo clippy --all-targets --all-features` con `-D warnings`,
`clippy::cognitive_complexity` y `clippy::too_many_lines` exit 0;
`cargo fmt --all -- --check` exit 0; `RUSTDOCFLAGS=-D warnings cargo doc
--workspace --all-features --no-deps` exit 0; los tres guards en verde
(el de targets vacios con su harness, el de inventario y el de tests
muertos); la matriz de crash 27/27, el store SQLite con `persistence` en
verde, y la suite behavioral 168/168.

L20 — **Cierre del lote**: L8 queda cerrado con 13 commits. Los criterios que
NO se cierran aqui: T8 y T11 son del lado issues (del owner del plan), y
T6 se cierra con la desviacion de L17. El issue #1852 no se cierra con este
lote: la parte que le toca al otro agente (el lado issues) sigue abierta.

L21 — **La fila del catalogo nombra QUIEN verifica, y no insinua un gate.** El
peer cerrio el punto de T9 con una medicion que ya esta en `ci.yml`: la lane
`test-crossplatform` corre en `[windows-latest, macos-latest]` y ejecuta
`cargo build -p webfang_cli --bin webfang` mas `cargo nextest run --lib` y
`cargo nextest run --test '*'` ⇒ un test IN-MODULE (windows_console.rs) SI
corre en la lane de Windows, y el runner trae cmake y NASM preinstalados
(lo dice el propio comentario de la lane).

Y por que NO hago un check cruzado local: `cargo check --target
x86_64-pc-windows-msvc` en un host Linux compila BoringSSL desde C++ y
necesita el toolchain MSVC, que aqui no existe ⇒ el check local falla en el
build script, y **un check local que falla no es evidencia**. Es el mismo
principio que rechazo el scratch-crate: la lane real de Windows es la unica
que compila Windows.

El matiz que importa para la evidencia: esa lane hoy es **advisory** —no
esta en el `needs` del gate, y su propio comentario dice que los primeros
runs se esperan fallando en Windows—. Por eso la fila del catalogo
congelado tiene que decir QUE la verifica ('lane de Windows') y no insinuar
un gate: las dos filas del catalogo que nombraban 'the parent' ahora nombran
la lane. El promote a required queda como follow-up aparte, como ya
anticipa la lane.

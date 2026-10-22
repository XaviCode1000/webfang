# El export entrega su token de cancelación (decisión A de #2244)

El export de la CLI entrega al `ResumeContext` el token de cancelación de la
sesión, para que un export cancelado **drene** en vez de seguir hasta el
final: lo ya completado se persiste, lo cancelado no, y el punto de crash
`during_cancel_drain` queda alcanzable.

## Specs

S1. La sesión de la CLI entrega su token al export. El `ShutdownGuard` de
    `cli/shutdown.rs` es el dueño del token (`ShutdownGuard::install()` lo
    crea; `wait_for_signal` lo cancela ante la señal de terminación). El
    export lo recibe como un **clone** y lo propaga hasta el `ResumeContext`,
    que se construye con `.observing_cancel(token)`. El builder
    `ResumeContext::observing_cancel` ya existe en
    `application/export_factory.rs`: lo que falta es el cableado.

S2. Un export cancelado **no persiste estado parcial del item cancelado**, y
    **sí** persiste lo que ya se había completado antes de la cancelación.
    La rama `SingleItemOutcome::Cancelled` del drain rompe el loop y el
    `final_persist()` posterior escribe el estado ya completeness: el
    contrato "drain-before-final-persist" se respeta. Lo que NO puede pasar
    es que el item en vuelo se persista a medias.

S3. La rama no cancelada **no cambia**: sin cancelación, el export persiste
    exactamente lo que persistía antes, con los mismos números y los mismos
    snapshots.

S4. Con el cableado, el punto `during_cancel_drain` es alcanzable y **la fila
    de la matriz de crash se escribe**: `crates/webfang_core/tests/crash_matrix_test.rs`
    recibe la fila `during_cancel_drain`, y el comentario que la reservaba
    para "PR5b" desaparece. La fila usa la misma máquina de la matriz
    (crash → resume → las cuatro invariantes globales) y necesita **una**
    pieza: entregar `SIGINT` en un momento determinista. El patrón ya existe
    y es reutilizable tal cual:
    `crates/webfang_core/tests/behavioral/cli/signal_lifecycle_test.rs`
    entrega la señal cuando wiremock ha visto la k-ésima petición (no con un
    sleep), que es la única forma de que el test sea determinista.

S5. El texto que hoy dice que la rama es inalcanzable se corrige en los
    **cuatro** lugares que lo declaran: el doc-comment de la constante
    (`cli/crash_points.rs`), el comentario de la suite de crash, la tabla de
    `docs/persistence-resume.md` y el doc del lote de L8
    (`odd/tasks/false-green-tests.md`, sección S4 y `## Log`). Con el
    cableado, la frase correcta es "alcanzable" y la fila **existe**.

## Tasks

- [x] T1 · S1 — `crates/webfang_core/src/cli/export_flow.rs`: el `ExportConfig`
      gana un campo `cancel: Option<&'a CancellationToken>` (o el tipo que
      corresponda a la vida útil), y los **dos** sitios que construyen el
      `ResumeContext` (`run_standard_export` y `run_ai_export`) le enganchan
      `.observing_cancel(token)`. OJO CON LA VIDA ÚTIL: el `ResumeContext` se
      construye **dentro** de `spawn_blocking`, así que el token tiene que
      entrar al closure como **clone propio** (owned), no como borrow del
      stack del async.
- [x] T2 · S1 — `crates/webfang_core/src/cli/orchestrator.rs`: `export_phase`
      recibe el token y lo pasa al `ExportConfig`; el `ShutdownGuard` instalado
      en `run()` (`orchestrator.rs`, `ShutdownGuard::install()`) es la fuente.
      OJO: `export_phase` se llama desde **tres** sitios (dos en
      `orchestrator.rs` —el normal y uno de test— y uno en `batch_flow.rs`);
      todos los call sites se actualizan, y el de `batch_flow.rs` recibe el
      token que ya maneja (`batch_flow.rs` ya crea un `CancellationToken`).
- [x] T3 · S1, S2, S3 — `crates/webfang_core/tests/`: el test que prueba que
      un export cancelado **no** persiste estado parcial del item cancelado y
      que el camino no cancelado no cambia. El test cita el punto de crash en
      `cli/crash_points.rs` (como manda el inventario de L8).
- [x] T4 · S4 — `crates/webfang_core/tests/crash_matrix_test.rs`: la fila
      `during_cancel_drain` (SIGINT entregado cuando wiremock vio la k-ésima
      petición, con el patrón de
      `tests/behavioral/cli/signal_lifecycle_test.rs`) + el **borrado** del
      comentario "reserved for PR5b" ya está; la tabla de
      `docs/persistence-resume.md` **ya listaba la fila 9** (HECHO 1, no se
      duplicó — la spec presumía diez filas previas, hay nueve).
- [x] T5 · S5 — los textos de "reserved/PR5b" de S5, corregidos a
      "alcanzable": `cli/crash_points.rs` y el comentario de la suite de crash
      (HECHO 2); la fila de `docs/persistence-resume.md` ya era verdadera;
      `odd/tasks/false-green-tests.md` **no existe en este árbol** (reportado).
- [ ] T6 · S1–S5 — verificación: los tests afectados verdes; `cargo check`;
      `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt
      --all -- --check`; `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace
      --all-features --no-deps`; y la suite de la matriz de crash completa
      (las once filas).

## Log

L1 — Decisión del owner (2026-10-22), verbatim: "Implementar la decisión A de
`#2244` en el flujo de exportación, sin esperar aprobaciones adicionales." Y:
"el export entrega su token de cancelación". Registrada en el plan, commit
69546042; `status:approved`, comentario 6333165316.

L2 — Lo visible, verbatim del plan: antes Ctrl-C no detenía el export; ahora
sí.

L3 — Mapa MEDIDO (por el padre, no de memoria), que es lo que guía la
implementación:

- `cli/shutdown.rs` es el dueño del token: `ShutdownGuard::install()` lo crea,
  `token()` lo clona, `wait_for_signal` lo cancela ante la señal de
  terminación. El guard se instala en `cli/orchestrator.rs`, en `run()`.
- `cli/export_flow.rs`: `ExportConfig` (struct en `export_flow.rs`) **no**
  tiene campo de token. `run_export(config, ai_cleaner)` lo consume.
- El `ResumeContext` se construye en **dos** sitios de `export_flow.rs`
  (`run_standard_export` y `run_ai_export`), los dos como
  `ResumeContext::new(store).with_resume(resume)` — **sin**
  `.observing_cancel(...)`.
- `application/export_factory.rs`: el builder `observing_cancel(&'a
  CancellationToken)` ya existe (campo `cancel: Option<&'a
  CancellationToken>`). Los **únicos** llamadores actuales de
  `observing_cancel` están en el mismo fichero, y ambos **después** de
  `mod tests` ⇒ son calls de test ⇒ el punto es inalcanzable en producción
  (la premisa de L8, verificada).
- La rama `SingleItemOutcome::Cancelled` del drain está en `export_factory.rs`
  y ahí se dispara `crash_points::hit(DURING_CANCEL_DRAIN)` ⇒ el punto está
  ARMADO y esperando el token.
- `export_phase` se llama desde **tres** sitios: dos en `orchestrator.rs` (el
  normal en `run()` y uno de test) y uno en `batch_flow.rs`.

L4 — El drain, medido: la rama `Cancelled` hace `break` del loop de items y
**después** se llama `session.final_persist()` ⇒ el item cancelado NO se
persiste y lo ya completado SÍ. Ese es el contrato drain-before-final-persist
que E8 pedía; el slice lo hace observable, no lo reescribe.

L5 — Decisión del padre: la implementación **aterriza después del merge de
`#2230`**, porque ambos tocan el flujo de exportación. `#2237` y `#2238` son
independientes. Esta spec se escribe ahora (prep no bloqueado); la
implementación espera.

L6 — Decisión del padre: con el cableado, la fila de `during_cancel_drain` se
escribe en el MISMO slice. La fila no es un adorno: es la prueba de que el
punto es alcanzable, y sin ella el doc volvería a afirmar algo no verificado
(la clase de falsehood que este trabajo viene a eliminar). El rótulo de L8
("aún no llega") se corrige a "alcanzable" en el mismo slice, en los cuatro
lugares que lo declaran.

L7 — Riesgo asumido y por qué: el cambio es de comportamiento observable del
export ante cancelación (hoy el export sigue hasta el final). Está autorizado
por el owner (decisión A, `status:approved`). Por eso el slice tiene que
traer el test que prueba el comportamiento nuevo **y** el que prueba que el
camino no cancelado no cambió.

L8 — El wiring de producción de `during_cancel_drain` (el `hit()` en la rama
`Cancelled`) **no se toca**: ya existe y ya está en el lugar correcto. Lo que
faltaba era que la rama fuera alcanzable. La fila de la matriz lo prueba
matando al hijo en ese punto.

L9 — Implementación de T1–T3 (worktree `fix-export-cancel-token`, sobre la
spec commiteada en 89b6b145). El token viaja como **
`Option<CancellationToken>` owned dentro de `ExportConfig`** — no como
`Option<&'a CancellationToken>` — porque el `ResumeContext` se construye
DENTRO de `spawn_blocking` (#1814 slice B): el closure necesita `'static`, un
borrow del stack async no puede cruzar esa frontera, y `CancellationToken`
es clonable por diseño. En los dos constructores (`run_standard_export`,
`run_ai_export`) el `config.cancel` entra al closure como propio y
`.observing_cancel(token)` toma `&token` / `cancel.as_ref()` sobre ese clone
local. `export_phase` recibe `&CancellationToken` y hace
`Some(cancel.clone())` en el `ExportConfig`.

L10 — Call sites de `export_phase`: son **cuatro**, no tres (la spec contó
"dos en orchestrator, uno de test", pero en `orchestrator.rs` hay **dos**
tests que la llaman: `export_phase_rejects_stdout_as_output_dir` y
`export_phase_reports_inbox_creation_failure`): el de `run()` pasa
`&shutdown.token()`, los dos tests un `CancellationToken::new()` y el de
`batch_flow.rs` el `cancel` que ya maneja.

L11 — RED/GREEN medido de la fila `during_cancel_drain`
(`crash_during_cancel_drain_sigint_drain_kills_then_recovers`): el test se
escribió ANTES del wiring y FALLÓ con "armed crash point must terminate the
child by signal, got clean exit" (el hijo salió limpio: la cancelación nunca
llegaba al drenaje). Con T1+T2 cableados, la misma fila pasa y muere por
SIGKILL(9). El SIGINT se entrega determinísticamente cuando wiremock vio la
1ª petición de página (patrón `signal_lifecycle_test`, con delay de 200 ms en
las respuestas para que la ventana exista) — nunca con un sleep.

L12 — Consecuencia prevista del contrato nuevo (S2/L2): los tres `p85_*` de
`signal_lifecycle_test.rs` codificaban el contrato VIEJO ("todas las páginas
fetcheadas quedan COMMITTED tras la señal", porque el export ignoraba el
token y corría hasta el final) y quedaron ROJOS con el drenaje. Se
actualizaron al contrato nuevo: el record store sólo contiene ítems
COMMITTED (nunca parciales), el drenaje persiste ≥1 ítem completado, el
`--resume` no re-fetchea lo drenado y lo demás se re-dibuja a lo sumo una vez
por corrida (`count ≤ 2`). También quedó el invariante explícito "the drain
may only persist COMMITTED items, never partial ones" (S2 hecho test). Los
tres pasan: 5/5 en `signal_lifecycle`, 167/167 en `behavioral` completo.

L13 — Textos de "reserved/PR5b" corregidos (HECHO 2): (1) `cli/crash_points.rs`
— "Reserve for PR5b: during cancellation drain before final persist." →
"During cancellation drain, before final persist (#2244 decision A): hit when
a cancelled export drains after observing the run's token, before
`final_persist()` writes the drained state."; (2) `crash_matrix_test.rs` —
"Matrix rows — one per pinned crash point. during_cancel_drain is reserved
for PR5b and intentionally absent here." → "Matrix rows — one per pinned
crash point.". La fila de `docs/persistence-resume.md` (fila 9) **ya existía**
y NO se duplicó (HECHO 1). El cuarto texto de S5
(`odd/tasks/false-green-tests.md`) **no existe en este árbol** — no hay nada
que corregir ahí; reportado al padre.

L14 — Corrección del PADRE sobre la aserción de `p85_signal_case` (el cambio
más sensible del lote, revisado a mano antes de commitear). El worker dejó el
comentario afirmando "fetched at most once per run — never twice in the same
run" pero la aserción era `count <= 2` sobre el total cross-run: un doble
fetch DENTRO de la corrida señalada pasaba verde, y `committed.len() <=
fetched_paths.len()` aceptaba un récord de una URL nunca fetcheada (commit
fantasma). Endurecido a lo que el texto ya prometía, con los contadores que la
prueba ya tenía: (a) pertenencia — todo record drenado tiene que ser una URL
fetcheada en esa corrida; (b) por corrida — `fetched_before` se muestrea justo
después de que la corrida señalada sale, así que `run1 <= 1` y `run2 =
total − run1 <= 1` prueban "a lo sumo una vez por corrida" de verdad. Además
el doc-comment ya no sobrevende: declara que esta prueba fija la SEGURIDAD del
drenaje y que la ALCANZABILIDAD del punto la fija la fila de la matriz (sin el
cableado esa fila falla con "got clean exit"). Medido después del cambio:
`signal_lifecycle` 5/5, `crash_matrix_test` 27/27, ambos con exit real 0.

L15 — Slice y forma de entrega. El hook del commit midió 680 líneas cambiadas
en 2 commits desde `origin/main`: producción ~70, fila de matriz ~255, spec
~70, fixture del golden set 66, más el ajuste de `p85`. NO se parte en dos
PRs: la fila de la matriz ES la prueba RED del cableado (sin el token, el
punto no se alcanza y el hijo sale limpio), y este repo no admite PRs
apiladas (`scripts/check-topology.sh`) ⇒ un PR secuencial con el cableado
solo llegaría sin RED. El límite de slice queda registrado acá: 680 líneas
autoradas, de las cuales el código de producción es ~70.

L16 — La rama estaba ATRÁS de `main` al entregar (`89ff9d46`; `origin/main`
ya en `276d0b05`, con #2246 y #2248 dentro). Traído con `merge` (nunca rebase:
`89b6b145` ya está pusheado), sin conflictos. Consecuencia verificada, no
supuesta: el golden set regenerado sobre la base vieja SIGUE coincidiendo con
el árbol mergeado (`the_real_tree_parses_without_errors_and_matches_the_golden_set`
ok, exit 0), porque las dos PRs mergeadas tocan scripts y no `crates/webfang_core/src`.

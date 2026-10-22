# L15 — Lane de UB: que el detector de Miri vuelva a correr (#2235)

Plan padre: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes; vive en el branch
del Lote 2, `fix/merge-when-green-merged-state`, y viaja con él a `main`).
Este doc es la especificación de **un** lote: la lane de UB tiene que **detectar y atribuir** el
UB, no abortar el proceso y dejar un dead end. Sin PR: el trabajo queda commiteado en el
worktree.

Rama: `fix/miri-ub-lane` · Worktree: `~/Projects/Rust/webfang-worktrees/fix-miri-ub-lane`
Issue: #2235 (aprobado: `status:approved`, `type:chore`).

---

## Specs

**S1.** El issue (#2235), verbatim:

> La lane `Miri (core)` está **roja en las últimas cuatro corridas schedule** con **undefined
> behavior vivo** en `application::scraper_service.rs`, y el `--skip` net que la acompaña sigue
> puesto en `ci.yml`. El detector de UB no está corriendo: la lane aborta el proceso y no deja
> resultado reportable.

> - Verificado con `gh run view <id> --json jobs`: `Miri (core) = failure` en las corridas
>   `39192353219` (2026-10-19), `38201931380` (2026-10-12), `37260512917` (2026-10-05) y
>   `36372960382` (2026-09-28). Las otras tres lanes Miri (domain, infra-slow, infra-fast) están
>   en `success`. El run completo figura `success` porque la lane es `continue-on-error` ⇒ **el
>   fallo no bloquea nada y nadie lo ve**.
> - El UB es real: `Undefined Behavior: write access through <...> at alloc...[0x0] is forbidden`,
>   familia `servo_arc`/`lol_html`. El gate de #2187 (`scraper_service.rs`) cubre **un** test del
>   archivo; un test hermano del mismo archivo sigue disparando el UB y aborta el proceso antes
>   de dejar el resultado de la lane.
> - El skip-net sigue en tres sitios de `.github/workflows/ci.yml` (`--skip wreq_downloader
>   --skip obscura_downloader --skip chromiumoxide_downloader`). Es el residual de #1423, cuyo AC
>   pedía "4/4 verdes **sin** skip-net": el dispatch manual ocurrió (run `37038367607`, citado en
>   `odd/tasks/miri-copy-file-range-trap.md`) pero esa cláusula nunca se cumplió.
> - Un lane de UB que aborta el proceso es un **dead end**: no deja ver qué test falló, así que
>   nadie puede actuar sobre él.

Criterios de aceptación (verbatim):

> - `Miri (core)` en verde en una corrida schedule, o el UB atribuido a un test concreto con el
>   gate que le corresponde.
> - El UB de `scraper_service.rs` tiene un gate **por test** (como el de #2187) y la lane deja un
>   resultado reportable en vez de abortar el proceso entero.
> - Cada `--skip` que sobreviva tiene un motivo escrito y un caso de test asociado; el skip de
>   crates (no de tests) se elimina o se justifica por nombre.

**S2.** El plan padre define el lote (verbatim de su tabla):

> **L15** | Lane de UB | #2235 | el UB de `scraper_service.rs` con gate **por test**; cada
> `--skip` con motivo escrito; la lane deja resultado, no aborta el proceso | job `Miri (core)` en
> verde en una corrida schedule | +20/−10 | worker | PENDIENTE

**S3.** La forma del gate es la que el repo **ya** usa (y la que nombra el AC de #2187):
`#[cfg_attr(miri, ignore = "<motivo escrito>")]`, atributo sobre **el test concreto**, con un
motivo que nombre el UB o la limitación concreta — no "flaky" ni "no soportado". Hay ~15 gates
de esa forma en el repo (p. ej. `container.rs`, con el motivo `btls-sys FFI (wreq Client) not
supported by Miri`). El UB de `scraper_service.rs` se gatea **por test**: el test hermano que
lo dispara queda ignorado bajo Miri con su motivo, y el resto del archivo sigue corriendo.

**S4.** **El skip-net se elimina, y su red de seguridad se traslada al código donde vive el
motivo.** Los tres `--skip` (`wreq_downloader`, `obscura_downloader`, `chromiumoxide_downloader`)
son skips de **submódulo**, no de test: cada uno borra de la lane un árbol entero de tests. El
AC pide que el skip de crates se elimine o se justifique por nombre. Decisión: **se eliminan**,
y lo que los protegía se vuelve **gate por test** — los tests que de verdad necesitan FFI
(`btls-sys`/wreq, `onig-sys`/syntect, `chromiumoxide`) llevan su propio
`#[cfg_attr(miri, ignore = "<motivo>")]`, con el motivo escrito al lado. Motivo de fondo: una
red de seguridad que borra un árbol entero es una red que además **esconde** el UB de ese
árbol; el skip de #1423 era un parche, y el AC de #1423 ("4/4 verdes **sin** skip-net") nunca se
cumplió. La red de seguridad que queda es **más fuerte**, porque cada skip tiene un motivo
escrito y un test asociado, y los tests no-skipeados corren.

**S5.** La lane debe dejar resultado, no abortar. Con el gate por test, el UB conocido queda
atribuido y la lane termina. Para que un UB **futuro** no se coma el resultado de toda la lane,
el harness estático (S7) fija que el filtro de `Miri (core)` cubre el módulo del UB
(`application::`) y que todo gate tiene motivo: la detección sigue siendo la de Miri (no se
desactiva la aliasing) — el repo ya lo dice en el bloque de diseño del job: "If a specific test
fails under Tree Borrows, isolate it with `#[cfg(not(miri))]` — never disable aliasing detection
globally".

**S6.** El `MIRIFLAGS` de la lane es parte del contrato del resultado:
`-Zmiri-tree-borrows -Zmiri-disable-isolation -Zmiri-ignore-leaks -Zmiri-seed=42
-Zmiri-permissive-provenance`. **Toda corrida local usa los mismos flags y el mismo toolchain
pinneado** (`RUSTUP_TOOLCHAIN=nightly-2026-08-27`, que es el pin de la lane): un resultado local
obtenido con otros flags no es el resultado de la lane.

**S7.** El harness de este lote es **estático y barato** (shell, sin Miri, sin cargo): un guard
que fija las tres propiedades que la lane debe tener, para que ninguna se rompa en silencio:
1. Todo `--skip` que aparezca en un job Miri tiene, en el bloque de comentario de ese job, un
   motivo escrito **y** el nombre del test o módulo asociado. (Con S4, la lista de skips queda
   vacía: el guard falla si vuelve a aparecer un skip sin justificar.)
2. Todo `#[cfg_attr(miri, ignore = ...)]` del workspace lleva un motivo **no vacío** (un gate
   sin motivo es un gate que nadie puede evaluar).
3. El filtro de `Miri (core)` cubre el módulo del UB atribuido (`application::`).
Su RED pre-fix: el skip-net de tres entradas sin motivo por entrada, y el test hermano del UB
sin gate. Su GREEN: los skips eliminados, los gates con motivo, y el guard en verde.

**S8.** Build: la verificación de Miri necesita compilar bajo Miri (el sysroot + el crate), que
es caro. El trabajo **no compila nada hasta que el padre le dé la ventana de build** (en esta
máquina hay otro agente compilando, y la regla es un build a la vez). El harness estático de S7
no necesita build: se puede escribir y correr en cualquier momento.

**S9.** Paths de este lote: `.github/workflows/ci.yml` (los tres jobs Miri: filtro, skips y el
bloque de comentario de diseño), `crates/webfang_core/src/application/scraper_service.rs` (el
gate por test) y los submódulos downloader que hoy están skipeados (sus gates por test), más
el harness nuevo de S7. **No** toca `docs/`, ni `CHANGELOG.md`, ni los jobs Miri de otros
workflows. El owner autorizó Miri/nightly (O8) y este step de `ci.yml` es mío: ningún otro
lote en curso agrega un step en `ci.yml` (el owner del plan lo confirmó).

---

## Tasks

- [x] T1 · S1, S3 — identificar el test hermano de `scraper_service.rs` que dispara el UB
  (familia `servo_arc`/`lol_html`) y gatearlo **por test** con
  `#[cfg_attr(miri, ignore = "<motivo escrito>")]`, siguiendo el patrón de #2187.
- [x] T2 · S1, S4 — determinar qué tests de los submódulos `wreq_downloader`,
  `obscura_downloader` y `chromiumoxide_downloader` necesitan de verdad FFI/asm, gatearlos
  **por test** con su motivo, y **eliminar** los tres `--skip` de los jobs Miri.
- [x] T3 · S5 — actualizar el bloque de comentario de diseño de los jobs Miri: el mecanismo
  nuevo (gate por test con motivo; sin skip-net) y por qué el UB de `scraper_service.rs` queda
  atribuido a un test.
- [x] T4 · S7 — harness estático (shell, sin build) que fija las tres propiedades de S7, con su
  caso RED contra el estado pre-fix.
- [x] T5 · S6, S8 — verificación: harness estático en verde; **scoped Miri** con el toolchain y
  los flags de la lane: el módulo del UB completa (con el gate) y los submódulos antes
  skipeados corren sin UB bajo Miri (o con su gate explícito). La corrida de la lane completa en
  verde **no** se puede producir localmente: es un run schedule, y lo da el CI cuando el owner
  abra la PR.
- [x] T6 · S1-S9 — commit de work-unit en `fix/miri-ub-lane` (sin PR: la abre el owner cuando el
  plan entero esté implementado).

Ruta: **worker** (`gentle-ai-worker`) para T1-T4; el padre da la ventana de build para T5 y
commitea. Motivo: el lote encadena lectura de la lane y de los módulos, una corrida Miri para
identificar el UB, los gates, el harness y su verificación — varios pasos encadenados.

---

## Log

L1 — Petición original del owner (verbatim, del lote padre):

> coordena con el otro agente para implementar el plan ODD del backlog completo →
> odd/tasks/backlog-implementation-plan.md (+ espejo odd/backlog-implementation-plan/tasks.md),
> commiteado en el branch del Lote 2 para que viaje con él y no quede huérfano en main (justo
> la regla que el propio lote establish).

> recuerda que debeis tu y el otro agente tener cuidado siempre en no ejecutar comandos
> pesados , para no saturar mi sistema, y el trabajo se guarda en worktree sin abrir pr's
> hasta que no este todo el plan implementado.

L2 — Reparto: el owner del plan (sesión `01a1607b`) me confirma que **L15 es mío** y que **el
step de `ci.yml` de la lane Miri me corresponde** (L3 ya está commiteado: `1b49ef71`,
`eaa172d7`, así que el path está libre). Miri/nightly quedó autorizado por el owner (O8).

L3 — **Miri ya está en la máquina**: `rustup toolchain list` muestra `nightly-2026-08-27` (el
pin de la lane) con `cargo-miri` (`miri 0.1.0`) y `rust-src` instalado; el `nightly` genérico
también. No hay nada que descargar. El `rustup toolchain install nightly` del otro agente
falló por timeout de red, pero el toolchain ya estaba.

L4 — Estado real de la lane, leído en `ci.yml`: el job `miri:` (matriz `core` y `domain`, con
filtro por módulo), `RUSTUP_TOOLCHAIN: nightly-2026-08-27`, `continue-on-error: true`,
`timeout-minutes: 240`, `MIRIFLAGS: -Zmiri-tree-borrows -Zmiri-disable-isolation
-Zmiri-ignore-leaks -Zmiri-seed=42 -Zmiri-permissive-provenance`, y el paso de corrida
`cargo miri setup` + `cargo miri test --lib -- --skip wreq_downloader --skip
obscura_downloader --skip chromiumoxide_downloader ${{ matrix.filter }}`. Los tres skips
aparecen **tres veces** (jobs `miri` 1588, `miri-infra-fast` 1647, `miri-infra-slow` 1714), y el
bloque de comentario de diseño (1480-1500) ya describe las tres capas de defensa —con el
skip-net como "CI safety net"—, así que el comentario se actualiza con el mecanismo nuevo.

L5 — La evidencia de que el skip-net es un parche y no una política: los tests que necesitan
FFI **ya están gateados por test** en el repo (`#[cfg_attr(miri, ignore = "btls-sys FFI (wreq
Client) not supported by Miri")]` y familia, ~15 en `container.rs`). El skip de submódulo es
redundante con esos gates y además borra tests que sí podrían correr. Por eso S4: eliminar los
skips, no justificarlos.

L6 — El skip de #1423 queda documentado como **residual de un AC que nunca se cumplió**: el
dispatch manual ocurrió (run `37038367607`) pero la cláusula "4/4 verdes sin skip-net" no se
cumplió. Este lote la cumple.

L7 — **El arbitraje del gate defensivo se corrigió ANTES de que costara una corrida.** El
worker proponía "la corrida scoped de Miri es el árbitro" con el gate **puesto** en el brazo de
error. Un test con `#[cfg_attr(miri, ignore = "…")]` queda `ignored` bajo Miri: la corrida iba
a reportar "2 passed, 1 ignored" **sin ejecutar el brazo de error**, o sea, el arbitraje era
lógicamente vacío — la evidencia de un falso verde. La secuencia correcta (sacar el gate, correr,
y recién ahí decidir) se la pedí explícitamente.

L8 — **El arbitraje, por evidencia: la premisa de la issue queda refutada.** Con el gate
fuera, la corrida scoped de `application::scraper_service::` dio **2 passed / 0 failed / 1
ignored** (el de éxito, gateado por #2187). El brazo de error **pasa limpio**: el 404 retorna
antes de `clean_html` y el único test que llega a `clean_html → lol_html::rewrite_str` es el de
éxito, ya gateado. Consecuencia: **no hay gate nuevo** — `scraper_service.rs` queda intacto
y la frase de #2187 ("The error arm below returns before `clean_html` and needs no gate")
sigue siendo cierta. El AC de la issue ("el hermano sigue disparando el UB") se cumplió
**refutándolo con la corrida**, que es más honesto que dejarlo escrito.

L9 — **Las tres correcciones de robustez al harness** que pidió el padre: (a) la extracción de
nombres de `--skip` **ignora las líneas de comentario** (hoy no muerde porque los comentarios
usan la forma con backticks, pero un autor futuro que escribiera "--skip foo" en un
comentario disparaba un fallo confuso); (b) la invariante 4 se reformuló: ya no exige gate a
todo test que maneja el caso de uso, sino al que **alcanza la superficie del UB**, con una
**exención nominal** para el brazo de error que lleva motivo **y evidencia** (la corrida de
2026-10-22: 2 passed, 0 failed, 1 ignored) — una entrada vieja o un test no exento sin gate
hacen fallar el harness; (c) **guard de vacuidad**: si el marcador `scrape_with_config(`
desaparece por un renombre del use case, la invariante **falla** en vez de quedarse verde sin
mirar nada.

L10 — **Verificación revisada por el padre, no aceptada por informe.** El harness da exit 0
sobre el árbol real (con control RED por check, más el control RED de vacuidad y el control
GREEN del filtro del core). Su RED es real: reconstruido contra el árbol pre-fix falla con 12
skips sin motivo (3 submódulos × 3 jobs, más `wikilinks`) y 4 gates sin razón en
`resource_governor.rs`. El único `--skip` **real** que queda en todo `ci.yml` es el del job de
cobertura (`cargo llvm-cov … --skip test_mcp`), que no es la lane Miri.

L11 — **Las corridas scoped, con el toolchain y los flags de la lane**
(`nightly-2026-08-27`, `-Zmiri-tree-borrows -Zmiri-disable-isolation -Zmiri-ignore-leaks
-Zmiri-seed=42 -Zmiri-permissive-provenance`): `application::scraper_service::` → 2 passed /
1 ignored; los **tres submódulos antes skipeados** → **21 passed / 0 failed / 29 ignored** (los
que el skip borraba ahora corren: los 5 del parser de cookies, los 2 del fallo ruidoso en
build, los 4 del constructor de obscura y los de drain/stub de chromiumoxide);
`infrastructure::converter::` → 41 passed / 0 failed (wikilinks pasa bajo Miri, así que quitar
su `--skip` no cambia la selección de tests y no hace falta un gate nuevo). `cargo check
--all-targets` exit 0 (9m49s) y `cargo fmt --all -- --check` exit 0.

L12 — **Riesgos que quedan anotados**: `cargo miri test --lib` unifica features, así que el
feature `chromium` está EN la lane → el E2E de chromiumoxide (que lanza Chrome) necesita su
gate, y lo tiene. Y la lane infra-slow ahora interpreta los tres submódulos de downloader (~6
min bajo Miri, dentro de su presupuesto de 120 min). La corrida **completa** de la lane no se
puede producir localmente (es un run schedule): la da el CI cuando el owner abra la PR.

L13 — **Corte de slice de este lote**: el branch suma ~1045 líneas autorales repartidas en
cinco commits (spec · skip-net y gates por test · guard · harness con su wiring ·
evidencia). El harness del worker mediría 566 líneas en un solo archivo, por encima del
límite de ~500 del repo, así que se partió en el par idiomático del repo: el guard
(`scripts/check_miri_lane_invariants.sh`, 454 líneas, con las cuatro invariantes) y su
harness hermano (`scripts/test_miri_lane_invariants.sh`, 163 líneas, con el control RED).
El guard corre solo (exit 0/1) y el harness lo carga como librería (`MIRI_LANE_LIBRARY=1`)
para correrlo sobre sus fixtures: un guard que puede correr sobre fixtures es la mitad de
un guard reusable. Si el owner prefiere una PR más chica, el corte natural es (1) el
skip-net y los gates, (2) el guard, (3) el harness con su wiring.

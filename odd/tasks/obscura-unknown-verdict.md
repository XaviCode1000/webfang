# L11 — Obscura: el veredicto `Unknown` nombra el mínimo esperado (#1710)

Plan padre: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes; vive en el branch
del Lote 2, `fix/merge-when-green-merged-state`, y viaja con él a `main`).
Este doc es la especificación de **un** lote: el veredicto `Unknown` del preflight de Obscura
deja de ser un veredicto mudo y **nombra el mínimo esperado**. Sin PR: el trabajo queda
commiteado en el worktree.

Rama: `fix/obscura-unknown-verdict` ·
Worktree: `~/Projects/Rust/webfang-worktrees/fix-obscura-unknown-verdict`
Issue: #1710 (aprobado: `status:approved`, `type:chore`).

---

## Specs

**S1.** El plan padre define el lote en una línea (verbatim de su tabla):

> **L11** | obscura | #1710 | el veredicto `Unknown` **nombra el mínimo esperado**; el hit-rate se
> deriva de los logs por capa; sin modo estricto, sin contadores |
> `preflight.rs::unknown_verdict_names_the_expected_minimum` | +6/−1 | inline | PENDIENTE

**S2.** Lo que el veredicto `Unknown` es hoy (CodeGraph sobre `main@3a785d8e`):
`crates/webfang_core/src/cli/preflight.rs` degrade el chequeo de Obscura a "unknown version"
**con un warning** cuando el probe de `--version` falla o expira (`VERSION_PROBE_TIMEOUT`, 5 s).
Un veredicto que no dice qué esperaba es un veredicto que el operador no puede usar: el
warning tiene que **nombrar el mínimo esperado** (`MINIMUM_OBSCURA_VERSION_STR`), que es la
constante que el propio guard usa para rechazar una versión vieja.

**S3.** La forma del veredicto no cambia: sigue siendo `Unknown` (no pasa a `Failed`), sigue
siendo un **degradado**, no un error — el flujo de release y el crawl no se frenan. Lo que
cambia es el **contenido del warning**: pasa de "no se pudo determinar la versión" a "no se
pudo determinar la versión; se esperaba al menos 0.2.2". El mensaje al usuario final va en
**español** (regla del repo); los campos de tracing, en inglés.

**S4.** El test nombrado por el plan es `preflight.rs::unknown_verdict_names_the_expected_minimum`:
un test **in-module** que falla si el warning deja de nombrar el mínimo esperado. Test
**RED** contra el código pre-fix (el warning no lo nombra), GREEN después.

**S5.** Sin modo estricto y sin contadores (decisión del plan padre, D-list): el hit-rate de
L2 se **deriva de los logs por capa**, no se agrega instrumentación nueva. Un modo estricto
opt-in sería un verbo nuevo en el contrato del preflight, y los contadores por capa
(L1-hit / L2-hit / L2-vacío→L3 / L3-hit) serían estado nuevo en el resumen del crawl. Este
lote no los agrega: su alcance es el warning.

**S6.** Fuera de alcance de este lote, por decisión explícita:
- **El pin mínimo 0.2.2 → 0.2.3** (AC1 del issue): subir el requisito del binario es un
  cambio de requisito de producto, no un fix de defecto, y arrastra los fakes de los tests
  que reportan `obscura 0.2.0`. No entra en un lote de +6/−1.
- **Los docs** (`docs/src/cli-reference.md`, la nota de binario): `docs/` está fuera de
  alcance salvo pedido explícito, y la decisión de la variante de build (`-stealth` vs L2
  como render barato) no está tomada.
- **Los contadores por capa** y el **modo estricto**: por S5.

**S7.** El issue #1710 **no se cierra**: según D17 del plan padre, #1826 y #1710 se
**comentan, no se cierran** (un hilo con dos modos de falla). El comentario explica qué se
resolvió y qué queda con el owner.

**S8.** Build acotado (regla 3 del plan padre + el pedido del owner de no saturar la
máquina): el seed de target para este worktree da **cold** (`seed: cold reason=no-seed`), así
que el primer build compila. Por eso: `$(rustup which cargo)` (nunca el shim de mbx),
`CARGO_BUILD_JOBS=1`, `CARGO_INCREMENTAL=0`, y el build dentro de
`systemd-run --user --scope -p MemoryMax=2G` para que no pueda tumbar la máquina. **Un solo
build a la vez en toda la máquina**: si otro agente está compilando, esperar.

---

## Tasks

- [ ] T1 · S2, S3 — el warning del veredicto `Unknown` nombra el mínimo esperado
  (`MINIMUM_OBSCURA_VERSION_STR`), en español, sin cambiar el veredicto ni su degradado.
- [ ] T2 · S4 — test in-module `unknown_verdict_names_the_expected_minimum`: RED pre-fix,
  GREEN post-fix.
- [ ] T3 · S8 — verificación: `cargo nextest run -p webfang_core preflight` (o el filtro
  equivalente) en verde, con el build acotado de S8; `cargo fmt --all -- --check` y
  `cargo clippy -p webfang_core --all-targets -- -D warnings` (el clippy **con** features
  por defecto del paquete: no usar `--fix` con un conjunto de features más estrecho).
- [ ] T4 · S1, S6, S7 — registro: actualizar este doc con la evidencia RED/GREEN y el
  comentario de cierre parcial en el issue #1710 (lo hace el padre: el worker no escribe en
  GitHub).

Ruta: **worker** (`gentle-ai-worker`), con allowed edit surfaces estrechas: sólo
`crates/webfang_core/src/cli/preflight.rs` (el test vive in-module, en el mismo archivo).
Razón: el lote encadena lectura del veredicto, caso RED, fix, GREEN y un build acotado —
varios pasos encadenados, y el backstop de contexto pide delegar la unidad siguiente.

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

L2 — Reparto con el otro agente (sesión `01a1607b`): el otro agente toma **L3** (CI Health,
W-03). L11 vive en `crates/webfang_core/src/cli/preflight.rs` — path que L3 no toca.

L3 — Mapa del símbolo (CodeGraph + CodeDB sobre `main@3a785d8e`, worktree propio indexado):
`MINIMUM_OBSCURA_VERSION` y `MINIMUM_OBSCURA_VERSION_STR` en
`crates/webfang_core/src/cli/preflight.rs` (1 caller cada una);
`check_obscura_version` → `assess_obscura_version` → `VersionVerdict`; `probe_obscura_version`
con `VERSION_PROBE_TIMEOUT`; test existente `hybrid_version_below_minimum_errors` (el camino
de "versión por debajo del mínimo" ya está cubierto — el que falta es el de "no se pudo
saber la versión").

L4 — El seed de target para este worktree da cold (`seed: cold reason=no-seed
key=v1-5256d5a74e9f8731`): el primer build compila desde cero. De ahí el protocolo de S8 y
la regla de **un solo build a la vez** en la máquina.

L5 — **La premisa del issue sobre el pin mínimo está obsoleta** (verificado en el código):
`MINIMUM_OBSCURA_VERSION = (0, 2, 5)` y `MINIMUM_OBSCURA_VERSION_STR = "0.2.5"`. El criterio
de aceptación #1 de #1710 pedía subir el piso a `0.2.3` — el piso **ya está por encima**
(0.2.5), así que no hay bump que hacer, y el "revisar los fakes de tests que reportan
`obscura 0.2.0`" tampoco aplica: el piso vigente es 0.2.5. Esto refuerza S6: este lote no
toca el pin. (El owner advirtió que las issues pueden llevar información antigua: acá la
tenían.)

L6 — **El warning ya degradado no es un error** (el worker lo señaló y es correcto): el camino
`Unknown` es una degradación, no un fallo, así que conserva su `warn!`. Pasarlo a
`log_scrape_error` subiría el nivel a ERROR y cambiaría la semántica observable del preflight
— fuera del alcance de este lote.

L7 — El texto en español del warning se **deriva** de `MINIMUM_OBSCURA_VERSION_STR`, para que
no pueda divergir del piso que el guard impone: si el piso sube, el mensaje sube con él.

L8 — El worker entregó el lote (route: worker con superficie acotada a `preflight.rs`; el
commit es del padre). Lo entregado: el texto en español del warning vive en una función
nombrada (`unknown_obscura_version_warning`) que **deriva** de `MINIMUM_OBSCURA_VERSION_STR`,
así que el mensaje no puede derivar del piso que el guard aplica; el evento gana dos campos
(`expected_version` en inglés, `user_message` con la frase en español) y **el veredicto no
cambia**: sigue siendo `Unknown`, sigue degradando a `Ok(())`. El test in-module afirma las
DOS mitades, que es lo que impide el "verde por半个": que el veredicto siga siendo
degradación Y que el log nombre el mínimo.

L9 — **Evidencia revisada por el padre, no aceptada por informe.** RED: el warning capturado
por el harness (`LogBuffer`) no contenía ninguna versión — el síntoma del defecto, no un
error de compilación. GREEN: `cargo nextest run -p webfang_core preflight` → **79/79**
(la corrió el padre, en el target caliente del worktree, con el mismo protocolo: `jobs=1`,
`MemoryMax=3G`): exit 0. Los hermanos `Unknown` siguen verdes en la misma corrida
(`hybrid_version_garbage_degrades_to_warning`, `hybrid_version_probe_failure_degrades_to_warning`),
y el piso no se tocó: `MINIMUM_OBSCURA_VERSION_STR` sigue siendo "0.2.5" (S6). `cargo fmt
--all -- --check` en verde y `cargo clippy -p webfang_core --all-targets -- -D warnings` en
verde.

L10 — Lo que el worker señaló y el padre confirma: el `warn!` preexistente de este camino se
**queda** como degradación. Pasarlo por `log_scrape_error` lo subiría a ERROR y cambiaría la
semántica observable del preflight (un binario con versión ilegible dejaría de ser una
degradación) — eso es otro lote, no este.

L11 — **Desviación asumida y registrada**: el diff es +72/−0 contra un estimado de "+6/−1".
El estimado contaba solo la línea del warning; S4 exige un test in-module (~50 líneas) y el
texto en español lleva su doc (~16). No hay comportamiento nuevo más allá del contenido del
warning.

L12 — Commits del lote: `0e52f031` (spec + espejo, con este registro) y `6b82e087` (el fix y
su test). El commit es del padre: el worker no commitea. Estado: **cerrado** — el veredicto
`Unknown` nombra el mínimo esperado, la degradación se conserva, y la suite de preflight queda
en 79/79. Lo que NO se cierra con este lote: el pin 0.2.3 del issue (obsoleto: el piso
vigente es 0.2.5), los docs y los contadores por capa (decididos: el hit-rate se deriva de los
logs por capa, sin instrumentación nueva). El issue #1710 **no** se cierra: su parte del
veredicto `Unknown` recién aterriza, y el resto de sus ACs siguen vivos.

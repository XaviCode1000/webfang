# L4 — Residuales de guard post-#2068 (#2070, ítems 1-3)

Plan padre: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes; vive en el
branch del Lote 2, `fix/merge-when-green-merged-state`, y viaja con él a `main`).
Este doc es la especificación de **un** lote: tres guards con un hueco real cada uno, su
harness de negación y su commit. Sin PR: el trabajo queda commiteado en el worktree.

Rama: `fix/guard-residuals` · Worktree: `~/Projects/Rust/webfang-worktrees/fix-guard-residuals`
Issues: #2070 (ítems 1, 2 y 3). El ítem 4 se **documenta como "no hacer nada"** (D13).

---

## Specs

**S1.** El owner delega la decisión y pide que no sea la fácil, y que el trabajo no se
entregue en PRs hasta que el plan entero esté implementado:

> te dejo la responsabilidad de decision a ti como experto, y si tienes que investigar en
> internet para buscar la mejor opcion o mejor alternativa tienes mi permiso total.

> pero tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro
> producto.

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

> y crea on plan odd para ir implementando todo

**S2.** Los tres residuales son defectos de guard, no deuda de estilo, y cada uno trae su
harness. Del issue #2070 (verbatim):

> 1. El guard de monotonicidad semver acepta un backward walk con un componente con zero a la izquierda — `scripts/bump-support-patch.sh`, bloque `B2 (#2060): semver monotonicity`
>
> 2. El meta-guard acepta un harness "cableado" que sólo aparece en un comentario — `scripts/test_guard_coverage.sh`, función `harness_wired`
>
> 3. El `.envrc` del checkout principal se parsea con una regla distinta al de los worktrees — `scripts/check_target_isolation.sh:318`

Y sus criterios de aceptación (verbatim):

> - [ ] El guard de monotonicidad no acepta un componente con zero a la izquierda: la comparación es numérica en base 10 (prefijo `10#`) o el input se rechaza antes, con un caso de harness que fije el caso `2.09.0` contra `2.12.0` (el harness debe quedar **rojo** si se borra la protección — prueba de mutación).
> - [ ] `harness_wired` ignora las líneas de comentario, como ya hacen `guard_refs_in` y `script_refs_in`; el harness de semántica del meta-guard (`scripts/tests/test_guard_coverage.sh`) gana un caso con referencia sólo en comentario que debe fallar.
> - [ ] `MAIN_TARGET` se parsea con el mismo parser liberal que el resto del registro, con un caso de harness que cubra la forma indentada en el `.envrc` de main.

**S3.** El harness es la prueba, y el RED tiene que ser preciso (D3 del plan padre): rojo
por la razón del defecto, no por un refusal de otra etapa. Un caso que no puede
distinguir un script roto de uno arreglado no es evidencia.

**S4.** Nada de builds pesados en esta máquina (regla 3 del plan padre): este lote es shell
puro, ningún harness invoca `cargo` de verdad, y el gate de Rust no se corre localmente
porque el diff no toca Rust.

**S5.** Un solo escritor por path (mapa de colisiones del plan padre). L4 no toca
`.github/workflows/ci.yml`: los tres harnesses **ya existen y ya están cableados** en
`ci.yml` (steps "Support patch bump semantics harness", "Guard coverage meta-guard semantics
harness" y "Target isolation guard semantics harness"), así que ningún lote en curso puede
chocar con L3 en ese archivo.

---

## Tasks

- [x] T1 · S2-S3 — ítem 1: la comparación de monotonicidad se lee en base 10 (`10#`) en los
  tres componentes; harness: caso nuevo en `scripts/test_bump_support_patch.sh` (RED: el
  error octal de bash y el bump hacia atrás commiteado) → commit `fix(scripts)`.
- [x] T2 · S2-S3 — ítem 2: `harness_wired` ignora las líneas de comentario completo, igual
  que `guard_refs_in`/`script_refs_in`; harness: caso 13 en `scripts/tests/test_guard_coverage.sh`
  (RED: el meta-guard salía con exit 0) → commit `fix(scripts)`.
- [x] T3 · S2-S3 — ítem 3: `MAIN_TARGET` se parsea con `parse_envrc_target_decl` (el parser
  liberal), no con el `sed` rígido; harness: caso 19 en
  `scripts/tests/test_check_target_isolation.sh` (RED: `reason=main-unbootstrapped` sobre una
  declaración indentada que sí declara) → commit `fix(scripts)`.
- [x] T4 · S1-S5 — doc del lote + espejo `odd/guard-residuals/tasks.md` + registro de la
  decisión de ítem 4 ("no hacer nada") → commit `docs(odd)`.

Ruta: **inline**. El Writer rule no dispara: cada ítem es un one-liner más un caso de
harness (~40 líneas autorales), muy por debajo del umbral de un subagent, y los tres ítems
comparten la misma técnica de RED/GREEN. Delegar tres unidades de este tamaño costaría más
contexto del que ahorra.

---

## Log

L1 — Petición original del owner (verbatim):

> coordena con el otro agente para implementar el plan ODD del backlog completo →
> odd/tasks/backlog-implementation-plan.md (+ espejo odd/backlog-implementation-plan/tasks.md),
> commiteado en el branch del Lote 2 para que viaje con él y no quede huérfano en main (justo
> la regla que el propio lote establish).

> recuerda que debeis tu y el otro agente tener cuidado siempre en no ejecutar comandos
> pesados , para no saturar mi sistema, y el trabajo se guarda en worktree sin abrir pr's
> hasta que no este todo el plan implementado.

> comunica con el y coordina para que no hagais el mismo trabajo dos veces o saturar el
> sistema

L2 — El plan y su espejo ya estaban commiteados en el branch del Lote 2 (`66f23d63`,
`odd/tasks/backlog-implementation-plan.md` + `odd/backlog-implementation-plan/tasks.md`).
Verificado: `main` **no** los contiene, así que viajan con L2 y no quedan huérfanos en
`main`. Este lote no los duplica.

L3 — Reparto con el otro agente (sesión `01a1607b`, worktree
`fix-ci-health-w03-classifier`): el otro agente toma **L3** (CI Health / W-03, #2233 #2234);
este lote toma **L4** y después **L5**. Paths sin solape: L3 toca
`scripts/test_ci_health_close.sh` + `.github/workflows/ci-health.yml` + un step en `ci.yml`;
L4 toca `scripts/bump-support-patch.sh`, `scripts/test_guard_coverage.sh`,
`scripts/check_target_isolation.sh` y sus tres harnesses.

L4 — Corrección al plan: la premisa del plan ("`scripts/test_bump_support_patch.sh` (nuevo)")
era falsa — el harness **ya existe** (735 líneas) y ya está cableado en `ci.yml`. El plan
también decía que L4 tocaría `ci.yml`; tampoco hace falta: los tres harnesses ya tienen su
step. Por eso L4 no colisiona con L3 y este lote se puede cerrar sin esperar al otro agente.

L5 — Decisión del ítem 1: la comparación se hace **numérica en base 10** con el prefijo
`10#`, y **no** se agrega una regla de rechazo al contrato de versiones. Fundamento:
`scripts/refresh_support_latest.sh` ya usa `10#` para su contador de parches, con el
comentario exacto de que un componente con zero a la izquierda nunca debe leerse como
octal; el repo ya tiene la convención y el guard de `bump-support-patch.sh` era el
inconsistente. Además, la forma numérica deja el guard **fail-closed por construcción**: una
comparación que no puede errar no puede ser derrotada por un literal que bash no parsea.
Rechazar el input, en cambio, agregaría un verbo nuevo ("versión no canónica") al contrato
de un script de release, con un radio de impacto mayor y sin cerrar el hueco del lado de
`CURRENT` (que también puede traer un zero a la izquierda).

L6 — El caso del harness **no** es el del issue verbatim (`2.09.0` contra `2.12.0`) porque
esa forma no alcanza el guard: `2.09.0` pertenece a otra minor (`2.09`), y el guard de rama
la rechaza antes — el mismo error de forma que ya documenta el caso de `2.12.2 → 2.12.1`.
El caso equivalente dentro de la misma minor es `2.12.10 → 2.12.09`: numéricamente hacia
atrás, con zero a la izquierda, y alcanza la comparación. El RED lo confirma: pre-fix,
`((: 09: valor demasiado grande para la base` y el bump **commiteado**.

L7 — Ítem 4 de #2070: **no hacer nada** (D13 del plan padre). La ventana de publicación
pendiente es angosta, numérica y se auto-cura con el tag; un expiry agrega estado y una
falla nueva por un riesgo que hoy no existe. Se documenta acá, no se implementa.

L8 — Evidencia de los tres ítems (RED → GREEN, todo shell, sin `cargo`):

| Ítem | RED (pre-fix) | GREEN (post-fix) |
|---|---|---|
| 1 | `scripts/test_bump_support_patch.sh`: 3 fails — "aceptó un patch DOWNWARD con zero a la izquierda", con `línea 158: ((: 09: valor demasiado grande para la base` y el commit `chore: bump to v2.12.09` | 58 ok, 0 fail |
| 2 | `scripts/tests/test_guard_coverage.sh`: "a harness named only in a comment is not wired: expected non-zero exit, got 0" (el meta-guard contaba el comentario como wiring) | 13 casos ok |
| 3 | `scripts/tests/test_check_target_isolation.sh`: "case 19: main's indented declaration is parsed: want exit=0 … got exit=2" con `reason=main-unbootstrapped` | PASS (19 casos) |

L9 — Chequeo diferencial contra el árbol real (no sólo contra los fixtures):
`scripts/test_guard_coverage.sh --no-mutation` sobre el árbol real → "18 wired guard(s)
covered" (la regla nueva no descablea ningún guard real); `scripts/test_target_isolation_envrc.sh`
→ PASS; `scripts/check_target_isolation.sh` con el target del worktree →
`reason=own-registered-target`, exit 0. En el `.envrc` real de `main` (declaración en la
columna 0) el parser liberal devuelve el mismo valor que el `sed` rígido, así que el
veredicto no cambia para la forma común: el cambio **sólo** des-rechaza la forma indentada.

L10 — Lo que NO se corrió localmente, y por qué: el gate de Rust (`cargo check`, `clippy`,
`fmt`, `doc`) y la corrida de mutación del meta-guard (que ejecuta el harness de cada uno
de los 18 guards cableados, varios de ellos con `cargo` debajo). Motivo: S4 y la regla 3 del
plan padre — un build de BoringSSL compite con la máquina y con el otro agente. El diff de
este lote es 100% shell (`scripts/`), así que el gate de Rust no tiene nada que verificar
de este diff; la mutación del meta-guard la corre el job `repo-guards` de CI.

L11 — Cuando el owner abra la PR de este lote (tarea T11 del plan padre), el cuerpo lleva
`Closes part of #2070`: este lote cierra los ítems 1-3 del issue, y el ítem 4 queda
documentado como "no hacer nada" — el issue **no** se cierra entero. Rama:
`fix/guard-residuals`, base `main`. Una etiqueta `type:*` (la del vocabulario del repo:
`type:bug` para los tres guards) y cero atribución de IA.

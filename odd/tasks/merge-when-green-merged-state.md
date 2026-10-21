# Merge y limpieza post-merge: PR ya mergeado y comandos que no se encadenan

Objetivo: cerrar tres fallas de proceso que hacen perder tiempo y trabajo terminado,
sin agregar una sola máquina nueva. Un script que confunde "ya mergeado" con "GitHub
todavía computa", un runbook cuya cadena entera de limpieza nunca corre por un `rm`
encadenado, y un registro de misión que queda huérfano en `main` porque se escribió en
el checkout principal en vez del worktree de la misión.
Rama: `fix/merge-when-green-merged-state`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-merge-when-green-merged-state`.

Cierra: #1970, #1971, #1972, #2105. Sin dependencias: no toca `crates/`, ni el exportador
(#2230), ni el guard de SSRF (K1, worktree del owner).

---

## Specs

**S1.** El script distingue un PR **ya mergeado** de uno que GitHub todavía está
computando, y sale limpio en el primer caso:

> `merge-when-green.sh` detecta `state == MERGED` (o `mergedAt` no vacío) ANTES de entrar
> en el loop de reintentos de `UNKNOWN` y sale con éxito + mensaje "already merged".

> El script ya no reintenta 90s cuando el PR ya está mergeado.

Fallo de origen (verbatim de #1970):

> Cuando el PR ya está merged (state=MERGED), el endpoint reporta `mergeable: UNKNOWN` y
> `mergeStateStatus: UNKNOWN`, y el script trata ese `UNKNOWN` como "GitHub todavía está
> computando mergeability" → reintenta 90s y sale con error, en lugar de detectar
> `state == MERGED` y salir limpio.

Restricción de diseño (del triage, no del issue): **sin código de salida nuevo y sin flag
nuevo**. El estado deseado ("el PR está mergeado") ya está alcanzado ⇒ salida 0. Un
código de salida nuevo obligaría a quien lo invoca a aprender otra tabla.

**S2.** La limpieza post-merge **completa** corre siempre, y el borrado del target dir es
un comando standalone con path literal (verbatim de #1971):

> O se documenta explícitamente en AGENTS.md que `rm -rf` del target dir debe ejecutarse en
> un paso separado ANTES de la cadena de limpieza, no como parte de la misma.

> En ambos casos: `git branch -D` y `git worktree prune` deben ejecutarse aunque el `rm -rf`
> sea bloqueado.

La primera rama de la AC ("se refina el hard-deny") vive **fuera de este repo** (el guard
está en el harness del agente, no es configurable): no es repo-actionable. La rama
documental es la que se implementa, y se corrige el orden de la AC: si el `rm` va *antes*
de la cadena y el hard-deny lo bloquea, la cadena tampoco corre — el orden que sí cumple
es **cadena de git primero, `rm` standalone después**.

Fallo de origen (verbatim de #1971):

> El `rm -rf` matchea el patrón hard-deny `rm -rf ~` y bloquea la cadena entera antes de
> ejecutar nada — los pasos `git branch -D` y `git worktree prune` (legítimos y seguros)
> nunca se ejecutan.

**S3.** El registro de misión viaja con la rama (verbatim de #1972):

> Regla de proceso (en AGENTS.md o runbook) que ate el registro de misión al worktree de la
> misión: el archivo `odd/tasks/<feature>.md` debe crearse/editarse DENTRO del worktree de
> la misión, no en el checkout principal.

Se implementa la rama de **regla de proceso**. La rama del chequeo automatizado queda
descartada a propósito: un gate nuevo que nadie pidió es un estado nuevo que se pudre, y
el repo ya tiene el chequeo que hace falta (`git status --short` vacío en el handoff
post-merge); lo que falta es la regla que ata el registro al worktree.

Fallo de origen (verbatim de #1972):

> | Esta misión (B2 slice 2) | Registro redactado en el checkout principal en vez del
> worktree de la misión | 1 registro huérfano en `main` (untracked) |

> Mecanismos distintos, **misma consecuencia**: trabajo terminado que no llega al árbol.

**S4.** El ADR 0004 **no se toca**: su lista de residuo es un **registro histórico
congelado** y la corrección que la actualiza ya está escrita (de #2105).

> `docs/adr/0004-onnx-local-removal.md` — la línea que dice "#2105 (abierta)".

Corrección del padre (revisión de la implementación): esa línea **no está stale**. El
ADR dice, textualmente, que la lista "queda como el registro del 2026-10-20, que es lo que
era; esta corrección es la que la actualiza", y el párrafo siguiente ya declara cerrados
#2105 y #2106. Editar la lista sería **falsificar un registro histórico**: el 2026-10-20
##2105 estaba abierta. El cierre de #2105 se sostiene con la evidencia, no con un diff:
`LocalEmbeddingPort` → **0 símbolos**; `Tier2EmbeddingPort` es un newtype
(`pub struct Tier2EmbeddingPort(Arc<dyn EmbeddingPort>)`, sin variante `Local`); el colapso
ya entró en `c4468e7f` (PR #2154). ⇒ T5 se cancela: el ADR queda intacto y #2105 se cierra
por evidencia (comentario en el issue).

---

## Tasks

- [ ] T1 · S1 — `scripts/merge-when-green.sh`: leer `state` y `mergeStateStatus` en una
  sola llamada; si `state == MERGED`, imprimir "already merged" y salir 0 **antes** del
  loop de reintentos de `UNKNOWN`; el loop también re-consulta `state`.
- [ ] T2 · S1 — `scripts/test_merge_when_green_merged_state.sh` (nuevo): harness de
  negación con `gh` falso, modelado sobre `scripts/test_merge_when_green_breaking.sh`.
  Caso obligatorio: PR `MERGED` ⇒ exit 0, mensaje "already merged", **sin** espera de 90 s.
  Casos de no-regresión: `state == OPEN` + `mergeStateStatus == UNKNOWN` ⇒ sigue
  reintentando; `CLEAN`/`UNSTABLE` ⇒ el camino de merge intacto; `BEHIND`/`BLOCKED` ⇒
  exit 3 intacto. El harness debe cablearse en el job `repo-guards` de
  `.github/workflows/ci.yml` (junto a `scripts/test_merge_when_green_breaking.sh`, ~línea
  606): un harness que ningún workflow nombra es decoración y el metaguard lo detecta.
- [ ] T3 · S2 — `AGENTS.md` §Post-merge cleanup: el borrado del target dir se ejecuta como
  comando **standalone** con el path absoluto literal (`/home/xavi/.cache/cargo-target/<name>`),
  nunca con `$HOME`/`~` y nunca encadenado; la cadena `git branch -D` + `git worktree
  prune` corre antes y completa. Sin gate nuevo, sin helper de ownership.
- [ ] T4 · S3 — `AGENTS.md` §Worktree policy: el registro de misión
  (`odd/tasks/<feature>.md`) se crea y edita **dentro del worktree de la misión**, nunca en
  el checkout principal. Sin chequeo automatizado nuevo.
- [x] T5 · S4 — **CANCELADA por la revisión del padre**: el ADR no cambia (registro
  histórico congelado). #2105 se cierra por evidencia, no por diff.
- [ ] T6 · S1–S4 — verificación: `bash -n scripts/merge-when-green.sh`; el harness nuevo en
  verde; `bash scripts/test_merge_when_green_breaking.sh` sigue en verde (el guard de
  breaking-marker no se toca); `bash scripts/test_guard_coverage.sh` en verde (el harness
  nuevo está cableado y no abre una entrada en el ratchet); `bash scripts/test_ci_merge_when_green.sh`
  en verde; `bash scripts/ci_fast_gate.sh` en verde si el tiempo lo permite.

## Resultado de la implementación (2026-10-21)

Detalle por task y evidencia RED/GREEN: `odd/merge-when-green-merged-state/tasks.md`.
El árbol queda sin commitear: el commit es del padre. **No hay PR** (decisión del owner:
las PRs se abren más adelante).

### Veredicto por spec

- **S1 (#1970)** — `scripts/merge-when-green.sh` lee `state` y `mergeStateStatus` en **una
  sola llamada** (`gh pr view --json state,mergeStateStatus`) y sale 0 con "already merged"
  en tres puntos: antes del guard de breaking-marker, antes del loop `UNKNOWN` y dentro del
  loop (que re-consulta `state`, porque otro actor puede mergear el PR mientras espera).
  Sin flag nuevo y sin código de salida nuevo: el estado deseado ya está alcanzado.
- **S2 (#1971)** — `AGENTS.md` §Post-merge cleanup: el borrado del target dir es un comando
  **standalone** con path absoluto literal, nunca `$HOME`/`~` y nunca encadenado; la cadena
  de git (pasos 3-5) corre primero y completa. La primera rama de la AC (refinar el
  hard-deny) vive en el harness del agente, fuera del repo: no es repo-actionable.
- **S3 (#1972)** — `AGENTS.md` §Worktree policy: el registro de misión se crea y edita
  **dentro del worktree de la misión**. Sin gate automatizado nuevo.
- **S4 (#2105)** — cancelada: el ADR queda intacto (ver S4). El issue se cierra por evidencia.

### Decisiones de la revisión del padre (y por qué)

1. **Fuera el fallback legacy.** La primera entrega del worker caía a la proyección
   `--json mergeStateStatus` cuando la llamada combinada no devolvía nada. Eso es
   exactamente la forma "un hecho, dos lectores" que este lote viene a cerrar: un segundo
   lector que además **desactiva en silencio** la detección de "ya mergeado" (el defecto
   que se está arreglando). `gh pr view` sirve ambos campos desde la misma versión de API
   que ya introduce `mergeStateStatus` (el campo que el script leía antes), así que el
   fallback sólo existía para servir stubs. Decisión: **un solo lector**; los tres `gh`
   falsos de los harnesses sirven la proyección combinada.
2. **El "un solo lector" queda asertado, no supuesto.** El harness nuevo registra cada
   `pr view --json <campos>` que pide el script y afirma (a) que `state,mergeStateStatus`
   se leyeron **en una sola llamada** y (b) que el estado **nunca** se leyó por separado.
   Una regresión a dos lecturas no pasa desapercibida.
3. **Harness portable.** Se eliminó `timeout` (GNU coreutils): un macOS no lo tiene y un
   harness que corre en una sola plataforma es un harness que medio equipo nunca corre. El
   watchdog es bash puro.
4. **El RED tiene que ser preciso.** Con el stub serviendo sólo la forma post-fix, el
   script pre-fix moría en la primera lectura (rc=99) y el RED no distinguía "arreglado"
   de "el stub no entiende el pedido". El stub vuelve a servir las proyecciones
   individuales **a propósito** y la propiedad "un solo lector" se aserta explícitamente.
   RED real: rc=124 (el script pre-fix quemó la ventana de 90 s del loop `UNKNOWN` y
   murió por el deadline del harness — el síntoma exacto de producción), sin mensaje
   "already merged", 21 s de espera, y 2 lecturas individuales del estado.
5. **`AGENTS.md` en inglés.** El documento es íntegramente inglés; dos párrafos en
   español rompían el router. El doc ODD sí va en español (convención de `odd/tasks/`).
6. **Teredo/ISATAP, orden de precedencia, `mergedAt`**: `mergedAt` no vacío ⇔
   `state == MERGED`, así que `state` alcanza; no se agrega el segundo campo.

### Verificación (ejecutada por el padre sobre el árbol final)

```text
bash -n scripts/merge-when-green.sh                    exit=0
bash -n scripts/test_merge_when_green_merged_state.sh   exit=0
bash scripts/test_merge_when_green_merged_state.sh      PASS=25 FAIL=0
bash scripts/test_merge_when_green_breaking.sh          PASS=26 FAIL=0
bash scripts/test_ci_merge_when_green.sh                PASS=17 FAIL=0
bash scripts/test_topology.sh                           all checks passed
bash scripts/test_guard_coverage.sh                     FAIL (pre-existente, ver abajo)
```

`test_guard_coverage.sh` falla **igual que en `main`** (mismo mensaje, mismo archivo):
`scripts/test_intra_crate_direction.sh` exige el extractor intra-crate compilado con
cargo, que en CI construye el job `repo-guards` y en local no está. No es de este lote.

### Borde de slice (registrado por el hook de pre-commit)

El hook mide 431 líneas autorales en los 2 primeros commits: ya pasó el corte de ~400
líneas. Este repo **no tiene PRs apiladas**, así que la estrategia es **entrega
secuencial**, y el corte queda así:

| Slice | Commits | Contenido | PR (cuando el owner ordene) |
|---|---|---|---|
| 1 | `d16ffef2`, `67ff6128` | el fix de #1970 + sus harnesses + las dos reglas de #1971/#1972 | una PR, etiqueta `type:bug` (el diff es del script) |
| 2 | el commit de trackers | los docs ODD de este lote + el plan del backlog | una PR desde el `main` nuevo, etiqueta `type:docs` |

Ninguna PR se abre todavía (decisión del owner): el trabajo queda commiteado en el worktree.

### Pendiente de la revisión

- `bash scripts/ci_fast_gate.sh` no se corrió (el lote prohíbe cargo: es un build de
  BoringSSL). Lo corre el padre o CI.

## Log

L1 — Petición del owner (verbatim):

> tengo muchas y  varias issues abiertas y necesito un plan para ir implementando utliza la
> skill adecuada para triage de issues

L2 — Corrección del owner sobre el estado real del sistema (verbatim):

> debes tener en cuenta que nuesto sistema ha cambiado sobre todo en lo relacionado con
> modelos de *AI LOCAL* Y LAS ISSUES pueden decir cosas o indormacion antigua o falsa,

> utiliza siempre tu y ordenar a  todos los workers las potentes tools *codedb* y *codegraph*
> en lugar de las otras tools

L3 — Reparto de lotes (verbatim):

> ya tengo un agente trabajando en lote k1 en
> /home/xavi/Projects/Rust/webfang-worktrees/fix-ssrf-this-network-justification

L4 — Evidencia del triage (session `ses_ea41f680affeGRp0y786Lvfvt6` + workers B/C/E,
CodeDB 0.2.5849 + CodeGraph sobre `main@3a785d8e`):
- #1970 · el script lee sólo `mergeStateStatus`; para un PR mergeado GitHub responde
  `UNKNOWN` para siempre ⇒ 90 s de espera y salida 4 sobre un PR cuyo merge ya ocurrió.
- #1971 · el patrón hard-deny es `$HOME`/`~`; con path literal el comando cae en *confirm*.
  La AC ("antes de la cadena") es contraproducente: si el `rm` va primero y el hard-deny lo
  bloquea, la cadena tampoco corre.
- #1972 · el registro huérfano no es un defecto de código: es una regla ausente.
- #2105 · premisa falsa (0 símbolos); sólo queda la línea stale del ADR.

L5 — Restricción de colisión: este lote no toca `crates/`, `scripts/merge-when-green.sh`
solo, ni ningún archivo que toquen los worktrees vivos (`fix-ssrf-this-network-justification`,
`fix-export-data-integrity` → PR #2230, `feat-auto-label-pr-type` → PR #2228).

L6 — Revisión del padre sobre la entrega del worker (decisiones, verbatim del
razonamiento del padre):

> tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro producto.

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

Decisiones tomadas: (1) **fuera el fallback legacy** — un segundo lector de un hecho es
la forma que este lote viene a cerrar, y el fallback además apagaba en silencio la
detección que se arregla; (2) el harness nuevo **no depende de GNU `timeout`** (portable)
y su RED tiene que ser **preciso** (rc=124, el síntoma real), no "rojo porque el stub no
entiende el pedido"; (3) la propiedad "un solo lector" queda **asertada** desde el log de
lecturas; (4) `AGENTS.md` va **en inglés** (el documento es íntegramente inglés);
(5) **T5 cancelada**: el ADR no se toca.

L7 — Evidencia que cambió el alcance (verificada por el padre, no por el issue):

- `docs/adr/0004-onnx-local-removal.md` dice, textualmente, que su lista de residuo
  "queda como el registro del 2026-10-20, que es lo que era; esta corrección es la que la
  actualiza", y el párrafo siguiente ya declara cerrados #2105 y #2106 ⇒ la línea
  "#2105 (abierta)" **no está stale**: es el registro de un día en que estaba abierta.
- El ADR, en cambio, sí dice la verdad: el colapso de `LocalEmbeddingPort`/
  `Tier2EmbeddingPort::Local` ya está registrado, junto con el commit `c4468e7f`
  (PR #2154) y el hecho verificado con `codedb` (`LocalEmbeddingPort` → 0 símbolos;
  `Tier2EmbeddingPort` = newtype sin variante `Local`).
- `bash scripts/test_guard_coverage.sh` falla **igual que en `main`**: el harness culpable
  es `scripts/test_intra_crate_direction.sh`, que exige el extractor intra-crate compilado
  con cargo. Pre-existente, ajeno al lote.

L8 — Estado de la entrega: árbol commiteado en el worktree
`/home/xavi/Projects/Rust/webfang-worktrees/fix-merge-when-green-merged-state` (rama
`fix/merge-when-green-merged-state`), **sin PR** (decisión del owner: las PRs se abren más
adelante). El plan del backlog completo vive en
`odd/tasks/backlog-implementation-plan.md`.

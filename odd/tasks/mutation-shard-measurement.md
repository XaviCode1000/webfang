# L6 — Señal de mutación: conteo medido y matriz rebalanceada (#1844)

Plan padre: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes; vive en el branch
del Lote 2, `fix/merge-when-green-merged-state`, y viaja con él a `main`).
Este doc es la especificación de **un** lote: el conteo de mutantes del scope vigente se
**mide** (deja de ser la estimación `~600` heredada) y la matriz de shards se rebalancea con
ese número. Sin PR: el trabajo queda commiteado en el worktree.

Rama: `chore/mutation-shard-measurement` ·
Worktree: `~/Projects/Rust/webfang-worktrees/chore-mutation-shard-measurement`
Issue: #1844 (aprobado: `status:approved`, `type:chore`).

---

## Specs

**S1.** El issue (#1844), verbatim:

> Measure the weekly `mutants-weekly` per-shard draw after #1840 widened `examine_globs`
> (`application/export_factory.rs` + `adapters/**`), and rebalance the shard matrix from 64 to
> 96 if any shard hits the 90-minute wall again. Getting the exact mutant total requires
> cargo-mutants, which is not installed locally — install it (or measure in a
> `workflow_dispatch` run) before deciding.

Y sus criterios de aceptación (verbatim):

> - Exact mutant total for the current scope is measured and recorded in the `mutants.yml`
>   header comment, replacing the inherited `~600` estimate.
> - If the measured total divided by 64 approaches the per-shard draw that produced #1436, one PR
>   moves the matrix, `--shard k/N`, the job name and every comment from 64 to 96 shards;
>   otherwise this issue closes with the recorded number and no change.

**S2.** La medición es sobre el scope **vigente**: el de `.cargo/mutants.toml`
(`examine_globs`), que es la fuente única de la política de scope. El número se mide con la
herramienta que usa el propio CI (`cargo-mutants 27.1.0`, la misma versión que instala el
workflow) y con el modo que **no** ejecuta los mutantes: `cargo mutants --list`.

**S3.** El umbral de decisión es el del precedente #1436: a **~20 mutantes por shard**, 14
shards tocaron el muro de 90 minutos. Si `medidos / 64` llega a ese draw, la matriz se mueve
a 96 shards; si no, el issue se cierra con el número registrado y sin cambio.

**S4.** El cambio es **completo o nada**: la matriz (`shard: [...]`), el flag
`--shard k/N`, el nombre del job, el mensaje de advertencia y **todos** los comentarios que
nombran 64 quedan en 96. Un `--shard k/96` con una matriz de 64 (o al revés) deja shards
vacíos o mutantes sin cubrir: el reparto debe ser una partición.

**S5.** El número medido también queda en `.cargo/mutants.toml`, que es donde vive la
política de scope y el presupuesto de CI; los dos archivos tienen que contar lo mismo (el
propio archivo lo exige: "keep the two in agreement").

**S6.** La medición no compila: `cargo mutants --list` parsea el scope y lista los mutantes
**sin** construir el workspace. Es la operación barata; correr los mutantes sería el camino
caro y no está en el alcance de este lote.

---

## Tasks

- [x] T1 · S2, S6 — medir el total de mutantes del scope vigente con `cargo mutants --list`
  (cargo-mutants 27.1.0) y registrar el comando junto al número.
- [x] T2 · S3 — decidir con el criterio del precedente #1436: `medidos / 64` contra el draw
  de ~20 por shard.
- [x] T3 · S4, S5 — mover la matriz, el flag, el nombre del job, la advertencia y los
  comentarios de 64 a 96 en `.github/workflows/mutants.yml`, y dejar el número medido y el
  shard count de acuerdo en `.cargo/mutants.toml`.
- [x] T4 · S1-S6 — verificación: YAML válido, `actionlint` limpio con la lista de ignorados
  de CI, la matriz con 96 entradas (1..96), y el flag y el nombre del job en /96.
- [x] T5 · S1-S6 — commits de work-unit en `chore/mutation-shard-measurement` (sin PR: la
  abre el owner cuando el plan entero esté implementado).

Ruta: **inline**. El lote es una medición (0,71 s, sin compilar) más un reescrito de
comentarios y de la matriz: una sola unidad, sin pasos encadenados que justifiquen un
subagente.

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
W-03) y este **L6** cuelga de `.github/workflows/mutants.yml` + `.cargo/mutants.toml` —
archivos que L3 no toca (L3 vive en `ci-health.yml` y en un step de `ci.yml`). Sin solape.

L3 — Medición (2026-10-22, cargo-mutants 27.1.0, desde la raíz del worktree):

```bash
cargo mutants --list | wc -l
```

→ **1477** mutantes en el scope vigente. Coste: 0,71 s de reloj, 0,72 s de usuario, 62 MB de
RSS máximo, **cero** compilaciones (`grep -c compiling` → 0). Es la operación que el issue
pide: el conteo exacto, sin correr los mutantes.

L4 — La aritmética de la decisión (precedente #1436: a ~20 mutantes/shard, 14 shards
tocaron el muro de 90 minutos):

| Shards | Mutantes por shard | Veredicto |
|---|---|---|
| 64 (los de antes) | 1477/64 = **23,1** | POR ENCIMA del draw que tocó el muro → rebalance |
| 96 (los nuevos) | 1477/96 = **15,4** | por debajo del draw del muro |

→ Se mueve la matriz de 64 a 96, que es el número que el issue sanciona.

L5 — El `~600` que el header cargaba era una **estimación** heredada de antes de que #1825
ampliara el scope (`export_factory.rs` + `adapters/**`); nunca se midió. Reemplazada por el
número medido, con el comando de medición al lado para que el próximo lector pueda
re-derivarlo. El umbral que dispara el rebalance (el draw de ~20 por shard del #1436) queda
escrito en el header, con su origen, para que la próxima ampliación del scope sepa contra
qué número comparar.

L6 — Verificación: YAML válido (`yaml.safe_load`), `actionlint` con la lista de ignorados de
CI → exit 0, matriz con 96 entradas (1..96), `--shard k/96` y nombre del job
`cargo-mutants (shard k/96)`. Los "64" que quedan en los archivos son **historia** ("the
previous 64-way split drew ~23.1 mutants per shard"), no configuración.

L7 — Riesgo residual, anotado para el próximo: el conteo es una foto del scope de hoy. Cada
ampliación de `examine_globs` lo vuelve viejo; por eso el header dice que hay que re-medir
antes de confiar en el número, y por eso la cifra vive junto al comando que la produce.

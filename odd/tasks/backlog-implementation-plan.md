# Plan de implementación del backlog abierto — 12 clusters, 16 lotes

Objetivo: vaciar el backlog por **clase raíz**, no issue por issue. Cada lote es UN fix
por raíz, con su test nombrado, y se entrega commiteado en su worktree. Las PRs se abren
después, cuando el owner lo ordene.
Base: `main`. Regla de oro: **un lote a la vez, un worktree por lote, un solo escritor
por path**. Este documento es la especificación que leen los workers.

Estado al abrir este plan: 40 issues abiertas → 12 clusters → 16 lotes. K1 (SSRF) está
**ocupado** por el agente del owner y este plan no lo toca.

---

## Specs

**S1.** El owner deja la decisión al agente y exige que la decisión sea la adecuada, no la
fácil:

> te dejo la responsabilidad de decision a ti como experto, y si tienes que investigar en
> internet para buscar la mejor opcion o mejor alternativa tienes mi permiso total.

> pero tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro
> producto.

**S2.** No se abren PRs: el trabajo se conserva commiteado en el worktree.

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

**S3.** El plan es un documento ODD para ir implementando todo.

> y crea on plan odd para ir implementando todo

**S4.** K1 tiene dueño y no se toca desde acá.

> ya tengo un agente trabajando en lote k1 en
> /home/xavi/Projects/Rust/webfang-worktrees/fix-ssrf-this-network-justification

**S5.** El método (del skill `systemic-issue-triage`, vigente en este repo): clasificar por
clase raíz antes de tocar código; N issues que comparten raíz = **un** fix; cerrar contra
**tests nombrados**, nunca contra promesas; el fix de una falla que se reporta como
"dead end" es **una salida ejecutable**, no un guard; y el test de diseño antes de cada
arreglo: si el fix agrega un estado, un verbo, un flag o una representación paralela de la
verdad, hay que **borrar o relajar** algo en su lugar.

---

## Reglas de ejecución (no negociables)

1. **Un lote a la vez.** Un worktree por lote: `~/Projects/Rust/webfang-worktrees/<branch
   con / → ->`, nunca dentro del repo. `.envrc` de 5 líneas escrito desde el nombre del
   árbol; `codegraph init` + `codedb "$PWD" reindex` **antes** de delegar (sin los dos
   índices el worker está ciego).
2. **Un solo escritor por path.** Si dos lotes tocan el mismo archivo, se serializan. La
   tabla de colisiones de más abajo es la que decide el orden.
3. **Nunca dos builds simultáneas** en esta máquina: el OOM de BoringSSL ya tumbó el
   escritorio dos veces. `$(rustup which cargo)` (nunca `command -v cargo`, que devuelve el
   shim de mbx), `CARGO_BUILD_JOBS=1`, `systemd-run --user --scope -p MemoryMax=2G`.
4. **Harness de negación**: todo fix de comportamiento necesita un harness que se vea
   **RED contra el código pre-fix** y GREEN después. Un caso que no puede distinguir un
   script roto de uno arreglado no es evidencia. Y el RED tiene que ser **preciso**: rojo
   por la razón del defecto, no por un stub que no entiende el pedido.
5. **Commits de work-unit** (~400 líneas autorales por commit), con tests y docs al lado
   del comportamiento. Un commit cuyo `Risk` nombre un ítem de riesgo alto va solo.
6. **`status:approved` nunca auto-aplicada** (es del maintainer). Un `type:*` sí, si el
   form de issue/PR lo declara.
7. **Sin PR hasta que el owner lo pida.** Se deja el worktree commiteado y se reporta.
8. **El merge es del owner.** El agente entrega la rama commiteada.
9. **Nada de `git stash`, `git checkout`, `git switch`, `git worktree --force`.**
10. **Idioma**: los artefactos técnicos en inglés; `odd/tasks/**` en español (convención
    del repo); los mensajes de error al usuario final en español.

---

## Lotes

Estado: `HECHO` · `EN CURSO` · `OCUPADO` (dueño externo) · `PENDIENTE` · `BLOQUEADO` (falta
una decisión del owner).

| Lote | Cluster | Issues | Fix por raíz (una línea) | Test nombrado | Δ | Ruta | Estado |
|---|---|---|---|---|---|---|---|
| **L0** | Cierres por evidencia + backlog sin dueño | #2105, #1996, #1423, #2224, #1826, #1936 + nuevas **#2233**, **#2234**, **#2235** | cerrar con evidencia; abrir lo que no tenía dueño | evidencia citada por issue | 0 código | inline | **HECHO** |
| **L1** | SSRF: encapsulación IPv4 | #2231, #2232 | ISATAP y la forma IPv4-compatible revalidan la IPv4 embebida; Teredo primero; ISATAP **fuera** del set always-denied | `ssrf_a5_a6_family_test.rs` | +150/−51 | agente del owner | **OCUPADO** |
| **L2** | Merge y limpieza post-merge | #1970, #1971, #1972 | `state`+`mergeStateStatus` en una llamada ⇒ "already merged" exit 0; `rm` standalone con path literal; el registro de misión se escribe en el worktree | `scripts/test_merge_when_green_merged_state.sh` | +180/−15 | worker | **HECHO** (`d16ffef2`, `67ff6128`, `66f23d63`) |
| **L3** | **CI Health: el canal enmudecido** | #2233, #2234, #2162, #1936, #1980 | el clasificador matchea una línea de **FALLO**, no una mención; el test parsea hermético; el observer cierra por **subtítulo** de título | `scripts/test_ci_health_close.sh` (caso nuevo) | +40/−15 | worker | **L3a HECHO** (`1b49ef71`, `eaa172d7` en `fix-ci-health-w03-classifier`; harness partido en 468+474; #2162 y #1980 cerrados). **L3b HECHO** (`b08323cd` en `fix/2234-hermetic-args`: el helper no hermético `parse_args` **se borra** y `parse_args_hermetic` queda inlineado como único entry point ⇒ la hermeticidad es estructural; test nombrado `the_test_parse_surface_ignores_the_ambient_environment`; RED = el parseo desnudo moría con el env venenoso de la lane; GREEN = 2963 tests verdes **con** ese env; control negativo verificado). **L3a: review nativo APROBADO** (lineage `review-7279a44bd00f4c9f`, 2 lentes, 3 hallazgos no bloqueantes) |
| **L4** | Guard residuals (post-#2068) | #2070 (ítems 1-3; el 4º = **no hacer nada**) | `10#` en el comparador semver; el filtro de comentarios en `harness_wired`; `parse_envrc_target_decl` en vez del `sed` rígido | `scripts/test_bump_support_patch.sh` (**ya existe**, 735 líneas) + `scripts/test_guard_coverage.sh` (**el que tiene `harness_wired`**; ojo: hay dos harnesses de guard coverage y CI corre los dos) + `scripts/check_target_isolation.sh` | +90/−6 | worker | **HECHO** (`b2cf0a94`, `1111cd9d`, `cd333e76` en `fix-guard-residuals`; el ítem 4 documentado como "no hacer nada") |
| **L5** | Atribución de commits | #1995 | allowlist = identidades **exactas** de los bots (borra el prefix match) | `scripts/test_no_ai_attribution.sh` (caso nuevo) | −2/+18 | inline | **HECHO** (`4a17cb66` en `fix-ai-attribution-exact`; identidades medidas sobre la historia de main: `release-plz-tu-repo[bot]` ×20, `github-actions[bot]` ×8) |
| **L6** | Señal de mutación | #1844 | una medición (`cargo mutants list`) → `N`; borrar la estimación | shard `k/N` dentro de 90 min | −8/+12 | inline | **HECHO** (`26792e6b` en `chore-mutation-shard-measurement`): 1477 mutantes medidos (0,71 s, sin compilar) ⇒ 23,1 por shard en 64 ⇒ matriz rebalanceada a **96 shards** |
| **L7** | MCP: superficie verdadera | #1815, #1817, #1837, #2193 | `match_url_pattern` documenta lo que hace; `mcp = []` (la key **no** se borra); el conteo 36 en una sola fuente; `eq_ignore_ascii_case` en el path gate; la constante de shutdown `pub` | `path_gate.rs::components_start_with_never_accepts_a_case_variant_the_filesystem_keeps_distinct` | +18/−10 | worker | **DEL PEER**. No necesita build de ONNX: `webfang_core` **no tiene** feature `ai` (`[features]`: `default = []`, `loom-model`, `persistence`, `console`, `dev-tracing`, `adaptive-selectors`, `mcp`) y `crates/webfang_ai` no existe (ADR-0004 paso 6) ⇒ `--all-features` no enciende ONNX |
| **L8** | Falsos verdes de test | #2066, #1852, #1828, #1833, #1834, #1816 | detectar el descarte de SIGKILL (`getpid()==1`) y morir diagnosticamente; `default = ["mcp"]` + guard de target con 0 tests; `PRAGMA integrity_check` en vez de un punto de arming nuevo; el test de Windows **in-module** | `crash_matrix_test.rs::crash_point_never_hangs_when_sigkill_is_discarded` | +25/+150 | worker | **DEL PEER** (implementación). **Spec ODD commiteada: `da23c097` en `fix/false-green-tests`** (8 S#, 12 tasks; 7 premises falsas + 1 verde falso nuevo: `tokio_console_smoke_test.rs` está vacío en todos los builds porque `tokio_unstable` es un cfg, no una feature; S7/#1833 queda sin implementar a la espera de O4). O1 resuelta; O4 ya decidido (D19: la propuesta de scratch-crate se rechaza como gate) |
| **L9** | Grafo de build | #1799, #1797, #1796 | borrar `sys-info` (devuelve **KiB en todas las plataformas**: la premisa de la issue era falsa) y `once_cell`; dropear `version_string()` (0 call sites) y no escribir `build.rs`; el benchmark declara su propia feature `chromium` | `autotuning.rs::ram_budget_from_bytes_converts_to_kib` | −20/+40 | worker | **DEL PEER** (implementación; Risk item 5 ⇒ commit propio). **Spec ODD commiteada: `eb40ecbb` en `fix/build-graph-deps`** (4 areas, 9 tasks; el RED honesto del lote es de **grafo**, no de comportamiento; `#[cfg(not(miri))]` es obligatorio porque `sysinfo` llama `sysconf` al construir `System`) |
| **L10** | Crates sin mantenimiento | #1984, #2031 | borrar `serde_yaml` (sólo serializa en prod) y salir de `keyring` 3 preservando el **mismo store** (linux-keyutils); las 3 supresiones zbus expiran 2027-01-17 | `scripts/test_advisory_policy.sh` (caso nuevo) | −30/+45 | worker | **DEL PEER** (implementación; Risk item 5 ⇒ commit propio). **Spec ODD commiteada: `2f51441a` en `fix/unmaintained-crates`** (8 S#, 8 tasks; 4 premises falsas corregidas + un hallazgo crítico: el store nuevo **cambia la clave de la credencial**, así que sin el modificador `description` las credenciales ya guardadas quedan infindables en macOS/Windows) |
| **L11** | obscura | #1710 | el veredicto `Unknown` **nombra el mínimo esperado**; el hit-rate se deriva de los logs por capa; sin modo estricto, sin contadores | `preflight.rs::unknown_verdict_names_the_expected_minimum` | +6/−1 | inline | **EN CURSO** (peer, `fix-obscura-unknown-verdict`; es el primer lote que compila ⇒ serializa con L3b) |
| **L12** | Duplicación | #1757 | helpers de arrange compartidos; bajar el baseline por el delta **medido** (9272) | `scripts/check_duplication.sh` | −60/+12 | inline | PENDIENTE (espera **#2230**, que sigue abierta) |
| **L13** | Entrega macOS | #1821 | — | — | 0 | inline | **DESCARTADO 2026-10-22** (O3): el owner decide *"la issue 1821 la dejamos, no la tocamos"*. El lote sale del plan; la issue queda abierta y sin tocar |
| **L14** | Docs y runbooks | #1609, #2061, #1826 | cerrar #1609 con veredicto por fila y abrir ≤2 slices vivas; **rechazar** el auto-PR de rotación (el gate `published` ya corre dentro de `Validate PR metadata`); el WAL/SQLite no tiene runbook en ningún lado | `state_store.rs::test_load_or_default_*` | +55/−40 | inline | **DIVIDIDA**: el veredicto por fila de #1609 lo cierro yo (worker read-only + comentario en la issue); la implementación (runbook WAL/SQLite, nota de #1710, slices) es **DEL PEER** (sin build). **Veredicto posteado** (25 filas: 12 resueltas, 7 moot, 5 vivas, 1 no accionable) y las 5 vivas abiertas como **#2241** (doc de disclosure: DF-D1+DF-D4+DF-D6), **#2242** (la degradación de config es invisible: FR-10) y **#2243** (podar el seam muerto: M1/G1). #1609 **queda abierta** como umbrella; sus PRs la cierran con `Closes part of #1609` |
| **L15** | Lane de UB | #2235 | el UB de `scraper_service.rs` con gate **por test**; cada `--skip` con motivo escrito; la lane deja resultado, no aborta el proceso | job `Miri (core)` en verde en una corrida schedule | +20/−10 | worker | **DESBLOQUEADO** (O8 resuelta 2026-10-22) |

---

## Mapa de colisiones (dos lotes que tocan el mismo path se serializan)

| Path | Lotes que lo tocan | Orden |
|---|---|---|
| `scripts/merge-when-green.sh`, `scripts/test_*merge_when_green*` | L2 | L2 primero |
| `scripts/test_ci_health_close.sh`, `.github/workflows/ci-health.yml` | L3 | L3 |
| `scripts/check_no_ai_attribution.sh` + harness | L5, y L4 (si toca el filtro) | L4 → L5 |
| `scripts/test_guard_coverage.sh`, `scripts/check_target_isolation.sh`, `scripts/bump-support-patch.sh` | L4 | L4 |
| `scripts/quality-baselines.json` | L10, L12, y PR #2230 (abierto) | #2230 → L12 |
| `crates/webfang_mcp/**` | L7, L8 | L7 → L8 |
| `crates/webfang_core/src/domain/ssrf_guard.rs` | L1 | **L1 (agente del owner)** |
| `Cargo.toml` (workspace + crates) | L9, L10 | L9 → L10 |
| `docs/adr/0004-onnx-local-removal.md` | nadie (L0 lo decidió: no se toca) | — |
| `.github/workflows/ci.yml` | L2 (step del harness), L3, L4, L13 | serializar: cada uno con su step como anchor |

Regla: **dos writers vivos no pueden compartir un path**, aunque toquen regiones
distintas del mismo archivo. Las entregas que tocan `ci.yml` se serializan.

Regla de `ci.yml` (repo sin PRs apiladas): cada lote commitea **su** step en su branch, con
el mismo estilo (nombre + comentario con el número de issue, el porqué y las 3 líneas
"Offline: fake gh, no network, no cargo"); al abrir la PR, el step se re-aplica sobre el `main`
nuevo (el conflicto es de un bloque de step, no de lógica).

---

## Estrategia de entrega (por qué los lotes se Deliver en slices)

Este repo **no tiene PRs apiladas**: `scripts/check-topology.sh` sólo admite head→base de
su tabla, y ningún head de feature puede apuntar a otra rama de trabajo. La vía que
funciona es **entrega secuencial**: slice 1 como PR normal sobre `main` → merge → slice 2
desde el `main` nuevo → PR normal → repetir. Nunca se retargetea un PR abierto.

Reglas derivadas para los lotes de este plan:

- Un lote que pase de ~400 líneas autorales se parte en slices por **comportamiento**, no
  por cantidad: el slice 1 lleva el código + sus tests, el slice 2 lleva los docs/trackers
  cuando el cambio es de otra clase (`type:docs`).
- Cada PR lleva **una** etiqueta `type:*` (la del vocabulario del repo: `type:bug`,
  `type:feature`, `type:refactor`, `type:docs`, `type:chore`, `type:testing`,
  `type:breaking-change`), la issue enlazada con `Closes #N` y **cero atribución de IA**
  (el guard `scripts/check_no_ai_attribution.sh` corre en cada commit y en el push).
- `main` se mueve cada ~15 min por otras sesiones y la protección tiene `strict: true`: una
  rama que quedó atrás hay que actualizarla antes del merge, y eso dispara un ciclo de CI
  completo. Se avisa antes de encadenar varios.
- El merge es del owner. El agente entrega la rama commiteada y el PR listo.

## Decisiones tomadas por el agente (con su fundamento)

| # | Decisión | Por qué |
|---|---|---|
| D1 | El fix de #1970 lee `state` y `mergeStateStatus` en **una sola llamada**, sin fallback | Un segundo lector de un hecho es la forma que el lote viene a cerrar, y el fallback además **apagaba en silencio** la detección del defecto. `gh` sirve ambos campos desde la misma versión de API que ya introduce `mergeStateStatus` |
| D2 | El harness nuevo **no depende de GNU `timeout`** | Un macOS no lo tiene; un harness que corre en una sola plataforma es un harness que medio equipo nunca corre |
| D3 | El RED tiene que ser **preciso** (rc=124, el síntoma real) | Un stub que sólo entiende la forma post-fix hace que el script pre-fix muera en la primera lectura: rojo por la razón equivocada |
| D4 | "Un solo lector" queda **asertado** desde el log de lecturas | Una propiedad no asertada es una propiedad que se pierde en el próximo refactor |
| D5 | El ADR 0004 **no se toca** | Su lista de residuo es un registro histórico congelado y la corrección que la actualiza ya está escrita; editarla sería falsificar el registro |
| D6 | La key `mcp` de `webfang_cli` se vuelve `mcp = []`, **no se borra** | Es load-bearing: `release.yml`, `docs.yml`, `check_compatibility.sh`, `gen-cli-reference.sh`, third-party notices |
| D7 | #1837 → `eq_ignore_ascii_case` (ASCII-narrow) | El par que sobre-acepta es `ß`/`ẞ` (Unicode los pliega, NTFS y APFS los distinguen); ASCII-narrow es estrictamente más estricto que ambos filesystems ⇒ falla cerrado. Se documenta la limitación (un root macOS con nombre no-ASCII queda rechazado) |
| D8 | #1852 → `default = ["mcp"]` **+** `default-features = false` en `webfang_test_utils` | Sólo con la primera línea el fix es invisible: la unificación de features por la dev-dependency vuelve a re-encender los defaults de `webfang_core` |
| D9 | #1799: `sys-info` sale, `sysinfo` 0.30 entra (ya era dependencia de core ⇒ **ninguna dep nueva**); una sola fuente, una sola unidad, y el nombre que la dice (`ram_budget_from_bytes` público + regla KiB privada); `ram_budget_from` (nombre ambiguo) se borra. **La premisa de la issue era FALSA** y se corrigió el 2026-10-22 | `sys-info` 0.9.1 devuelve **KiB en todas las plataformas** (`c/darwin.c` divide por 1024 igual que `linux.c`, verificado en el registry), no bytes en macOS: no hay sobreestimación y el código del repo ya convierte bien (`ram_budget_from(total_kib)` → `kib × 1024 × 0.7` → bytes). El defecto real es de **mantenimiento**: dos crates para el mismo hecho (`sys-info` y `sysinfo`), una unidad que `sys-info` no documenta (deriva futura) y un nombre de función que no la dice. Por eso el RED honesto del lote es de GRAFO, no de comportamiento |
| D10 | #2031 → `keyring-core` + `linux-keyutils-keyring-store` (el **mismo** store) | `keyring` 4 con Secret Service movería las credenciales del usuario: las entradas existentes quedan infindables y `no_implicit_fallback_from_keyring_to_env` forbids la salida automática ⇒ un bloqueo nuevo sin salida. Además zbus 6 exige rustc 1.89 y el workspace es 1.88 |
| D11 | #1844 → **medir** una vez, no inventar un flag de shards | El baseline que hay es una estimación que ya no se sostiene; un flag nuevo sería estado nuevo |
| D12 | #1984 → borrar `serde_yaml`, **no** "purgar ignores muertos" | `cargo audit` lee el lock y **sí** detecta `paste 1.0.15`: purgar ese ignore deixa el Security Audit en rojo. Y el trío zbus no se toca antes de #2031 |
| D13 | #2070 ítem 4 = **no hacer nada** | La ventana de publicación pendiente es angosta (un parche, mismo minor, sin tag), numérica y se auto-cura con el tag. Un expiry agrega estado y una falla nueva por un riesgo que hoy no existe |
| D14 | #1821 → job **no bloqueante** y `xattr` intacto | Un paso de firma que falla cuando falta el secreto bloquearía cada release (bloqueo nuevo sin salida), y borrar el `xattr` antes de que exista artefacto notarizado convierte la única salida en un callejón |
| D15 | #2061 → **rechazar** el bot de issues/PR | El scope `published` ya corre dentro del `Validate PR metadata` (fail-closed); un bot nuevo es un escritor nuevo con su propia máquina de estados |
| D16 | #1828 → la fila WAL-kill se **rechaza**; `PRAGMA integrity_check` sobre las filas existentes | La fila WAL exigiría un punto de arming **nuevo** en producción para servir a un test: un verbo nuevo en el contrato del crash-harness |
| D17 | #1826 y #1710 se **comentan, no se cierran** | Un hilo con dos modos de falla: cerrar deja strandado al que espera la otra mitad |
| D19 | #1833 (scratch-crate para type-checkear `cfg(windows)`) se **rechaza como gate**: la lane Windows sigue siendo el único compilador real de Windows, y el test de Windows pasa **in-module** | Un scratch crate con una copia del módulo y la tabla de lints replicada a mano es un **segundo** compilador de Windows que no compila el crate real: puede quedar verde mientras el crate real está rojo. Un gate que puede mentir es peor que un procedimiento. La lane no shieldea (decisión #1633, registrada en el comentario de `ci.yml`) y el feedback de 25 min se acepta: el costo de la alternativa es un mecanismo falso |
| D18 | #1797: `version_string()` (0 call sites) se dropea y **no** se escribe `build.rs` | El issue pide un `build.rs` que nadie necesita: la función que lo alimentaría tiene 0 call sites, así que el archivo sería metadata falsa. Sin `build.rs`, el perfil de release sigue siendo el de Cargo, sin una arista que nadie puede reproducir |

---

## Decisiones que quedan con el owner (bloquean lotes)

| # | Decisión | Bloquea |
|---|---|---|
| O1 | `#1852`: cambiar feature flags (`default`) y una arista de dev-dependency | L8 | **RESUELTA 2026-10-22**: sí, `default = ["mcp"]` + `default-features = false` |
| O2 | `#1796/#1797/#1799/#2031/#1984`: tocar o remover deps y features | L9, L10 | **RESUELTA 2026-10-22**: los tres swaps autorizados (`sysinfo`, `keyring`, `serde_yaml`) |
| O3 | `#1821`: `status:approved` + credenciales de Apple | L13 | **RESUELTA 2026-10-22 — L13 DESCARTADO**: el owner decide *"la issue 1821 la dejamos, no la tocamos"*. No es diferido: el lote sale del plan, la issue queda abierta y sin tocar, y el peer **no** lo implementa |
| O4 | `#1833`: su propuesta revierte una decisión registrada (la lane CI es el único compilador de Windows) | L8 | **RESUELTA POR EL AGENTE (D19)**: la propuesta del scratch-crate se rechaza como gate; la lane Windows queda como único compilador real de Windows |
| O5 | `#1710`: la decisión stealth/standard es documentación (`docs/` está fuera de alcance por defecto) | L11 | **RESUELTA 2026-10-22**: `docs/` autorizado (runbook WAL/SQLite + la nota de #1710) |
| O7 | Orden de apertura de las PRs y de los merges (`main` se mueve cada ~15 min con `strict: true`) | todas | **RESUELTA 2026-10-22**: `L2 → L3a → L3b → L4/L5/L6 → L8/L9/L10`. Cada slice sale del `main` nuevo (este repo no admite PRs apiladas). Estado real: sólo **#2230** sigue abierta (exporter, `fix/export-data-integrity`); #2228 y #2229 ya se cerraron |
| O8 | `#2235`: Miri exige el toolchain `nightly`, que no está en la máquina | L15 | **RESUELTA 2026-10-22**: instalar nightly y hacer L15 |

**O6 salió de la tabla del owner** (2026-10-22): dropear `version_string()` (0 call sites) y
**no** escribir `build.rs` es una decisión técnica dentro del código, no un pedido de
permiso sobre deps. Queda como D18.

---

## Tasks

- [x] T1 · S1–S5 — cerrar L2: revisado, commiteado en 3 work-units en `fix-merge-when-green-merged-state`, reportado (sin PR).
- [x] T2 · S1–S5 — L3 (el canal enmudecido): L3a (`1b49ef71`, `eaa172d7`) y L3b (`b08323cd`) commiteados y verificados; el review nativo de L3a quedó **aprobado** (3 hallazgos no bloqueantes, ninguno abre corrección).
- [ ] T3 · S1–S5 — L4 (residuales de guard): 3 one-liners con sus harnesses; el ítem 4 se documenta como "no hacer nada" — peer de coordinación.
- [ ] T4 · S1–S5 — L5 (atribución): allowlist exacta de bots, con el caso que rewrite.
- [ ] T5 · S1–S5 — L6 (mutación): medir, fijar `N`, borrar la estimación.
- [ ] T6 · S1–S5 — L7 (MCP): doc del matcher, `mcp = []`, conteo 36 en una fuente, path gate ASCII-narrow, constante de shutdown `pub`.
- [x] T7 · S1–S5 — L8 (test infra): **spec ODD commiteada** (`da23c097`); la implementación es del peer.
- [x] T8 · S1–S5 — L9/L10 (grafo de build y crates): **specs ODD commiteadas** (`eb40ecbb`, `2f51441a`); la implementación es del peer.
- [ ] T9 · S1–S5 — L11 (peer, en curso), L12 (espera #2230) y L14 (**mi parte hecha**: veredicto de #1609 posteado + #2241/#2242/#2243 abiertas; la implementación es del peer).
- [ ] T10 · S1–S5 — L15 (lane de UB) y L13 (macOS) cuando O3 esté resuelta.
- [ ] T11 · S1–S5 — cuando el owner lo ordene: abrir las PRs lote por lote, con su issue enlazada y su `type:*` único.

---

## Log

L1 — Petición original (verbatim):

> tengo muchas y  varias issues abiertas y necesito un plan para ir implementando utliza la
> skill adecuada para triage de issues

L2 — Corrección sobre el estado del sistema (verbatim):

> debes tener en cuenta que nuesto sistema ha cambiado sobre todo en lo relacionado con
> modelos de *AI LOCAL* Y LAS ISSUES pueden decir cosas o indormacion antigua o falsa,

> utiliza siempre tu y ordenar a  todos los workers las potentes tools *codedb* y *codegraph*
> en lugar de las otras tools

L3 — Reparto de lotes (verbatim):

> ya tengo un agente trabajando en lote k1 en
> /home/xavi/Projects/Rust/webfang-worktrees/fix-ssrf-this-network-justification

L4 — Delegación de la decisión y formato de entrega (verbatim):

> te dejo la responsabilidad de decision a ti como experto, y si tienes que investigar en
> internet para buscar la mejor opcion o mejor alternativa tienes mi permiso total.

> pero tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro
> producto.

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

> y crea on plan odd para ir implementando todo

L5 — Evidencia del triage (5 workers read-only con CodeDB 0.2.5849 + CodeGraph sobre
`main@3a785d8e`, 40 issues): las 3 premises falsas confirmadas (#2105, #1996, #1423 ya
resueltos), los mecanismos falsos desmontados (#1815 ítems 1 y 3, #1833 insatisfacible,
#1826 ítems 1-2 moot), las precedencias de error fijadas (#1821, #1837, #2066, #1970,
#1984) y 3 issues nuevas por lo que no tenía dueño (#2233, #2234, #2235).

L6 — Regla de trabajo para los lotes que siguen: el harness es la prueba, el RED tiene que
ser preciso, y ninguna decisión se toma por comodidad.

L6 — Decisiones del owner (2026-10-22, verbatim):

> Sí: default=["mcp"] + default-features=false

> Los tres: sysinfo, keyring, serde_yaml

> Sí: runbook WAL/SQLite + nota de #1710

> Instalar nightly y hacer L15

Q1 era sobre `#1852` (O1), Q2 sobre los tres swaps de crates (O2), Q3 sobre si `docs/` puede
tocarse (O5), Q4 sobre Miri (O8). Quedan pendientes: O3 (credenciales de Apple: es una
acción del owner), O4 (`#1833` y la lane de Windows) y O7 (orden de apertura de PRs).

L7 — Estado de L3 (2026-10-22): L3a (clasificador W-03 + cierre por subtítulo) verificado por
el padre de forma independiente, no por el informe del worker:
- el harness da `PASS=104 FAIL=0`;
- el RED contra el estado roto (patrón viejo del clasificador + cierre viejo por título por
  origen) da `PASS=99 FAIL=5`, con los 5 checks fallando por la razón real;
- la semántica de la búsqueda de GitHub se verificó contra la API real: `CI failed on main
  in:title` trae 60 trackers de CI (31 de ellos con el título retirado) y **0** de Bug
  Discovery o Benches ⇒ la clave de cierre no se come los otros trackers;
- `#2162` y `#1980` cerrados con evidencia; `#1936` queda abierto con su comentario.

### 2026-10-22 · L3b commiteado + specs de L8/L9/L10 + veredicto de #1609

- **L3b HECHO** (`b08323cd` en `fix/2234-hermetic-args`): el helper no hermético `parse_args` **se borra** y `parse_args_hermetic` queda inlineado ⇒ un solo entry point de parseo en el crate, hermético por construcción. El test nombrado `the_test_parse_surface_ignores_the_ambient_environment` envenena `WEBFANG_URL`, parsea y afirma que el valor ambiental no llega a los args. El worker corrió la suite **con** el env venenoso de la lane (2963 verdes) y el control negativo (restaurar el entry point no hermético) reproduce el panic del RED. El gemelo de integración no se tocó: ya era hermético vía `sanitize_env` → `strip_poisoned_env`.
- **Review nativo de L3a APROBADO** (lineage `review-7279a44bd00f4c9f`, 2 lentes `review-risk` + `review-reliability`, 1330 líneas, tier high). Los 3 hallazgos son **no bloqueantes** (`R1-ci-close-search-scope`, `R3-001`, `R3-002`, los tres WARNING/informational): son la semántica conocida del cierre por subtítulo (`in:title` ANDa palabras ⇒ "CI failed on main" alcanza también "Scheduled CI failed on main") y el check del harness que exige una línea de FALLO. Se registran como seguimiento, **no** reabren el review. La autoridad quedó quemada con el acknowledgement exacto (`gentle-ai.review-acknowledged/v1`).
- **Specs ODD commiteadas** y entregadas al peer: L8 `da23c097` (`fix/false-green-tests`), L9 `eb40ecbb` (`fix/build-graph-deps`), L10 `2f51441a` (`fix/unmaintained-crates`).
- **D9 corregida** (ya en el plan): `sys-info` 0.9.1 devuelve **KiB en todas las plataformas** — `c/darwin.c` divide por 1024 igual que `linux.c` (verificado en el registry) — así que la premisa de #1799 ("bytes en macOS") es falsa y el código del repo ya convierte bien (`ram_budget_from(total_kib)` → `kib × 1024 × 0.7` → bytes). El defecto real es de mantenimiento (dos crates para el mismo hecho, una unidad que `sys-info` no documenta, un nombre que no la dice) ⇒ el RED honesto del lote es de **grafo**.
- **#1609**: veredicto por fila posteado (25 filas: 12 resueltas, 7 moot, 5 vivas, 1 no accionable). Las 5 vivas se abrieron como **#2241**, **#2242** y **#2243** (una raíz = un issue). #1609 queda abierta como umbrella.

### 2026-10-22 · Decisiones del owner (segunda tanda)

- **ADR-0013**: se autoriza un **addendum fechado** en `docs/adr/0013-state-store-port-seam.md` (el registro histórico no se reescribe; la corrección queda trazable con fecha). Amplía la autorización de `docs/`, que hasta ahora cubría sólo el runbook WAL/SQLite y la nota de #1710 ⇒ cada archivo de `docs/` que se toque lleva su motivo.
- **Orden de apertura de PRs (O7)**: `L2 → L3a → L3b → L4/L5/L6 → L8/L9/L10`, cada slice desde el `main` nuevo.
- **#1821 / L13**: **descartado** — *"la issue 1821 la dejamos, no la tocamos"*. No es un bloqueo: el lote sale del plan y la issue queda abierta sin tocar.

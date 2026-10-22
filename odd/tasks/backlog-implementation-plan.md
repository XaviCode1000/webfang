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
| **L4** | Guard residuals (post-#2068) | #2070 (ítems 1-3; el 4º = **no hacer nada**) | `10#` en el comparador semver; el filtro de comentarios en `harness_wired`; `parse_envrc_target_decl` en vez del `sed` rígido | `scripts/test_bump_support_patch.sh` (**ya existe**, 735 líneas) + `scripts/test_guard_coverage.sh` (**el que tiene `harness_wired`**; ojo: hay dos harnesses de guard coverage y CI corre los dos) + `scripts/check_target_isolation.sh` | +90/−6 | worker | **HECHO** (7 commits en `fix-guard-residuals`: `b2cf0a94`, `1111cd9d`, `cd333e76` (código) + `c0c675c8`, `be6eb2e4`, `d35f53b7`, `5f19fbb5` (docs); tip `5f19fbb5`; el ítem 4 documentado como "no hacer nada" (D13). **Review nativo ABIERTO**: `review-ead5e692fc9ea607` (peer) y `review-74427c00c7b8343a` (padre) ||
| **L5** | Atribución de commits | #1995 | allowlist = identidades **exactas** de los bots (borra el prefix match) | `scripts/test_no_ai_attribution.sh` (caso nuevo) | −2/+18 | inline | **HECHO** (`0f004bf4` doc + `4a17cb66` código en `fix-ai-attribution-exact`; identidades medidas sobre la historia de main: `release-plz-tu-repo[bot]` ×20, `github-actions[bot]` ×8) ||
| **L6** | Señal de mutación | #1844 | una medición (`cargo mutants list`) → `N`; borrar la estimación | shard `k/N` dentro de 90 min | −8/+12 | inline | **HECHO** (`01767192` doc + `26792e6b` código en `chore-mutation-shard-measurement`): 1477 mutantes medidos (0,71 s, sin compilar) ⇒ 23,1 por shard en 64 ⇒ matriz rebalanceada a **96 shards** ||
| **L7** | MCP: superficie verdadera | #1815, #1817, #1837, #2193 | `match_url_pattern` documenta lo que hace; `mcp = []` (la key **no** se borra); el conteo 36 en una sola fuente; `eq_ignore_ascii_case` en el path gate; la constante de shutdown `pub` | `path_gate.rs::components_start_with_never_accepts_a_case_variant_the_filesystem_keeps_distinct` | +18/−10 | worker | **HECHO** (peer): `dccd06da` spec, `c40fac93` (`#1815`, `#1817`), `80be9903` (`#2193`), `315441ab` evidencia; tip `315441ab` en `fix-mcp-surface-truth`. No necesita build de ONNX: `webfang_core` **no tiene** feature `ai` (`[features]`: `default = []`, `loom-model`, `persistence`, `console`, `dev-tracing`, `adaptive-selectors`, `mcp`) y `crates/webfang_ai` no existe (ADR-0004 paso 6) ⇒ `--all-features` no enciende ONNX ||
| **L8** | Falsos verdes de test | #2066, #1852, #1828, #1833, #1834, #1816 | detectar el descarte de SIGKILL (`getpid()==1`) y morir diagnosticamente; `default = ["mcp"]` + guard de target con 0 tests; `PRAGMA integrity_check` en vez de un punto de arming nuevo; el test de Windows **in-module** | `crash_matrix_test.rs::crash_point_never_hangs_when_sigkill_is_discarded` | +25/+150 | worker | **HECHO** (peer): spec `da23c097` (padre) + `8ae91119` (matriz de crash y runbook, `#1828`), `4221bb4f` (cierre de consola REAL de Windows, `#1834`), `934e7f18` (pacing 429 observable, `#1816`), `1702d1ec` (comentario de seguridad que exige clippy en el `getpid` nuevo), `fd9596a5` (T6 cerrado con desviación), `24a819c2` (verificación final); tip `99e05793` (16 commits) en `fix-false-green-tests`. O1 resuelta; O4 ya decidido (D19) ||
| **L9** | Grafo de build | #1799, #1797, #1796 | borrar `sys-info` (devuelve **KiB en todas las plataformas**: la premisa de la issue era falsa) y `once_cell`; dropear `version_string()` (0 call sites) y no escribir `build.rs`; el benchmark declara su propia feature `chromium` | `autotuning.rs::ram_budget_from_bytes_converts_to_kib` | −20/+40 | worker | **HECHO** (peer; Risk item 5 ⇒ commit propio): spec `eb40ecbb` (padre) + `b62a4ac2` (guard del grafo de build), `845bfb08` (el grafo deja de llevar lo que no se usa, `#1799`/`#1797`/`#1796`), `6b109426` (baseline de crates duplicados regenerada), `713502cd` (evidencia); tip `713502cd` en `fix/build-graph-deps`. D9 corregida: `sys-info` devuelve KiB en todas las plataformas ||
| **L10** | Crates sin mantenimiento | #1984, #2031 | borrar `serde_yaml` (sólo serializa en prod) y salir de `keyring` 3 preservando el **mismo store** (linux-keyutils); las 3 supresiones zbus expiran 2027-01-17 | `scripts/test_advisory_policy.sh` (caso nuevo) | −30/+45 | worker | **HECHO** (peer; Risk item 5 ⇒ commit propio): spec `2f51441a` (padre) + `e02d1829` (ningún ignore de `deny.toml` puede quedar muerto, `#1984`), `905183ac` (evidencia de S1); tip `6b4bd8ed` (8 commits) en `fix/unmaintained-crates` ||
| **L11** | obscura | #1710 | el veredicto `Unknown` **nombra el mínimo esperado**; el hit-rate se deriva de los logs por capa; sin modo estricto, sin contadores | `preflight.rs::unknown_verdict_names_the_expected_minimum` | +6/−1 | inline | **HECHO** (peer): `0e52f031` spec, `6b82e087` (el veredicto `Unknown` nombra el mínimo esperado, `#1710`), `0747fea3` (cierre con los shas); tip `0747fea3` en `fix-obscura-unknown-verdict` ||
| **L12** | Duplicación | #1757 (**parte**, no la cierra) | helpers de arrange compartidos; **bajar el ratchet al valor medido del candidato** (el baseline commiteado 9272 está rancio) | `scripts/check_duplication.sh` | −60/+12 | inline | **DESBLOQUEADO 2026-10-22** (la PR `#2230` mergeó): spec ODD en escritura por un worker; la implementación la lleva el peer |
| **L13** | Entrega macOS | #1821 | — | — | 0 | inline | **DESCARTADO 2026-10-22** (O3): el owner decide *"la issue 1821 la dejamos, no la tocamos"*. El lote sale del plan; la issue queda abierta y sin tocar |
| **L14** | Docs y runbooks | #1609, #2061, #1826 | cerrar #1609 con veredicto por fila y abrir ≤2 slices vivas; **rechazar** el auto-PR de rotación (el gate `published` ya corre dentro de `Validate PR metadata`); el WAL/SQLite no tiene runbook en ningún lado | `state_store.rs::test_load_or_default_*` | +55/−40 | inline | **DIVIDIDA**: el veredicto por fila de #1609 lo cierro yo (worker read-only + comentario en la issue); la implementación (runbook WAL/SQLite, nota de #1710, slices) es **DEL PEER** y está HECHA: `f79e42ba` spec, `08d46907` docs, `0616c3bf` registro, `da990e2d` carry-over del wiki, `a902229f` evidencia; tip `cc380657` (5 commits) en `fix/docs-runbooks-wal` (~650 líneas en 3 commits, cada uno bajo el corte de ~400, **sin PR y sin publicar nada en GitHub**, sin tocar `ci.yml` ⇒ no colisiona con la re-aplicación de los steps). **Veredicto posteado** (25 filas: 12 resueltas, 7 moot, 5 vivas, 1 no accionable) y las 5 vivas abiertas como **#2241** (doc de disclosure: DF-D1+DF-D4+DF-D6), **#2242** (la degradación de config es invisible: FR-10) y **#2243** (podar el seam muerto: M1/G1), las tres con `status:approved` del owner. #1609 **queda abierta** como umbrella; sus PRs la cierran con `Closes part of #1609` ||
| **L15** | Lane de UB | #2235 | el UB de `scraper_service.rs` con gate **por test**; cada `--skip` con motivo escrito; la lane deja resultado, no aborta el proceso | job `Miri (core)` en verde en una corrida schedule | +20/−10 | worker | **HECHO** (peer): `6065049e` (la lane de UB deja de saltarse árboles de test, `#2235`), `cf4403fa` (guard de las invariantes de la lane), `f0e39852` (harness que prueba que el guard puede fallar), `4a9cb8c0` (registro del cierre con su evidencia); tip `4a9cb8c0` en `fix-miri-ub-lane`. O8 resuelta 2026-10-22 ||

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
| `.github/workflows/ci.yml` | L2 (step del harness), L3, L4, L13, **L12** (medido: su spec mete un harness nuevo en el job `repo-guards`) | serializar: cada uno con su step como anchor |

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
| O4 | `#1833`: su propuesta revierte una decisión registrada (la lane CI es el único compilador de Windows) | L8 | **RESUELTA POR EL AGENTE (D19)**: la propuesta del scratch-crate se rechaza como gate; la lane Windows queda como único compilador real de Windows. Fundamento (redactado por el peer, que lo enuncia mejor): *"un gate que copia el sujeto no gatea el sujeto"* — el scratch-crate compila una COPIA, así que puede quedar verde mientras el crate real está rojo |
| O5 | `#1710`: la decisión stealth/standard es documentación (`docs/` está fuera de alcance por defecto) | L11 | **RESUELTA 2026-10-22**: `docs/` autorizado (runbook WAL/SQLite + la nota de #1710) |
| O7 | Orden de apertura de las PRs y de los merges (`main` se mueve cada ~15 min con `strict: true`) | todas | **RESUELTA 2026-10-22**: `L2 → L3a → L3b → L4/L5/L6 → L8/L9/L10`. Cada slice sale del `main` nuevo (este repo no admite PRs apiladas). Estado real de las PRs (verificado por la API, 2026-10-22): **2 abiertas** — `#2240` (release v5.0.1, `chore/release-2026-10-21T23-09-53Z`, CLEAN) y `#2230` (exporter, `fix/export-data-integrity`, BLOCKED); #2228 y #2229 ya se cerraron |
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

### 2026-10-22 · El peer implementó L7/L8/L9/L10/L11/L14/L15 sobre las specs; el review de L4 queda abierto

- **No había colisión**: mis 3 workers de spec terminaron (specs commiteadas, árboles limpios) y el peer implementó **encima** de mis commits de spec, en las mismas ramas (`fix/false-green-tests` tip `24a819c2`, `fix/build-graph-deps` tip `713502cd`, `fix/unmaintained-crates` tip `905183ac`). Los tests nombrados de cada lote existen (verificación estructural: `scripts/test_empty_test_targets.sh`, `scripts/test_build_graph.sh` + `autotuning::ram_budget_from_bytes_converts_to_kib`, `scripts/check_no_dead_ignores.sh`, `preflight::unknown_verdict_names_the_expected_minimum`, `scripts/test_miri_lane_invariants.sh`, `docs/src/state-recovery-runbook.md`).
- **L15 no lo tomé**: sólo verifiqué que `nightly` + `miri` ya estaban instalados. El lote está implementado por el peer en `fix-miri-ub-lane` (4 commits).
- **L13/#1821**: descartado por el owner (no diferido). No se implementa.
- **Review nativo de L4**: el assess del candidato `fix/guard-residuals` (tip `5f19fbb5`) devuelve `nativeRisk high` / `reviewDue true` ⇒ el review **sí** correspondía. Arrancó `review-74427c00c7b8343a` (2 lentes, 379 líneas, budget 190) y el forecast del slot 0 salió, pero el ack se rechaza con `collectBinding is missing or stale for current STATUS` aunque el binding entregado es **byte-idéntico** al que imprime el facade y al que reconstruye el CLI nativo (verificado con `diff`). Queda **abierto y documentado** (junto al lineage del peer `review-ead5e692fc9ea607`): el trabajo no depende del review, cada fix tiene su RED/GREEN, que es exactamente el `writerSelfVerification` que el assess pide. La receta **sí** funcionó en L3a: lineage `review-7279a44bd00f4c9f` ⇒ APROBADO, 3 hallazgos no bloqueantes, autoridad quemada.

---

## Readiness de PRs (2026-10-22): qué falta para abrir cada una

Las 5 puertas del gate `Validate PR metadata`: issue enlazada con `Closes #N`, la issue con
`status:approved` (**nunca auto-aprobada**), exactamente un `type:*`, nombre de branch
convencional, cero atribución de IA. El orden de apertura ya está firmado por el owner:
`L2 → L3a → L3b → L4/L5/L6 → L8/L9/L10`.

| Lote | Rama | Commits del PR | Cierra | `status:approved` | `type:*` | Listo |
|---|---|---|---|---|---|---|
| L2 (s1) | `fix/merge-when-green-merged-state` | `d16ffef2` | #1970 | ✓ | `type:chore` | **SÍ** |
| L2 (s2) | ídem | `67ff6128` | #1971, #1972 | ✓ | `type:chore` | **SÍ** (reglas de proceso en `AGENTS.md`) |
| L2 (s3) | ídem | `66f23d63` + el libro mayor | — (documental) | — | `type:docs` | **SÍ** (sin issue que cerrar) |
| L3a | `fix/ci-health-w03-classifier` | `1b49ef71`, `eaa172d7` | #2233 | ✓ | `type:chore` | **SÍ** (#2162 y #1980 ya cerrados; #1936 sigue abierto) |
| L3b | `fix/2234-hermetic-args` | `b08323cd` | #2234 | ✓ | `type:chore` | **SÍ** |
| L4 | `fix/guard-residuals` | `b2cf0a94`, `1111cd9d`, `cd333e76` + 4 docs | #2070 | ✓ | `type:chore` | **SÍ** |
| L5 | `fix-ai-attribution-exact` | `4a17cb66` + `0f004bf4` | #1995 | ✓ | `type:chore` | **SÍ** |
| L6 | `chore-mutation-shard-measurement` | `26792e6b` + `01767192` | #1844 | ✓ | `type:chore` | **SÍ** |
| L7 | `fix-mcp-surface-truth` | `c40fac93`, `80be9903` + spec/evidencia | #1815, #1817, #1837, #2193 | ✓ | `type:bug` | **SÍ** (4 issues, un `type:*` ⇒ revisar el tipo: el lote mezcla bug y docs ⇒ dos slices o un `type:bug` con nota) |
| L8 | `fix-false-green-tests` | `8ae91119`, `4221bb4f`, `934e7f18`, `1702d1ec` + docs | #2066, #1852, #1828, #1834, #1816 | ✓ | `type:testing` | **SÍ** (#1833 queda abierto: su S# está sin implementar a la espera de O4) |
| L9 | `fix-build-graph-deps` | `b62a4ac2`, `845bfb08`, `6b109426` | #1799, #1797, #1796 | ✓ | `type:chore` | **SÍ** |
| L10 | `fix-unmaintained-crates` | `e02d1829` + el commit en vuelo | #1984, #2031 | ✓ | `type:chore` | **espera** al worker (el worktree tiene 4 archivos sin commitear) |
| L11 | `fix-obscura-unknown-verdict` | `6b82e087` + spec/cierre | #1710 | ✓ | `type:chore` | **SÍ** |
| L15 | `fix-miri-ub-lane` | `6065049e`, `cf4403fa`, `f0e39852` | #2235 | ✓ | `type:chore` | **SÍ** |
| L14 (a) | `fix-docs-runbooks-wal` | `08d46907` | #1826 | — | `type:docs` | **falta `status:approved` en #1826** |
| L14 (b) | ídem | `da990e2d` | #1609 (`Closes part of #1609`) | ✓ | `type:docs` | **SÍ** |
| L14 (c) | — | — | #2241, #2242, #2243 | ✓ (aplicadas 2026-10-22) | `type:docs`/`type:bug`/`type:chore` | **SÍ** (3 issues con 3 tipos distintos ⇒ un PR por issue, no uno solo) |

**Aprobaciones resueltas (2026-10-22)**: el owner autorizó y se aplicó `status:approved` en
**#2241, #2242, #2243** (las issues del veredicto de #1609) y en **#1826** (el runbook
WAL/SQLite). Verificado por la API. Con esto **ningún lote queda bloqueado por el gate**: las
cuatro aprobaciones las pone el owner, nunca el agente.

**#2230** (la PR del exporter) no está rota: su cuerpo tiene `Closes #1812`, la issue tiene
`status:approved` y `type:bug`. Está `BLOCKED` por la **cola de 64 shards de cargo-mutants**
en `queued` ⇒ es espera de CI, no un defecto. L12 sigue esperando su cierre.

### 2026-10-22 · Correcciones del lado issues y un hallazgo de procesos colgados

- **#1815**: dos de sus AC están obsoletas (hablan de la feature `ai` y de un build "sin la feature ai"). La feature `ai` no existe en ninguna crate y no hay `ort`/`onnxruntime`; el feature set real de `webfang_core` es `default = []` + `loom-model`, `persistence`, `console`, `dev-tracing`, `adaptive-selectors`, `mcp`, `chromium` (verificado en los manifiestos y en la línea de comandos del `rustc` de un build `--all-features`). El pinning que queda es **default vs `--all-features`**, y las AI tools hoy están siempre registradas y responden *"funcionalidad no disponible"*. Corrección posteada en la issue: el texto contractual deja de pedir un build que no existe.
- **Procesos colgados (hallazgo propio)**: hay **3 procesos `webfang --resume --state-dir /tmp/gpYNCL2W/...` vivos desde 01:52**, con el padre ya muerto (reparentados a systemd) ⇒ son los hijos del **RED de #2066** (el `SIGKILL` descartado deja el proceso con vida), anterior a los commits del fix de L8 (02:04 en adelante). Orfanados hace ~3 h, `do_sys_pause`, ~33 MB cada uno. El repo no admite procesos colgados en segundo plano: reportados al peer (son de su lote) para que los recole.
- **Builds**: medido a las 04:48, **no hay ninguna compilación viva** (ni cargo ni rustc): el build de L3b terminó hace rato (el worktree está limpio en `b08323cd`) y el de L11 también. El slot de build está libre.

### 2026-10-22 · El protocolo de las seeds (propuesta del peer, con una corrección de timing)

La movida es correcta en mecanismo y hay que hacerla **con otro timing**:

- **La key de una seed fija `cargo_lock` y `workspace_contract`**, y el contrato del workspace es
  el hash de los manifiestos de los miembros (`cargo metadata --no-deps`, features + deps +
  `uses_default_features`). ⇒ **cualquier cambio de `Cargo.toml` o del lock invalida la seed**.
- La store tiene hoy `v1-5256d5a74e9f8731` (publicada 02:45, `built_from_commit` = main
  `3a785d8e`) con **`features = ""`**: es la recipe de **features default**, no la de
  `--all-features`. Las dos seeds de septiembre (`v1-da8aa82dd29c4018`, `v1-dfde71780c07a662`)
  son de otro manifest.
- `--all-features` **no es vocabulario de la recipe** (el escape hatch `--` se rechaza a
  propósito), así que la recipe equivalente se expresa con la lista explícita. La lista
  `--all-features` del workspace, verificada con `cargo metadata`, es:

  ```text
  webfang_cli/adaptive-selectors, webfang_cli/chromium, webfang_cli/console, webfang_cli/default,
  webfang_cli/dev-tracing, webfang_cli/mcp, webfang_cli/persistence,
  webfang_core/adaptive-selectors, webfang_core/chromium, webfang_core/console, webfang_core/default,
  webfang_core/dev-tracing, webfang_core/loom-model, webfang_core/mcp, webfang_core/persistence,
  webfang_mcp/chromium, webfang_mcp/mcp, webfang_mcp/persistence
  ```

  (canonicalizada por el propio script: `,` separado y ordenado; `webfang_benchmark` todavía no
  declara features ⇒ L9 le añade `chromium` ⇒ **la lista también cambia** con L9).
- **Timing**: publicar la seed de `--all-features` AHORA compra una ventana que se cierra en el
  próximo merge, porque L9 (sysinfo/once_cell/built) y L10 (serde_yaml/keyring) cambian
  `Cargo.toml` y el lock. La seed que vale es la del **main post-merge**: una vez publicados los
  lotes que tocan dependencias, se publican **dos** seeds (features default + la lista explícita)
  por ~18 min, y cubren todos los worktrees siguientes hasta el próximo cambio de dependencia.
  Publicar antes es gastar 9 min de slot en una key que muere con el merge.

⇒ Decisión: **no se publica ninguna seed hasta que L9/L10 aterricen en `main`**. El slot de build
sigue siendo de un build a la vez, y ahora mismo no hay build pendiente de mi lado.

### 2026-10-22 · Orden único de apertura (sirve también para la re-aplicación de `ci.yml`)

**Un solo orden para las dos cosas**: el orden en que se abren las PRs y el orden en que se
re-aplica el step de `ci.yml` sobre el `main` nuevo son la misma lista, restringida a los lotes
que tocan el workflow. Así no hay dos listas que diverjan.

```text
L2 (s1 → s2 → s3) → L3a → L3b → L4 → L5 → L6 → L7 → L8 → L11 → L14 → L15 → L9 → L10
```

**Por qué este orden**:

- **L2 primero**: es la corrección del tooling de merge y además es la primera que toca
  `ci.yml`; fija el patrón de re-aplicación para el resto.
- **L3a y L3b después**: el canal del CI Health es el defecto más visible (el observer cerraba
  trackers de más) y L3b es la hermeticidad del mismo carril.
- **L4, L5, L6**: residuales de guard, atribución de commits y la señal de mutación. **L6 no
  toca `ci.yml`**: toca `.github/workflows/mutants.yml` (otro archivo) ⇒ no genera conflicto.
- **L7, L8, L11, L14, L15**: lotes independientes que no cambian el grafo de dependencias.
- **L9 y L10 al final, y por una razón dura**: cambian `Cargo.toml` **y** el lock ⇒ cualquier
  slice abierta después tendría que re-aplicarse sobre un lock nuevo. Cerrarlos al final deja el
  lock quieto para todo lo demás.

**Conflicto de `ci.yml`, medido sobre `main...<rama>` (no de memoria)**:

| Rama | Toca `ci.yml` | Otro workflow |
|---|---|---|
| `fix/merge-when-green-merged-state` (L2) | **sí** | — |
| `fix/false-green-tests` (L8) | **sí** | — |
| `fix/miri-ub-lane` (L15) | **sí** | — |
| `fix/build-graph-deps` (L9) | **sí** | — |
| L12 (duplicación, rama propia) | **sí** (harness nuevo en `repo-guards`) | — |
| `fix/mcp-surface-truth` (L7) | no (hoy) | — |
| `chore/mutation-shard-measurement` (L6) | no | `mutants.yml` |
| `fix/ci-health-w03-classifier` (L3a) | no | `ci-health.yml` |

⇒ Los pasos de `ci.yml` se re-aplican **en este orden: L2 → L8 → L15 → L9** (L7 entra en el
lugar que le toque si su worker añade un step), **uno por uno sobre el `main` nuevo**, y todos en
el job `repo-guards`, junto a los `scripts/test_*.sh` vecinos, con el estilo de sus vecinos:
`name` + comentario con el issue y el porqué + la nota de que el step no corre cargo (offline).
Si dos steps caen en el mismo contexto, gana el que va primero en el orden de arriba.

**Regla de calidad de los harnesses (del harness de L9, ahora general)**: todo harness nuevo
necesita un **control positivo** — un check que falla cuando el defecto **no** está — además del
RED. Ejemplo del de L9: `once_cell` ausente de los manifiestos pero **presente** en
`Cargo.lock`. Un harness que puede dar verde por construcción no es evidencia.

### 2026-10-22 · L14 cerrado (peer) y el lado issues publicado (padre)

**L14 HECHO** (peer): `f79e42ba` spec · `08d46907` docs · `0616c3bf` registro · `da990e2d`
carry-over del wiki · `0616c3bf` evidencia · `cc380657` la fila de `default` de la tabla Build Features; tip `cc380657`, 5 commits, sin PR y
sin publicar nada en GitHub. El runbook nuevo (`docs/src/state-recovery-runbook.md` + entrada en
`SUMMARY.md`) sigue el principio pedido: **cada afirmación cita el test que la pinea, y lo que
ningún test cubre queda MARCADO como no cubierto** (`created_at` NULL, el VACUUM posterior al
prune y `PRAGMA integrity_check` / `wal_checkpoint` son herramientas de SQLite, no promesas de
webfang).

**Cuatro correcciones suyas, verificadas por mí contra el árbol**: `docs/src/overview.md` decía
"MCP server (35 tools)" → el número real y pineado es 36 (`crates/webfang_mcp/tests/stdio_handshake.rs`);
`troubleshooting.md` sugería "Try a different TLS fingerprint profile (`--tls-emulation`) **or JS
rendering**" contra un desafío WAF, y es falso: `hybrid_router.rs` dice que la detección de WAF
cortocircuita en la capa que la vio y **nunca** se escala al render ⇒ se quitó esa mitad; el
comentario del módulo de `obscura_downloader.rs` llevaba el piso viejo (0.2.0) contra el real
(0.2.5); y la fila del wiki (que prometía "Architecture, API reference, guides" en un wiki sin
páginas) quedó fuera del README.

**Un hallazgo que NO se corrigió y queda abierto**: la tabla **"Build Features"** del README sigue
mintiendo sobre `default` — `crates/webfang_core/Cargo.toml` tiene `default = []` (set vacío
explícito desde #1813 T4) y la fila sigue diciendo
`` `default` (`images` + `documents`) | Image and document extraction | ✅ Yes ``. El resto de la
tabla es correcto. Lo cierra la Slice B / un one-liner.

**Lado issues: los 4 comentarios del peer, posteados y verificados por la API** (`#2061` rechazo
del auto-PR de rotación · `#1826` estado real de las 4 filas · `#1710` estado real de los AC ·
`#1609` addendum), con **una corrección de ruta**: el borrador citaba
`infrastructure/crawler/elastic_ingestion.rs`, que no existe; el archivo real es
`crates/webfang_core/src/application/elastic_ingestion.rs` (los hechos del borrador se confirman
ahí: `ram_budget_bytes` y `.buffer_unordered(self.config.cpu_cores)`).

**El borrador de `#1609` NO se posteó tal cual**: contradecía el veredicto por fila ya publicado en
esa issue y proponía cerrar el umbrella con ≤2 slices, cuando el estado real son **3 issues por
raíz** ya abiertas y aprobadas con `#1609` como umbrella. Lo que se posteó fue un **addendum**: el
puntero al veredicto, las 3 issues, los 2 hallazgos nuevos (la fila de "Build Features" y la tabla
de sub-issues desactualizada: `#1647` y `#1622` ya están cerradas) y la decisión de **no cerrar**
`#1609` hasta que aterrice la última de las tres.

### 2026-10-22 · Verificación del punto delicado de L8 (el peer lo señaló) y candidatos de L10

**L8 — el punto que el peer marcó como delicado ESTÁ resuelto** (verificado en
`crates/webfang_core/src/cli/crash_points.rs`):

- Las dos ramas —el `SIGKILL` **descartado** (el proceso es el init de su namespace de PID:
  `getpid() == 1`) y el `kill` **rehusado**— pasan por el **mismo** camino de muerte:
  `die_diagnostically(reason)` → `tracing::error!` + `std::process::abort()`. El propio código lo
  dice: *"The ONE diagnostic death path of the crash-injection harness … Both branches — the
  refused `kill` and the discarded one — go through it, so a single test covers both. A path that
  only one branch used would leave the other one uncovered, which is how the discarded case
  stayed invisible until #2066."* ⇒ un solo test cubre las dos, como pedía el spec.
- El test nombrado `crash_matrix_test.rs::crash_point_never_hangs_when_sigkill_is_discarded`
  ejerce el caso **real**: lanza el hijo con `unshare --user --map-root-user --fork`, así que el
  `getpid()` del hijo es 1 dentro del namespace ⇒ no es un mock del init, es el init.

**L10 — los candidatos, y el solapamiento con L9 (respondido)**: los candidatos de L10 son
**`serde_yaml`** (el crate sin mantener que queda sólo en el frontmatter) y **`keyring` 3 con su
`zbus` 4** (las 3 entradas de `deny.toml` de #2030), más el ignore muerto de `RUSTSEC-2024-0436`.
**Ninguno** de ellos es `built`, `git2`, `libgit2-sys`, `sys-info`, `once_cell` ni `chromiumoxide`:
esos son hallazgos de **L9** y no de L10 ⇒ **no hay solapamiento**. Y L10 trabaja sobre el
**main actual** (`3a785d8e`), no sobre la rama de L9: su rama ya está basada en main ⇒ la
condición que el peer puso se cumple sola.

**Corrección de shas**: L14 tiene **5** commits, no 3 — `f79e42ba` spec · `08d46907` docs ·
`0616c3bf` registro/evidencia · `da990e2d` carry-over del wiki · `cc380657` la fila de `default` de la tabla Build Features (tip `cc380657`, 5 commits).

**El lado issues ya está publicado**: los 4 borradores de `odd/docs-runbooks-wal/issue-drafts.md`
salieron como comentarios verificados (`#2061` 6332290927 · `#1826` 6332291286 · `#1710` 6332291573 ·
`#1609` 6332291798), con la ruta de `elastic_ingestion.rs` corregida y el borrador de `#1609`
sustituido por un addendum (no se posteó tal cual: contradecía el veredicto ya publicado y
cerraba el umbrella antes de tiempo).

### 2026-10-22 · Dos issues nuevas del lane del exporter (#2237, #2238) y el cierre del hallazgo de procesos

**#2237** (`perf(exporter)`, `status:approved`): `recover_torn_tail` lee el archivo de export
**entero**, así que el pico de memoria sigue creciendo con el tamaño del archivo. Nació del
verificador independiente de la PR #2230: el índice de resume pasó de +18,4 MiB a +2,0 MiB sobre
un archivo de 16,5 MiB, pero el camino de recuperación del tail truncado no se midió.

**#2238** (`feat(mcp)`, `status:approved`): el resultado del export **no expone** el conteo de
líneas saltadas por no-UTF-8. La decisión de #1812 (saltar en vez de abortar, y que el skip sea
ruidoso: error estructurado + `skipped_non_utf8` en el summary) existe, pero la superficie MCP
no lo muestra ⇒ el operador que mira el resultado de la tool no ve lo que se saltó.

**Orden del lane del exporter** (dependencia dura: las dos cuelgan de lo que #2230 mergee):
`#2230` (abierta, `BLOCKED` por la **cola de 64 shards de cargo-mutants**, no por un defecto) →
`#2237` → `#2238` → **L12** (duplicación, que también espera el cierre de #2230).

**Cerrado el hallazgo de los 3 procesos colgados**: los pids `2441103`, `2441994` y `2442239`
(los hijos del RED de #2066, con el padre muerto) **ya no están** ⇒ recolectados. Nada queda
colgado en segundo plano.

**L10 en vuelo**: el worker pasó de los archivos de código a `deny.toml` y `fuzz/Cargo.lock`
(la fase T4/T5 del spec: purgar los ignores RUSTSEC-2026-0183/0184 sólo **después** de que
`git2` salga de los DOS lock, y regenerar `fuzz/Cargo.lock` después de purgar) ⇒ el orden que
exige el spec se está respetando.

### 2026-10-22 · D20: el cambio de clave del almacén de credenciales (decisión del agente, no del owner)

**Lo que preguntó el peer** (L10): el store nuevo cambia la clave de la credencial
(`keyring-rs:{user}@{service}` → `keyring:{service}@{account}`), o sea que las credenciales **ya
guardadas** quedan infindables en Linux (en macOS y Windows la clave es idéntica por
construcción: `service`/`account` y `target_name = {account}.{service}`). ¿Migración o decisión del
owner?

**Decisión (D20): NO es una migración y NO es una decisión del owner.** El requisito de producto
es inequívoco —*un refactor de dependencias de seguridad no puede dejar huérfanas las
credenciales del usuario*— así que lo decide el agente, y el mecanismo lo elige el worker.

**Evidencia verificada**:

- Hoy, `keyring` 3.6.3 construye la clave de keyutils en `src/keyutils.rs`
  (`new_with_target(target, service, user)`): sin target explícito,
  `format!("keyring-rs:{user}@{service}")` ⇒ **prefijo `keyring-rs:`, orden user@service**.
- El store nuevo (`linux-keyutils-keyring-store`) expone su construcción como **configuración
  pública**: `Store { id: String, delimiters: [String; 3], service_no_divider: bool }`, y su
  default es *"prefix `keyring:`, divider '@', no suffix"* ⇒ el prefijo **y** el orden cambian
  (documentado en la doc de `Store::new`).
- La API nueva **no tiene** un modificador `description` que fije la clave: la clave es la
  construcción del store. ⇒ **La premisa de mi propia spec de L10 ("hay que pasar el modificador
  `description`") era falsa** y se corrige.

**Criterio de aceptación (uno solo, y verificable)**:

> La clave de una entrada `(service, user)` en el almacén **debe ser byte-idéntica** a la de hoy:
> `keyring-rs:{user}@{service}`. Si el store puede producirla con su configuración, se configura
> así (sin migración, sin estado nuevo, sin lectura doble). Si no puede, el camino de lectura
> consulta **primero la clave legacy** y la credencial guardada sigue encontrándose.

En los dos casos: **cero migración, cero credenciales huérfanas, cero estado nuevo**. Un test
nombrado lo pinea (el que falla con el store por defecto y pasa con la configuración correcta):
`the_credential_key_is_byte_identical_to_the_legacy_key`.

**Por qué no "migrar"**: una migración escribe una credencial nueva y borra la vieja: es estado
nuevo, deja restos si falla a mitad, y no arregla a quien ya tenía la credencial guardada con la
clave vieja si el borrado falla. La clave que no cambia no tiene ninguno de esos problemas.

### 2026-10-22 · D21 (el punto de crash inalcanzable) y la verificación del código de Windows

**D21 — el punto `DURING_CANCEL_DRAIN`:** el peer descubrió, midiendo, que el punto está **armado
pero no puede dispararse**: la sesión de export sólo observa un token de cancelación cuando se
llama `ResumeContext::observing_cancel`, y **el único llamador en todo el crate está en el módulo
de tests de `export_factory`** ⇒ en producción `cancelled()` es siempre `false`, la rama
`Cancelled` del drain nunca corre, y el hijo del crash matrix con el token ya cancelado **sale
con código 0 sin llegar al drain**.

**Decisión**: el punto **se queda** (es el marcador de un requisito registrado: E8, "drain before
final persist"), pero **rotulado como lo que es** —un punto que hoy la producción no alcanza—; la
**fila del matrix no se escribe** (una fila de un punto inalcanzable es un verde falso, que es
justo la clase que este lote audita); y el **hueco de fondo se abre como issue propia** (**#2244**)
con las dos salidas explícitas: (A) cablear el token en `cli/export_flow.rs` —el export observa la
cancelación y no persiste estado parcial, que es la intención de E8— o (B) aceptar "el export
siempre termina" como contrato, y entonces **borrar el punto** y su expectativa. Es decisión de
producto ⇒ la lleva el issue con `status:approved` del owner, no un follow-up automático. Mi
recomendación: **A** (E8 es un requisito de auditoría registrado y el punto existe para guardarlo),
pero cambia el comportamiento visible del export ante `Ctrl-C` ⇒ la elige el owner.

**Verificación del código `#[cfg(windows)]` (T9 de L8): no hay check cruzado local.** Medido en
`.github/workflows/ci.yml`: la lane `test-crossplatform` corre en `[windows-latest, macos-latest]`
y ejecuta `cargo build -p webfang_cli --bin webfang` + `cargo nextest run --lib` +
`cargo nextest run --test '*'` ⇒ **un test in-module sí corre en la lane de Windows**, con
cmake/NASM preinstalados en el runner (lo dice el propio comentario de la lane). Un
`cargo check --target x86_64-pc-windows-msvc` en un host Linux **no puede** cerrar el caso:
`wreq → btls → btls-sys` compila BoringSSL desde C++ y necesita el toolchain MSVC, que aquí no
existe ⇒ el check local fallaría en el build script, y un check local que falla no es evidencia.
⇒ **La verificación de T9 es la lane de Windows**, que hoy es **advisory** (no está en `gate`
needs) ⇒ la fila del catálogo congelado (`docs/test-inventory.md`) dice **qué** la verifica
("lane de Windows") y el promote a required queda como follow-up aparte, como ya anticipa el
comentario de la propia lane. Es la misma principio que rechazó el scratch-crate: **la lane real
de Windows es la única que compila Windows**; un gate que compila una copia no gatea el sujeto.

**Un fallo propio del peer, de la misma clase que el lote audita**: el `default-features = false`
de `webfang_test_utils` **no surtía efecto** (cargo avisa que se ignora mientras la dependencia
del workspace no lo declare) ⇒ la inmunización de D8 estaba escrita y era un flag silenciosamente
ignorado. Corregido en la dependencia del workspace (`80ae3571`).

### 2026-10-22 · D21-A: el owner decide la salida A de #2244 (cablear la cancelación)

**Decisión del owner (2026-10-22): salida A.** `cli/export_flow.rs` entrega el token de
cancelación de la sesión al `ResumeContext` ⇒ la rama `Cancelled` del drain corre, y un export
cancelado **no persiste estado parcial**. Es la intención de E8 ("drain before final persist") y es
el requisito de auditoría registrado que el punto de crash guardaba.

**Lo que queda fijado por esta decisión**:

- **Cambia el comportamiento visible, a propósito**: hasta ahora `Ctrl-C` durante un export **no**
  lo detenía — el export terminaba y persistía el estado final. A partir de ahora, un export
  cancelado se detiene en el drain y no persiste nada parcial.
- **`DURING_CANCEL_DRAIN` deja de ser un verde falso**: mientras el cableado no exista, el punto
  está armado pero inalcanzable; una vez que existe, el punto es real y **la fila del matrix se
  puede escribir** (y el catálogo `docs/persistence-resume.md` la lista con el test que la pinea).
  Hasta entonces, la matriz sigue listando **sólo las filas que existen**.
- El punto **se queda** (marcador de un requisito vivo, no código muerto) y su documentación en
  `cli/crash_points.rs` **no** puede decir "reserved for PR5b".

**Criterios de aceptación (en la issue `#2244`, ya con `status:approved`)**: el token se entrega;
un test prueba que un export cancelado no persiste estado parcial (y que el camino no cancelado
no cambia); la fila del matrix se escribe **después** de que el cableado exista; la documentación
del punto lo describe como alcanzable y nombra el test que lo prueba.

**Orden**: toca el camino del export, que `#2230` (abierta) también toca ⇒ aterriza **después** de
que `#2230` mergee, para no re-basear dos veces seguidas el mismo camino. `#2237` y `#2238` son
independientes de esta decisión y conservan su orden.

### 2026-10-22 · L8 cerrado (peer) y el lado issues de L8 publicado (padre)

**L8 HECHO** (peer): 13 commits sobre mi spec `da23c097` — `4919caaf` (T3+T4: `default=["mcp"]`,
`default-features=false` en el harness, 4 targets gated con `required-features`), `072c0383` (T5: el
guard `scripts/check_empty_test_targets.sh` + su harness + el step en `repo-guards`), `22972eaf`
(T1+T2: el fix del cuelgue + su test), `335b8116` (T7: la integridad SQLite tras reabrir),
`126c421f` (T10: el pacing del 429 observable + la barrera del flush), `80ae3571` (el
`default-features` que no surtía efecto), `8ae91119` (T6: la tabla de la matriz y el runbook),
`4221bb4f` (T7/T9), `fd9596a5` (T6 con desviación), `934e7f18` (el pacing era inestable), `1702d1ec`
(el comentario de seguridad que exige clippy), `24a819c2` (evidencia y cierre), `99833e11` (la fila del inventario nombra la lane de Windows y no insinúa un gate) · `99e05793` (el rótulo de `during_cancel_drain`: de imposible a *aún no llega*). Tip `99e05793`, 16 commits.

**Dos defectos de la misma clase que el lote audita, aparecidos en el propio lote**:
el `default-features = false` de `webfang_test_utils` **no surtía efecto** (cargo avisa que se ignora
mientras la dependencia del workspace no lo declare) ⇒ la inmunización estaba escrita y era un flag
silenciosamente ignorado; y el guard nuevo leía el gate con `head -40`, así que dos archivos con el
gate más abajo se contaban **sin** gate ⇒ un falso PASS (el número real es 22, no 20).

**Verificación final del lote**: `check`, `clippy --all-targets --all-features -D warnings`, `fmt`,
`doc` con `RUSTDOCFLAGS=-D warnings`, los tres guards, la matriz de crash **27/27**, el store SQLite
con `persistence`, y la behavioral **168/168 dos veces seguidas** (el test del pacing era inestable:
medía desde el inicio del run en vez de desde la respuesta 429, y `--backoff-max-ms 50` capaba un
`Retry-After` de 1000 ms por debajo del límite que la aserción exigía).

**Lado issues de L8, publicado y verificado**: `#1852` comentario `6333191158` + **CERRADA** (sus AC
quedan satisfechas: AC1 con `default = ["mcp"]`, AC2 eliminada con el crate, AC3 con el guard
nuevo, AC4 con el conteo real pinedo en el harness) · `#1828` comentario `6333191355` (las
decisiones registradas de las filas sin test: la de WAL-kill con `PRAGMA integrity_check` sin kill
ni punto de arming nuevo, la de `during_cancel_drain` **inalcanzable en producción** y por eso sin
fila —con el hueco en `#2244`, ya aprobado—, y el `cfg(loom)` aceptado y documentado; el catálogo
lista las **diez** filas que existen) · `#1833` comentario `6333191560` (la decisión del 2026-09-30
sigue vigente: la lane real de Windows es la única que compila Windows; la propuesta del scratch
crate se rechaza porque *un gate que copia el sujeto no gatea el sujeto*; la issue queda a la
espera de O4).

**Corrección al estado que reportó el peer**: L10 **no** espera ninguna definición mía (D20 está
decidida desde `0509e2b8`: la clave de la credencial no cambia) y L13 **no** espera O3 (el owner lo
descartó: *"la issue 1821 la dejamos, no la tocamos"*). Lo único pendiente de L10 es la
implementación; lo único pendiente de L13 es nada.

### 2026-10-22 · L10 avanzado (peer): el swap de serde_yaml landed, el lock regenerado y D20 ya aplicado al spec

**L10** (peer), 8 commits sobre mi spec `2f51441a`, tip `6b4bd8ed`, árbol limpio, **sobre `main`**
(el rebase sobre L9 está *medido* con `merge-tree`, no ejecutado ⇒ L10 no depende de L9):

`e02d1829` (el guard de ignores muertos) · `905183ac` (la evidencia de S1) · `865652db` (el swap
`serde_yaml` → `serde_norway` + el borrado de la variante `ScraperError::Yaml`, marcado **BREAKING
CHANGE**) · `1040d312` (el lock del fuzz regenerado + el ignore de `unsafe-libyaml` purgado) ·
`0e750fab` (el rebase sobre L9 medido con `merge-tree`) · `71a2d02e` (**D20 aplicado al spec**) · `6b4bd8ed` (la lista de tasks fija el orden real: **T3 antes que T2** ⇒ la premisa corregida entra en el spec antes de implementarla).

**Una premisa mía que era falsa, corregida por el peer**: mi spec decía que la fork de YAML
elegida (`serde_norway`) no tenía release desde 2024-12. **Es falso**: `serde_norway` 0.9.42
existe y entra **por registro** (no por ref de git), con `unsafe-libyaml-norway ^0.2.13` (la fork
mantenida del C). El riesgo asumido en la spec (item 1) queda sin objeto: el riesgo real era otro
—que la salida del frontmatter no fuese byte-idéntica—, y la evidencia dice que **sí lo es**: el
snapshot `obsidian_byline_artifact_test__empty_byline_fragment_repaired_in_excerpt` queda
byte-idéntico, que es el criterio de aceptación del lote, y los 16 tests de `frontmatter.rs` pasan
(incluidos los dos pines de PI-7). El lock ya no tiene `serde_yaml` (0 ocurrencias).

**El bloqueo de L10 que quedaba (la clave de la credencial) ya no existe**: D20 está en el spec
(`71a2d02e`) ⇒ el criterio es *la clave de una entrada `(service, user)` debe ser **byte-idéntica**
a la de hoy, `keyring-rs:{user}@{service}`*; si el store puede producirla con su configuración
pública, se configura así; si no, la lectura consulta primero la clave legacy. Cero migración,
cero credenciales huérfanas, cero estado nuevo. Test: `the_credential_key_is_byte_identical_to_the_legacy_key`.

**Consecuencia para las seeds** (medida): L10 cambia `Cargo.toml` **y** el lock ⇒ la key de la seed
publicada sobre el `main` actual **no** la consume L10 (ni L9). Las dos seeds que cubren la fase de
apertura de PRs son las del lock de `main`; cuando L9 y L10 mergeen, hay que **volver a publicar**
las dos sobre el lock nuevo.

### 2026-10-22 · #2230 mergeada: L12 desbloqueado y el lane del exporter puede avanzar

La PR **#2230** (exporter: un JSONL no-UTF-8 degrada y salta en vez de abortar, `#1812`) **mergeó**
⇒ dos cosas que esperaban su cierre se destraban:

- **L12** (duplicación, `#1757`) queda **desbloqueado**: estaba esperando ese merge. Es el último
  lote de código del plan, así que ahora la spec ODD se escribe (con el baseline de duplicación
  **medido** con la herramienta del repo —la premise del plan dice 9272 y hay que **verificarla**,
  no darla por cierta—) y la implementación la lleva el peer.
- **El lane del exporter** (`#2237` —el pico de memoria de `recover_torn_tail`— y `#2238` —el
  resultado MCP del export no expone el conteo de líneas saltadas—) puede abrir sus PRs, y **L12**
  va después de ellos.

El ciclo de release siguió su curso: la PR `#2240` (v5.0.1) se cerró y hay una `#2245` (v5.0.1) abierta
⇒ las PRs del backlog se abrirán sobre un `main` que se mueve, que es exactamente el motivo por el
que el orden de apertura es **secuencial** (cada slice desde el `main` nuevo) y no apilado.

### 2026-10-22 · D22: el orden real de T2 y T3 en L10 (inversión de premisa en mi spec)

**Lo que encontró el peer, y es cierto**: mi spec de L10 listaba **T2 antes de T3**, y eso está
**al revés**. La entrada `RUSTSEC-2024-0449` es de `unsafe-libyaml`, y mientras el **lock del fuzz**
siga llevando ese crate, la supresión **sigue viva**: no hay nada que purgar (borrarla antes de
re-resolver el lock dejaría el `cargo audit` en rojo, y el guard de S1 —"ningún ignore puede quedar
muerto"— la detectaría). El ignore **muere** cuando el lock del fuzz se re-resuelve (y entonces entra
`unsafe-libyaml-norway`, que es otro crate y no lo cubre). ⇒ **El orden real es T3 (lock del fuzz) y
después T2 (purgar el ignore)**.

La spec **decía la verdad en su texto** (L84 y L103-104 lo explicaban: *"Mientras ese lock exista
con el crate adentro, la supresión sigue viva y borrarla del `deny.toml`…"*) pero **su lista de
tasks contradecía su propio texto**. El peer ejecutó el orden correcto (un solo commit con el lock
re-resuelto **primero** y el ignore purgado después). ⇒ La corrección pendiente es en la **lista de
tasks** del spec de L10, no en su fundamento.

**Estado real medido** (rama `fix-unmaintained-crates`): el lock del fuzz lleva
`unsafe-libyaml-norway` (no `unsafe-libyaml`), el lock del workspace lleva `serde_norway` +
`unsafe-libyaml-norway` y **0** ocurrencias de `serde_yaml`, y `deny.toml` **ya no** tiene la entrada
`RUSTSEC-2024-0449` ⇒ nada que purgar y la cadena queda consistente.

**Corolario de proceso (del peer, y lo adopto)**: *el residuo de un RED puede dejar procesos
colgados y hay que barrerlos al terminar el lote*. Los 3 hijos del RED de `#2066` hubo que
recolectarlos con **SIGKILL** (el SIGTERM no los movió: estaban bloqueados en `do_sys_pause` con la
señal pendiente). La evidencia del RED ya estaba capturada en el test y en el Log, así que no se
pierde nada ⇒ la regla es: **al terminar un lote, barrer los procesos que su RED dejó vivos**.

### 2026-10-22 · Las dos seeds del recipe vigente, publicadas y verificadas

| Seed | Key | Features | Tamaño | Para qué |
|---|---|---|---|---|
| default | `v1-5256d5a74e9f8731` | `""` (features default) | 1,9 G | builds de features default |
| `--all-features` (explícito) | `v1-5045d576a52507ec` | las 18 features calificadas del workspace | 2,1 G | las verificaciones de los lotes (que usan `--all-features`) |

La de `--all-features` **no es un `--all-features` literal**: la recipe no tiene ese vocabulario (el
escape hatch `--` se rechaza a propósito), así que se publica con la **lista explícita** que
resuelve al mismo conjunto de features (`webfang_core/{adaptive-selectors,chromium,console,default,
dev-tracing,loom-model,mcp,persistence}` + `webfang_cli/*` + `webfang_mcp/*`), calculada con
`cargo metadata`. La key lo dice con verdad: `features = "webfang_cli/adaptive-selectors,…"` y el
`workspace_contract` y el `cargo_lock` fijan contra qué árbol es válida. La seed incluye **BoringSSL
compilado** (`btls-sys`, 147 M + 17 M) ⇒ es la parte cara, y esa parte es *independiente de las
features*.

**Las dos cubren la fase de apertura de PRs** (13 slices que se re-aplican sobre el `main` nuevo y se
verifican). **No cubren L9 ni L10**: ambos cambian `Cargo.toml` y el lock ⇒ su `cargo_lock` y su
`workspace_contract` son otros ⇒ hay que **volver a publicar las dos** sobre el árbol post-merge.

**Dos aprendizajes de la operación** (que costaron un诊断 fallido de mi parte):

- **El store hay que leerlo DESPUÉS de que la publicación termine**: comprobé el store a los 49 s,
  la seed todavía no estaba, y concluí que la publicación había fallado. La verdad es que el script
  construye la referencia en frío (con el C++ de BoringSSL) y publica al final; el job reporta el
  fin del shell, no el de la transacción.
- **Un `| tail -40` sobre la salida se come el error**: si la transacción falla, el error se pierde y
  sólo queda un "Terminado" sin explicación. La verificación de una seed es *el store y su
  `manifest.toml`*, no la última línea del log.

### 2026-10-22 · L10 plegado en el libro mayor: qué commit sostiene cada S#

| S# | Qué exige | Commit que lo sostiene | Estado |
|---|---|---|---|
| **S1** | el ignore muerto de `RUSTSEC-2024-0436` se purga y no vuelve | `e02d1829` (el guard) + `905183ac` (su evidencia) | HECHO |
| **S2** | el reemplazo de `serde_yaml` cubre los mismos casos de parseo que el camino actual | **`865652db`** (el swap a `serde_norway` + el borrado de `ScraperError::Yaml`, marcado BREAKING CHANGE) | HECHO |
| **S3** | `cargo deny check` verde sin el error de `unsafe-libyaml`, y el build sigue verde | `1040d312` (el lock del fuzz regenerado **primero** y el ignore purgado **después**, que es el orden que D22 fija) | HECHO |
| **S4** | la entrada de `deny.toml` lleva razón y `# Expires:` | `217c3959` (preexistente, PR #1986) | HECHO |
| **S5**–**S8** | la mitad de `keyring` (el paquete `keyring` fuera del grafo, MSRV, las tres entradas de `deny.toml` de #2030, y que el audit lee el **lock**) | pendiente (T4+) | EN CURSO |

**Evidencia de S2**: el lock del workspace tiene `serde_norway` + `unsafe-libyaml-norway` con **0**
ocurrencias de `serde_yaml`; los 16 tests de `frontmatter.rs` pasan (incluidos los dos pines de PI-7)
y el snapshot `obsidian_byline_artifact_test__empty_byline_fragment_repaired_in_excerpt` queda
**byte-idéntico**, que es el criterio de aceptación del lote.

**Una confirmación del mecanismo de seeds, aportada por el peer**: su worktree de L10 **invalidó su
propia seed** al cambiar el manifiesto (por eso el rebuild post-cambio no fue instantáneo) ⇒ es
exactamente lo que la key ata (`cargo_lock` + `workspace_contract`), y no un fallo. También queda
anotado que **`webfang_benchmark` todavía no declara features** (L9 le añade `chromium`) ⇒ la lista
explícita equivalente a `--all-features` **también** cambia con L9 ⇒ otra razón para publicar las
seeds después de que L9 y L10 entren en `main` (en dos keys: default y la lista explícita).

### 2026-10-22 · Tips de rama corregidos contra el árbol (los tres del peer, medidos)

El peer mandó tres tips "medidos ahora"; **dos iban atras** y el tercero acertó, y además señaló un
número **mío** que estaba mal. La verdad es el árbol (`git log main..HEAD` en cada worktree):

| Rama | Tip real | Commits | Lo que decía el peer |
|---|---|---|---|
| `fix-false-green-tests` (L8) | **`99833e11`** | 15 | `24a819c2` (14) — le falta el commit `99833e11` (*"la fila del inventario nombra la lane de Windows, y no insinúa un gate"*) |
| `fix-unmaintained-crates` (L10) | **`71a2d02e`** | 7 | `1040d312` (5) — le faltan `0e750fab` (el rebase sobre L9 medido) y `71a2d02e` (D20 en el spec) |
| `fix/docs-runbooks-wal` (L14) | **`cc380657`** | 5 | `cc380657` (5) ✓ |

Y el peer **tenía razón en un punto mío**: el `a902229f` que yo tenía anotado como la evidencia de
L14 **no es un commit de esa rama** (la evidencia es `0616c3bf`; `a902229f` venía de una lista suya
anterior que nunca existió en el árbol) ⇒ corregido en el plan y en el espejo.

**Sobre D20 (la clave de la credencial)**: el requisito está **en el spec de L10**, escrito por el
propio peer en `71a2d02e` (la tabla de plataformas, el criterio de aceptación y la corrección de la
premisa del modificador `description`), y en el Log del spec como **L16**. No hay ninguna definición
pendiente: el criterio es *la clave de una entrada `(service, user)` debe ser **byte-idéntica** a
`keyring-rs:{user}@{service}`*; los dos mecanismos admitidos (configurar el store, o lectura
legacy-first) quedan a elección de quien implementa, y el test nombrado es
`the_credential_key_is_byte_identical_to_the_legacy_key`.

### 2026-10-22 · El rebase de L10 sobre L9 es limpio: medido por mí con `merge-tree`

El peer lo midió con `git merge-tree --write-tree` (sin tocar ninguna rama) y yo lo **verifiqué
independientemente**: el merge de tres vías de **L9 + L10 sobre `main`** sale con **rc=0 (cero
conflictos)** y el `Cargo.toml` resultante tiene `serde_norway = "0.9"` y **ni `once_cell` ni
`sys-info` ni `serde_yaml`** ⇒ los tres cambios de dependencia coexisten.

**Por qué importa** (el peer lo señaló bien): la premisa de que *"el lock queda quieto para todo lo
demás"* es justo la que **L10 altera** — L9 quita `once_cell` y `sys-info`, L10 cambia `serde_yaml`
por `serde_norway` **y regenera el lock del fuzz**— y los hunks de `[workspace.dependencies]` están
a **ocho líneas** de distancia. Un rebase por parche puede fallar donde un merge de tres vías no:
la distancia de los hunks, por sí sola, no es un conflicto. Con la medición, el orden firmado
(L9 justo antes de L10) es seguro.

**Consecuencia para las seeds**: como L9 y L10 cambian el manifiesto **y** el lock, las dos seeds
que cubren la fase de apertura hay que publicarlas **después** de que ambos entren en `main` (dos
keys: default y la lista explícita de `--all-features`, que además cambia con L9 porque
`webfang_benchmark` pasa a declarar `chromium`).

**Estado de L10**: tip `6b4bd8ed` (la rama avanzó con el commit del Log que registra la medición del
merge).

### 2026-10-22 · L12: la spec está escrita (worker `mvj2yyuf-e-bowq`) y el ratchet de duplicación está rancio

La spec de L12 (`odd/tasks/duplication-arrange-helpers.md`, 578 líneas + su espejo) está commiteada en
esta rama. Lo que encontró, contra el árbol y no contra el texto de la issue:

**1. El ratchet está rancio, y eso es el fix de verdad.** El baseline commiteado
(`scripts/quality-baselines.json` → `duplicated_lines_rust`) es **9272**; la medición del árbol da
**menos**. Y el número **depende del árbol**, que es lo que invalida cualquier cifra recordada:

| Árbol | Duplicated lines | Clones | Ficheros rust |
|---|---|---|---|
| `b1673234` (el commit que fijó el baseline) | 9272 | 864 | 495 |
| `3a785d8e` (la base de esta rama) | **8889** | 828 | 476 |
| `89ff9d46` (`main` hoy) | 9009 | 839 | 479 |

⇒ El gate hoy pasa con **383 líneas de margen** sobre esta rama (o 263 sobre `main`): la
duplicación bajó desde que se fijó el número y **nadie bajó el ratchet**. Mi medición
(`bash scripts/check_duplication.sh` en esta rama) confirma: `Duplicated lines (rust): 8889
(baseline: 9272) ⇒ OK`.

**El número que se escribe es el del CANDIDATO, medido con el comando del gate** — nunca uno
recordado ni uno calculado a mano (precedente #1892: la estimación aritmética se equivocó en 29
líneas al deslocar las fronteras de los clones). Por eso la spec **no fija el delta** y **no
escribe la estimación** "176 + 817": fija el **criterio** (el valor commiteado debe ser **igual**
a la medición del candidato; el delta, **estrictamente positivo**; la nota con los tres números) y
**sólo baja** (una subida exige decisión del maintainer).

**2. El guard es *fail-open* por vacuidad: pasa sin medir nada.** Medido por el worker con un stub
de `jscpd` en el `PATH` (repo intacto): un informe **sin** `statistics.formats.rust` ⇒ el guard
imprime `Duplicated lines (rust): 0` ⇒ **verde**; un corpus con **cero** ficheros rust ⇒ `0` ⇒
**verde**. El "test nombrado" del lote era, hasta ahora, **un faro sin prueba**. El RED de S4 es
exactamente ese: el guard **reemplazado por `exit 0`** tiene que dar **rojo**, y el meta-guard
acepta **verde con el guard intacto**.

**3. El harness nuevo tiene que ser HERMÉTICO** (stub de `jscpd`), porque el job `repo-guards` de CI
**no tiene el detector real**: un harness que lo exigiera dejaría el job rojo. Consecuencia
declarada: ese harness prueba la **decisión** del guard, no la fidelidad del detector real.

**4. L12 NO cierra `#1757`** ⇒ la PR del lote será **`Closes part of #1757`**. Su AC 1 (self-clones
de producción en `jsonl_exporter.rs` y `checkpoint.rs`) es un alcance distinto del de la fila L12
("helpers de arrange compartidos"), y su AC 2 ("por debajo de 7602") es **incomparable**: 7602
pertenece a la escala del detector anterior (el 2026-10-06, #1867, recalibró 7683 → 13310 al
cambiar de detector). El veredicto por fila de `#1757` queda en el Log de la spec.

**5. Corrección de las premisas de la issue** (medidas, no_OPINADAS): los self-clones de producción
son **118** líneas (96 en `jsonl_exporter.rs` + 22 en `checkpoint.rs`), no ~136; el supuesto
"batch_flow/discovery_phase ~40" **no se confirma** (batch_flow 0, discovery_phase 6); y el "el resto
es boilerplate de tests" es **a medias**: el **43 %** de las 9009 líneas duplicadas (**3864**) está
**en ficheros de test** — es la masa mayor, y ahí ya hay campaña.

### 2026-10-22 · `during_cancel_drain`: el rótulo "imposible" era una afirmación temporal

El peer corrigió el rótulo de la fila del crash point en los **cuatro** lugares que declaraban la
falsedad (el doc-comment de la constante en `crates/webfang_core/src/cli/crash_points.rs`, el
comentario de la suite en `crates/webfang_core/tests/crash_matrix_test.rs`, la tabla de
`docs/persistence-resume.md` y el doc del lote: S4 + Log **L18** nuevo, con **L17** conservando el
registro histórico y su corrección de rótulo, sin reescribirlo). El espejo lleva la NOTA.

**Por qué el rótulo era falso**: decía *"that branch is UNREACHABLE"*, que es una afirmación
**absoluta** ("imposible"). Con **D21-A aprobado** (`#2244`, decisión A: el export entrega su token
de cancelación al `ResumeContext`), el cableado **está aprobado y va a llegar** ⇒ la fila pasa de
*"armada e inalcanzable"* a *"armada y **aún no llega**"*: una fila que **aterriza con el
cableado**, no antes.

**El invariante sigue intacto** (verificado en el diff): la matriz de crash **no tiene fila** para
`during_cancel_drain` ⇒ hoy no hay un falso verde, que es exactamente lo que la suite existe para
prevenir. El texto nuevo no anuncia una fila; explica **por qué** todavía no la hay y **cuándo**
llega.

**Coherencia del lane**: el slice del token de cancelación (`#2244`) es el que **aterriza la fila**
de la matriz ⇒ el lane del exporter queda con un cabo visible: `#2237 → #2238 → L12 → #2244`, y
`#2244` entrega además la fila de `during_cancel_drain` en la matriz de crash.

**El cableado de producción de `during_cancel_drain` no se toca** hasta que el owner lo decida
(el peer lo deja explícito, y el lote lo dice).

### 2026-10-22 · MATRIZ DE APERTURA (el owner abre: "ok abrimos las pr's")

**Las tres reglas que decide la apertura, medidas y no supuestas:**

1. **El `type:*` lo dicta el TÍTULO**, no el label de la issue: `scripts/auto_label_type.sh` (que
   entró en `main` con #2228) mapea el prefijo del título al label (`fix`→`type:bug`,
   `feat`→`type:feature`, `docs`→`type:docs`, `refactor`→`type:refactor`, `chore`/`ci`→`type:chore`,
   `test`→`type:testing`). Los labels que existen: `type:bug`, `type:feature`, `type:docs`,
   `type:refactor`, `type:chore`, `type:testing`, `type:breaking-change`. ⇒ el título se escribe
   primero y el label sale de ahí; **no se agrega un segundo `type:*`**.
2. **Cada issue enlazada tiene `status:approved`**: verificado en las 14 issues de esta matriz
   (las 5 aprobaciones del owner, 2026-10-22, ya aplicadas). Ninguna se auto-aprueba.
3. **La forma del cierre es por fila, no por regla**: `Closes #N` cierra la issue; `Closes part of #N`
   no la cierra y deja el umbrella abierto. Las issues que **ya están cerradas** (`#1852`, `#2162`,
   `#1980`, `#2063`, `#1584`) **no** aparecen en ningún cuerpo de PR.

| Slice | Rama | Título del PR (prefijo → label) | Issues (forma exacta) |
|---|---|---|---|
| **L2a** | `fix/merge-when-green-merged-state-code` | `fix(ci): merge-when-green detecta la PR ya mergeada y la limpieza post-merge no queda encadenada` → `type:bug` | `Closes #1970` · `Closes #1971` · `Closes #1972` |
| **L3a** | `fix/ci-health-w03-classifier` | `fix(ci): el observer del CI Health clasifica por línea de fallo y cierra por subtítulo` → `type:bug` | `Closes #2233` · `Closes part of #1936` |
| **L3b** | `fix/2234-hermetic-args` | `fix(cli): el parseo de los tests es hermético por construcción` → `type:bug` | `Closes #2234` |
| **L4** | `fix/guard-residuals` | `fix(ci): los residuales de guard tienen dueño (parser liberal, filtro de comentarios, semver en base 10)` → `type:bug` | `Closes #2070` (los ítems 1–3 implementados; el ítem 4 = decisión documentada de no hacer nada) |
| **L5** | `fix/ai-attribution-exact` | `fix(ci): la allowlist de atribución exige identidades exactas` → `type:bug` | `Closes #1995` |
| **L6** | `chore/mutation-shard-measurement` | `chore(ci): el conteo de mutantes medido y la matriz rebalanceada a 96 shards` → `type:chore` | `Closes #1844` |
| **L7** | `fix/mcp-surface-truth` | `fix(mcp): la superficie del servidor MCP dice lo que hace` → `type:bug` | `Closes #1815` · `Closes #1837` · `Closes #2193` · `Closes part of #1817` |
| **L8** | `fix/false-green-tests` | `fix(test): los falsos verdes de test mueren con diagnóstico en vez de colgar en verde` → `type:bug` | `Closes #2066` · `Closes #1816` · `Closes #1834` · `Closes part of #1828` |
| **L9** | `fix/build-graph-deps` | `refactor(deps): el grafo deja de llevar lo que no se usa` → `type:refactor` | `Closes #1799` · `Closes #1797` · `Closes #1796` |
| **L10** | `fix/unmaintained-crates` | *pendiente T4+* → el prefijo lo fija el commit que cierre la mitad de keyring | `Closes #1984` · `Closes #2031` *(cuando T4+ confirme la salida de zbus)* |
| **L11** | `fix/obscura-unknown-verdict` | `fix(cli): el veredicto Unknown de obscura nombra el mínimo esperado` → `type:bug` | `Closes #1710` |
| **L12** | `fix/duplication-arrange-helpers` | *pendiente la implementación* | `Closes part of #1757` (**no la cierra**: su AC 1 es self-clones de producción, otro alcance) |
| **L14** | `fix/docs-runbooks-wal` | `docs: el runbook de recuperación de WAL/SQLite y la nota de obscura` → `type:docs` | `Closes #1826` · `Closes part of #1609` |
| **L15** | `fix/miri-ub-lane` | `fix(ci): la lane de UB deja de saltarse árboles de test y tiene un guard de invariantes` → `type:bug` | `Closes #2235` |
| **L13** | — | **DESCARTADO** (decisión del owner: `#1821` se deja, no se toca) | — |

**Los veredictos por fila que la issue exige (no una regla):**

- **#1936** (L3a): su cuerpo es *"Known FP only (W-03)"* ⇒ el lote arregla exactamente su contenido,
  pero la issue la auto-gestiona el CI Health Observer ⇒ `Closes part of` y un comentario con la
  evidencia; que la cierre el observer o el maintainer, no un PR.
- **#1828** (L8): **sigue abierta** hasta que aterrice `#2244` (la fila `during_cancel_drain` de la
  matriz de crash); el resto de su contenido (la fila WAL-kill con `PRAGMA integrity_check`, el
  `cfg(loom)` documentado como aceptado, el catálogo que ya coincide con la realidad) sí aterriza.
- **#1833** (L8): **no aparece**: su propuesta (un scratch-crate que compila una *copia*) está
  rechazada (D19) y espera la decisión O4 del owner ⇒ no hay nada de esa issue en el PR.
- **#1852** (L8): ya cerrada (con `default = ["mcp"]` y el guard nuevos) ⇒ no se vuelve a enlazar.
- **#1817** (L7 + L14): la parte de **las cifras** aterriza en L7 y la del **runbook** en L14 ⇒
  `Closes part of` en los dos; se cierra cuando landing ambos.
- **#2061** (L14): **no se cierra**: la rotación automática está **rechazada** (el gate `published`
  ya corre dentro de `Validate PR metadata`); lo que queda en pie es el gate, no un bot.
- **#1609** (L14): umbrella de 25 filas ⇒ `Closes part of #1609`; cierra con la última de sus tres
  issues vivas (`#2241`, `#2242`, `#2243`), cada una en su propio PR.
- **#2241** (`type:docs`), **#2242** (`type:bug`), **#2243** (`type:chore`): tres tipos distintos ⇒
  **tres PRs**, no uno.
- **#1757** (L12): `Closes part of` ⇒ su AC 2 ("por debajo de 7602") es incomparable con la escala
  vigente (7602 es del detector anterior).

**L2 se parte en dos PRs, y por qué:** la rama del plan arrastra **42 commits** (el tracker ODD del
backlog, ~2400 líneas de markdown en español) mezclados con los **2 commits de código**. Un PR con
todo eso mezcla una corrección de script con un documento de proceso que nadie pidió en esa PR ⇒
ruido para el revisor. ⇒ **L2a** lleva el código y las reglas (`d16ffef2`, `67ff6128`) desde una
rama propia; **el tracker** se abre **al final**, como PR de docs, con su issue propia.

**El orden de `ci.yml`:** abrir es paralelo, **merquear es en serie**. `ci.yml` lo tocan L2, L8, L12,
L15 y L9 ⇒ ese orden relativo es el que gana; el segundo que llega re-aplica su step sobre el
`main` nuevo (el conflicto aparece al mergear, no al abrir) ⇒ conflicto mecánico, con anclas
distintas, resuelto en el momento del merge.

### 2026-10-22 · Tres PRs abiertas (el owner: "ok abrimos las pr's") y un hallazgo transversal

| PR | Slice | Rama | Título | Label | Cierra |
|---|---|---|---|---|---|
| **#2246** | L2a | `fix/merge-when-green-merged-state-code` | `fix(ci): merge-when-green detecta la PR ya mergeada y la limpieza post-merge no queda encadenada` | `type:bug` | #1970, #1971, #1972 |
| **#2247** | L3b | `fix/2234-hermetic-args` | `fix(cli): el parseo de los tests es hermético por construcción` | `type:bug` | #2234 |
| **#2248** | L3a | `fix/ci-health-w03-classifier` | `fix(ci): el observer del CI Health clasifica por línea de fallo y cierra por subtítulo` | `type:bug` | #2233 + `Closes part of #1936` |

Las tres con `Validate PR metadata` **verde** y **un solo `type:*`** (lo pone `scripts/auto_label_type.sh`
desde el prefijo del título; no se agrega un segundo). Cero atribución de IA (verificado sobre los
commits pusheados).

**El hallazgo transversal que broke una de las tres**: el job `Repo guards` corre
`cargo test --manifest-path tools/intra-crate-lint/Cargo.toml --locked`, y ese test compara la
extracción del extractor sobre `crates/webfang_core/src` contra el golden set
`tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt`, que es un **snapshot con números de
línea del árbol real**. El propio doc del test lo dice: *"the set is refreshed in the same commit
that changes the tree"*.

⇒ **Toda PR que toque `crates/webfang_core/src` tiene que refrescar ese fixture** o el job
`Repo guards` falla con exit 101. Medido, no supuesto: `main@89ff9d46` tiene `Repo guards` **verde**
y el fallo aparece **sólo** en la rama que cambia el core. Afecta a L3b (ya lo arreglo) y a L7, L8,
L11, L12 y L15.

El comando (la CLI del tool imprime los sites con el mismo formato que el test):

```bash
cargo run --quiet --locked --manifest-path tools/intra-crate-lint/Cargo.toml -- \
  --root crates/webfang_core/src > tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt
```

**El procedimiento de apertura, medido en estas tres:**

- **Merge de `main` en la rama, no rebase**: el repo no admite reescribir historia y, con squash, el
  commit de merge no llega a `main`; además **conserva los SHAs** que el libro mayor y el peer tienen
  anotados. En las tres ramas la fusión fue limpia (`main` tocado `AGENTS.md` en `be9f00c7` y el
  hunk de L2a cayó en otra sección ⇒ conviven).
- **L2 se parte**: la rama del plan arrastra 42 commits (el tracker) mezclados con 2 commits de
  código ⇒ `L2a` lleva el código desde una rama propia; **el tracker** se abre al final, como PR de
  docs, con su issue propia (nadie lo pidió en el PR del script ⇒ es ruido para el revisor).
- **GraphQL es intermitente**: `gh pr create` para L3a murió con `dial tcp ... i/o timeout` ⇒ creada
  por REST (`gh api -X POST repos/.../pulls`). Y `gh api -f body=@archivo` **no** lee el archivo
  (envía la cadena literal `@/tmp/...`): tiene que ser `-F` ⇒ el body de #2248 salió con la cadena
  literal y se corrigió con un PATCH.

### 2026-10-22 · Las 17 PRs abiertas (el owner: "abrimos las pr's")

| PR | Lote | Rama | Label | Cierra |
|---|---|---|---|---|
| **#2246** | L2a | `fix/merge-when-green-merged-state-code` | `type:bug` | #1970, #1971, #1972 |
| **#2248** | L3a | `fix/ci-health-w03-classifier` | `type:bug` | #2233 + `part of #1936` |
| **#2247** | L3b | `fix/2234-hermetic-args` | `type:bug` | #2234 |
| **#2249** | L4 | `fix/guard-residuals` | `type:bug` | #2070 |
| **#2250** | L5 | `fix/ai-attribution-exact` | `type:bug` | #1995 |
| **#2251** | L6 | `chore/mutation-shard-measurement` | `type:chore` | #1844 |
| **#2257** | L7 | `fix/mcp-surface-truth` | `type:bug` | #1815, #1837, #2193 + `part of #1817` |
| **#2252** | L8 | `fix/false-green-tests` | `type:bug` | #2066, #1816, #1834 + `part of #1828` |
| **#2260** | L12 | `fix/duplication-arrange-helpers` | `type:chore` | `part of #1757` |
| **#2253** | L11 | `fix/obscura-unknown-verdict` | `type:bug` | #1710 |
| **#2254** | L14 | `fix/docs-runbooks-wal` | `type:docs` | #1826 + `part of #1609` |
| **#2255** | L15 | `fix/miri-ub-lane` | `type:bug` | #2235 |
| **#2256** | L9 | `fix/build-graph-deps` | `type:refactor` | #1799, #1797, #1796 |
| **#2258** | L10 | `fix/unmaintained-crates` | `type:chore` | #1984 |
| **#2261** | #2237 | `fix/exporter-tail-read` | `type:bug` | #2237 |
| **#2259** | #2238 | `feat/mcp-export-skipped-count` | `type:chore` | #2238 |
| **#2262** | #2243 | `fix/prune-dead-store-seam` | `type:chore` | #2243 |

**Las 24 issues enlazadas, verificadas una por una por API: existen TODAS y TODAS llevan
`status:approved`** ⇒ ninguna PR queda bloqueada por el gate de "Require status:approved on linked
issues", que además exige la aprobación **de cada** número enlazado (incluidos los `part of`).

**Dos correcciones al libro mayor, con su causa:**

- **El `type:*` lo deriva el TÍTULO, no el label de la issue** (`scripts/auto_label_type.sh`, que entró
  en `main` con #2228): `fix`→`type:bug`, `chore`→`type:chore`, `docs`→`type:docs`,
  `test`→`type:testing`, `feat`→`type:feature`, `refactor`→`type:refactor`, `!`→`type:breaking-change`.
  El gate exige **una sola** `type:*`, así que el prefijo del título y la label tienen que coincidir
  siempre. La convención del repo (medida en sus PRs mergeadas: #2230 `fix`→`type:bug`, #2223
  `chore`→`type:chore`, #2216 `fix(test)`→`type:bug`) es que **la label sigue al tipo del commit**,
  no al label de la issue enlazada.
- **`#1852` no aparece en ningún cuerpo**: está **CERRADA** (lo cerró el lote L8 con `default =
  ["mcp"]` y el guard nuevo) ⇒ enlazarla sería un no-op. `#1797`, `#1796`, `#2031` y `#2235` **sí**
  existen (y están aprobadas); una medición única contra una API caída las dio por inexistentes, y se
  corrigió con reintentos.

**El golden set del lint intra-crate** (`tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt`)
es un **snapshot con números de línea** de `crates/webfang_core/src` ⇒ toda PR que toca el core lo
deja rancio y el job `Repo guards` falla con exit 101. Refrescado y commiteado en L3b, L7, L11, L14
y L9. El comando (la CLI del tool imprime los sites con el mismo formato que el test):

```bash
$BIN --root crates/webfang_core/src > tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt
```

⇒ **es un segundo recurso serializado**, junto a `ci.yml`: en el orden de merge hay que refrescarlo
en cada PR que toque el core.

### 2026-10-22 · #2252 y #2258 arreglados: las tres causas raíz, cada una con su SHA

**#2258 (L10) — DOS causas, dos commits:**

1. `ba6bb9c8`: el **golden set** del lint intra-crate (38 sitios desplazados por los 6 archivos de
   `core/src` del swap de keyring).
2. `de0e160b`: la segunda violación, `no-harness: check_no_dead_ignores.sh is wired but has no
   scripts/test_*.sh` — el guard queda cableado **por indirección** (lo invoca
   `check_advisory_policy.sh:671`) y el meta-guard sigue ese nivel ⇒ necesita su harness. Fix por la
   vía declarada: **`scripts/test_no_dead_ignores.sh`** (marcador `# NEGATION:` real — el ignore vivo
   **no rescata** al muerto; ROJO con el guard sustituido por `exit 0`) **cableado** con un step nuevo
   en `Repo guards` ("Dead-ignores guard semantics harness"), hermético con stub de `cargo audit`.
   Verificado: 8 ok/0 fail; con el guard mutado rc=1; meta-guard `OK: 19 wired guard(s) covered
   (mutation: on)`; shellcheck sin hallazgos.

⇒ **Consecuencia para el mapa de conflictos: `#2258` ahora también toca `ci.yml`** ⇒ el grupo pasa a
ser #2246, #2252, #2260, #2255, #2256 **y #2258**.

**#2252 (L8) — TRES causas, tres commits del worker (`659fbd0c..113a4398`):**

1. `6cbcef84` — la raíz del **ENOENT**: el glob `--test '*'` hace que cargo **nombre** cada target, y
   `adaptive_selector_integration` exige la feature `adaptive-selectors` ⇒ **error duro 101 antes de
   arrancar** ⇒ nextest no escribe resultados. Fix: las lanes seleccionan por KIND (`--tests`). RED
   reproducido local con exit real.
2. `c0cf9d17` — los tres marcadores `# NEGATION:` del harness de empty-test-targets (la única
   violación del meta-guard; **no había** entrada de baseline: nada que shiftear).
3. `113a4398` — el crash point del SIGKILL descartado **salta con motivo escrito**: sonda el entorno
   primero; si el namespace no existe (el runner de GitHub no concede user namespaces: `write failed
   /proc/self/uid_map`), SKIP con remediation; si existe, las **tres aserciones siguen vivas**.
   `docs/persistence-resume.md` nombra quién verifica esa fila y las otras 26 filas siguen en la
   lane ordinaria.

**La epidemia se cerró con la lección del peer**: lo que yo medía era el **worktree** (con el
borrador del worker ya dentro) y el CI medía el **blob del SHA** (`659fbd0c`, sin marcadores).
La prueba no discutible es `git show <sha>:<path>` — nunca el fichero del worktree.

**El flake** `js_strategy_test::non2xx_browser_page_is_honest_error_exit_69` queda **fuera**: es
preexistente y ajeno al diff (el cambio del harness compartido es `fn` → `pub(crate) fn`).
Defecto separado si el owner lo quiere, con su issue propia.

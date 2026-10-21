# Guard de drift doc↔código para el doc normativo de providers (#2136)

Objetivo: que `docs/architecture/ai-providers-design.md` — spec normativa citada por 13
archivos de código y hoy fuera de todo gate — no pueda mentir sin que algo se entere.
Rama: `main` (sin commit: la entrega es del owner). Estrategia: un solo PR, **advisory primero**.
Runner: bash puro (harness offline con fixtures en `mktemp -d`), sin cargo, sin red.

El bug que justifica la pieza: `AuthSource::resolve()` documentada como
`Result<ApiKey, AuthError>` cuando la real es `Result<Option<ApiKey>, AuthError>`
(`crates/webfang_core/src/domain/auth_source.rs:118`). El doc se corrigió a mano en
#2027/#2137; lo que faltaba era el mecanismo quenotice el próximo.

---

## Specs

**S1.** El doc normativo se audita contra el código, no contra su propia prosa. Cada cita
`archivo:línea` del doc queda registrada en un manifiesto embebido y verificada contra el
archivo real: la línea citada tiene que decir lo que el doc afirma.

**S2.** El manifiesto vive **dentro** del doc (`docs/architecture/ai-providers-design.md`),
en un bloque delimitado por `<!-- claims:begin -->` / `<!-- claims:end -->` con un fence de
info-string `claims`. El doc es la fuente de verdad de sus propias afirmaciones; el guard
sólo lo lee. Formato de cada línea, campos separados por `|`:

```text
claim  | <archivo> | <línea> | <subcadena esperada>
shape  | <archivo> | <línea> | <declaración> | <miembros separados por coma>
impl   | <archivo> | <línea> | <declaración>
absent | <archivo|*> | - | <símbolo>
```

**S3.** El guard **falla cerrado** por defecto (exit 1 ante drift, exit 0 limpio) y expone
`--advisory` (reporta lo mismo, exit 0) y `--root <dir>` (para fixtures).

**S4.** Checks del guard (todos fail-closed):

- **C1 manifest**: el bloque existe, cada línea parsea en sus campos, ningún campo vacío,
  todo archivo existe y toda línea está dentro del archivo.
- **C2 claim**: la línea citada (trimmed) contiene la subcadena esperada — coincidencia
  literal, no regex.
- **C3 shape**: el tipo declarado en la línea citada tiene **exactamente** los miembros
  esperados (variantes para `enum`, campos para `struct`), en ambos sentidos: falta un
  miembro en el doc y sobra en el código es drift igual que al revés.
- **C4 impl**: el fence del doc que contiene la declaración tiene todas sus líneas de
  código (no comentarios, no vacías) presentes en el archivo citado, con whitespace
  normalizado (colapsado, sin coma antes de `)`/`]`).
- **C5 absent**: el símbolo no aparece en el archivo (o en todo `crates/` si el archivo es `*`).
- **C6 cobertura**: toda cita `archivo:línea` de la prosa del doc tiene que estar
  registrada en el manifiesto. Una cita sin claim verificable es drift — es lo que impide
  agregar una afirmación normativa nueva sin registrar su chequeo.
- **C7 referencias §**: todo `§N` / `§Na` del doc y del código que cita el doc resuelve a
  una sección numerada (`### N.`) o a un ítem letra (`- **(a)**`) del doc. Referencia rota
  es drift.

**S5.** Normalización de rutas: una cita se resuelve, en orden, contra la raíz del repo,
contra `crates/webfang_core/src/`, y por basename único bajo `crates/`. Si no resuelve o es
ambigua → drift (no se adivina).

**S6.** El harness (`scripts/test_ai_providers_doc.sh`) es offline, con fixtures en
`mktemp -d`, sin cargo y sin red, y cubre **un caso por check**: árbol limpio → 0;
`claim` con subcadena incorrecta → 1; `claim` con línea desfasada → 1; `shape` con miembro
faltante → 1; `shape` con miembro de más → 1; `impl` con firma que no está en el código → 1;
`absent` con símbolo presente → 1; cita de prosa sin registrar → 1; referencia `§` rota → 1;
manifiesto ausente → 1; `--advisory` sobre árbol con drift → exit 0 con hallazgos
impresos.

**S7.** Cableado. La fase del guard se declara **exactamente una vez** en el paso del job
`repo-guards` de `.github/workflows/ci.yml`: `continue-on-error: true` (advisory) **o** el
marcador `# guard-required` (required). Las dos a la vez es una promoción falsa —el paso
seguiría siendo advisory mientras el harness lo certificaría como bloqueante— y ninguna
es un paso sin fase declarada. **Fase actual: required** (decidida por el owner; L19). La
promoción desde advisory fue **reemplazar** el flag por el marcador: una edición, y la
fase queda legible por máquina en el propio archivo.

El **espejo local** (`scripts/ci_fast_gate.sh`, las tres lanes) tiene que coincidir con la
fase: required ⇒ el guard bloquea la lane local; advisory ⇒ corre con `--advisory` y la
lane nunca se pone roja. Un guard que bloquea en CI y no localmente miente: el dev ve la
lane verde y se encuentra con el CI rojo. El harness rechaza las dos incoherencias.

Las aserciones de wiring comprueban, además, que el guard esté invocado desde `ci.yml` y
desde el fast gate, y que su harness esté cableado en el mismo job.

**S8.** El doc se corrige donde hoy miente. El guard, al construirse, tiene que encontrar
las citas desfasadas que quedan vivas (ej. `providers.rs:83` y `providers.rs:251`, que ya
no apuntan a lo que el doc afirma) y el doc queda alineado con el código antes de que el
guard se encienda.

**S9.** El harness tiene que probar el **fallo cerrado** de cada check, no sólo el camino
feliz: un guard que dejó de reconocer drift es indistinguible de no tener guard (el mismo
argumento quejustifica el harness de los pines).

---

## Tasks

| ID | S# | Ruta | Estado | Evidencia |
| :--- | :--- | :--- | :--- | :--- |
| T1 | S1,S2,S4,S5,S8 | delegada (worker) | [x] | manifiesto embebido (44 entradas: 30 claim, 8 shape, 5 impl, 1 absent) + 9 citas corregidas + `scripts/check_ai_providers_doc.sh` (22 KB) |
| T2 | S3,S6,S9 | delegada (worker) | [x] | `scripts/test_ai_providers_doc.sh`: 17 casos de semántica + 3 de wiring = 20/20 verde |
| T3 | S7 | delegada (worker) | [x] | paso advisory en `ci.yml` (`continue-on-error: true`) + harness bloqueante + `run_advisory_guard` en las 3 lanes del fast gate |
| T4 | S6,S9 | delegada (worker) | [x] | corrección: harness 20 → 28 casos, C1b y C4a aislados, detección de fase XOR |

---

## Log

**L1 (2026-10-20, owner, verbatim):**

> Queda una sola cosa abierta, y es la que importa a futuro: la #2136 — el doc normativo de
> providers está fuera del libro mdBook, así que ningún gate lo lee. Fue la causa raíz de que
> una firma que no compilara sobreviviera meses. Recomiendo el guard de drift doc↔código
> cableado en CI (mismo patrón que el de los pines de versión), empezando en modo advisory
> antes de volverlo required. Cuando quieras lo arrancamos.

**L2 (2026-10-20, owner, verbatim):**

> siempre utiliza las potentes tools de *codedb* y codegraph

**L3 (2026-10-20, diseño):** el guard se modela como el de los pines de versión
(`scripts/check_docs_version_pins.sh`): documento con literales + fuente de verdad + check
fail-closed. La diferencia: ahí la fuente de verdad es `[workspace.package] version`; acá es
el árbol de código, y el "literal" del doc es la cita `archivo:línea` más su contenido
esperado. La regla que cierra la puerta —toda cita de prosa tiene que estar registrada—
es la contraparte exacta de "el version pin tiene que declarar la versión del workspace o
estar marcado como historia".

**L4 (2026-10-20, evidencia):** el contraste manual de #2137 (29/29 OK) se hizo con un
script de un solo uso; ese script es exactamente lo que este task vuelve permanente y
fail-closed.

**L5 (2026-10-20, riesgo):** ítem 5 (delivery/entorno: el guard es un gate de merge). Fase 1
advisory ⇒ un falso positivo no bloquea PRs; la promoción a required exige que el harness
pruebe el fallo cerrado de cada check.

**L6 (2026-10-20, limitación conocida de v1):** C3 compara conjuntos de miembros, no
tipos. Un cambio en el *tipo* de un campo que el doc declare con la firma textual correcta
(por ejemplo `embedding_dim: Option<usize>` → `Option<u32>`) lo detecta C4 (la línea del
código no contiene la subcadena esperada) pero no un cambio de tipo dentro de un fence que
sólo se compare como conjunto de miembros. Cerrado en v1 por C2/C4 para las citas de línea;
el resto es deuda de v1.1, anotada acá a propósito.

**L7 (2026-10-20, T1 — el guard encuentra drift REAL, no sólo de fixture):** el worker
revertió las 9 citas corregidas sobre un scratch fuera del worktree: 10 hallazgos de C6
(cobertura) — 9 citas revertidas más el extremo del rango `118-126`, que es un hallazgo
propio —, exit 1. Y apuntando el manifiesto a la línea equivocada: C2 (`la línea no
contiene 'pub base_url: Url,' (contenido: 'pub display_name: String,')`) + C3 (miembro
extra). Las tres capas atacan el bug original, no una versión de él.

**L8 (2026-10-20, T1 — dos hints del owner resultaron falsos, y el worker los verificó en
vez de seguirlos):** `llm_wire.rs:97` **ya era correcto** (es
`registry.resolve(id, Capability::Embedding)`) y `auth_source.rs:6` **ya era correcto** (es
`- Sin variante \`Inline\` en v1 (…)`). La lección operativa: un hint de línea es una
hipótesis, no evidencia; el guard la resuelve con la cita.

**L9 (2026-10-20, T2/T3 — el meta-guard del repo es la restricción que gobierna el
cableado):** `scripts/test_guard_coverage.sh` (#2056) exige que todo `check_*.sh`
cableado tenga harness, que el harness esté cableado en un workflow, que lleve un marcador
`# NEGATION:` y —esto no es negociable— que **se ponga rojo cuando el guard se reemplaza
por `exit 0`**. Por eso el harness invoca el guard por su ruta real y no por una copia del
fixture. Las categorías `no-mutation` y `broken-harness` no son baselinables, y `no-marker`
sí lo sería: baselinarlo sería comprar inmunidad permanente en un archivo que sólo puede
encogerse. **No se tocó `scripts/guard_coverage_baseline.txt`.**

Estado verificado: `bash scripts/test_guard_coverage.sh` → `OK: 18 wired guard(s) covered
(mutation: on)`, y el guard quedó intacto después de la mutación.

**L10 (2026-10-20, deuda de v1, anotada para no perderla):** los fences de §1
(`AuthSource`) y §2 (`AuthError`) no tienen claim `impl` porque el doc los renderiza en una
línea y el código los despliega en varias (containment literal fallaría); quedan cubiertos
por `shape` (variantes exactas) + `claim` por línea. Un cambio de *tipo* dentro de esas
variantes no lo detecta nadie: es la deuda de L6, concentrada en esos dos tipos.

**L11 (2026-10-20, entorno — la lane de cargo NO se ejecutó):** el árbol de trabajo tiene
cambios Rust en curso de **otra sesión en el mismo checkout** (issue #2106, bloques
compartidos `show`/`doctor` — `crates/webfang_core/src/cli/commands/{blocks,show,doctor}.rs`).
No son de esta tarea y no se tocaron. Consecuencia: el clasificador de paths marca
`code=true` y `ci_fast_gate.sh` habría corrido la lane de cargo sobre trabajo ajeno en
curso (lento, y potencialmente rojo por razones ajenas). Se verificó el ruteo con
`--dry-run` (`lane inputs: docs_only=false ci_only=false code=true`) más la ejecución
directa del guard, del harness y del meta-guard. La lane de cargo queda para el PR, cuando
el árbol esté limpio.

**L12 (2026-10-20, promoción — una sola edición mecánica):** fase 2 = reemplazar
`continue-on-error: true` por el marcador `# guard-required` en el paso de `ci.yml`. El
criterio de promoción ya está cumplido (el harness prueba el fallo cerrado de cada check y
el meta-guard prueba la no-vacuidad). La decisión es del owner. → **ejecutada en L19**.

**L19 (2026-10-20, promoción ejecutada — el owner decidió "ahora", sin esperar el mes de
datos de falsos positivos):** el paso de `ci.yml` pasó a required (marcador
`# guard-required`, sin `continue-on-error`), las tres lanes del fast gate pasaron a
bloqueante con la forma de la casa (`run_guard … bash scripts/check_ai_providers_doc.sh`,
igual que el guard de pines), y el helper `run_advisory_guard` —que quedaba sin uso— se
eliminó.

El harness ganó el **invariante fase ↔ espejo local**: la fase del CI y el modo del
espejo son dos caras del mismo estado, y el harness rechaza las dos incoherencias (fase
required con espejo advisory, y fase advisory con espejo bloqueante). Eso obligó a
cambiar la mecánica de las variantes: `phase_variant` (que muta la fase en `ci.yml`) tiene
ahora su gemelo `mirror_variant` (que muta el modo en el fast gate), porque derivar una
cara sin la otra produce un rechazo por el motivo equivocado —y un caso rojo que pasa por
otro camino no probó lo que dice probar. El harness pasó de 45 a **49 casos**.

Un detalle que salió del trabajo y que conviene no olvidar: el bloque efectivo del paso,
para el detector de fase, incluye el comentario que **sigue** al paso, no el que lo
precede. La prosa de la fase tiene que esquivar las dos formas regex (`continue-on-error:
true` y `# guard-required`) en posición de inicio de línea, o el harness la cuenta como
fase.

**L20 (2026-10-20, el CI rojo de #2146 NO es de este cambio):** `Security Audit` falla con
`RUSTSEC-2026-0383` (webpki-root-certs 1.0.9 vía `wreq`; la raíz emSign ECC G3, cuya clave
privada es de dominio público). El `Cargo.lock` de la rama es **idéntico** al de `main`, y
un run disparado sobre `main` (0ccedd9d, la base exacta) falla igual. El advisory tiene
fix real (`cargo update -p webpki-root-certs` → 1.0.10 elimina la raíz), así que
corresponde un PR `fix(deps)` separado y no una entrada de ignore en `deny.toml` — el
precedente es #1984.

**L13 (2026-10-20, T4 — la verificación independiente y lo que encontró):** verificador
independiente, 9/9 specs cumplidas, cero bloqueantes caused-by-change. Sus 7 advisories
Apuntaron a un problema de una sola clase: *checks que pueden morir en silencio porque
ningún caso del harness los aísla*. C1b (rango de línea) y C4a ("la línea citada contiene
la declaración") se ejercitaban pero no se aislaban: al neuterarlos, el harness quedaba
verde porque C3 y C4b capturaban el mismo drift. Corregido con dos casos nuevos que
asertan **quién** reporta el hallazgo (por exit code no se puede: dos checks sobre el
mismo drift son indistinguibles). El harness pasó de 20 a 28 casos.

También encontró dos recetas de promoción contradictorias en el mismo documento (la spec
decía "quitar el flag", el harness exigía el marcador) y que la detección de fase era un
OR: `continue-on-error: true` + `# guard-required` juntos pasaban el harness con el paso
**todavía advisory** — una promoción falsa certificada como promoted. Ahora es XOR, con
las dos fases ancladas a línea efectiva (la prosa del propio `ci.yml` mencionaba las dos
y el grep por subcadena leía esa prosa como declaración de fase).

**L14 (2026-10-20, estado de entrega — el guard está en rojo y NO es culpa del cambio):**
`bash scripts/check_ai_providers_doc.sh` da exit 1 sobre el árbol real con 3 hallazgos
(C4 en `:118`, C2 en `:151`, C2 en `:125`) porque **otra sesión** insertó 6 líneas en
blanco en `crates/webfang_core/src/domain/auth_source.rs` (mtime 21:27, issue #2106 en
curso), lo que corrió todas las citas de ese archivo. Es la primera demostración real de
que el guard funciona sobre el árbol de verdad, no sobre un fixture. No se tocó ese
archivo: no es de esta tarea, y `cargo fmt --all` lo resuelve en la sesión que lo posea.
Cuando esas líneas desaparezcan, las citas del doc vuelven a ser correctas sin tocar el doc.

**L15 (2026-10-20, evidencia de cierre):** `bash scripts/test_ai_providers_doc.sh` → 28/28
verde · `bash scripts/test_guard_coverage.sh` → `OK: 18 wired guard(s) covered (mutation:
on)` · `shellcheck` → exit 0 · la lane de cargo no se ejecutó (L11).

**L16 (2026-10-20, revisión nativa del commit `3610ea5d`):** la lente de **R1 (riesgo)**
completó con `inspection.status: completed` sobre los 6 paths y **cero bloqueantes**: un
WARNING y dos SUGGESTION, los tres reales y confirmados leyendo el código.

- **R1 (WARNING)** — `resolve_path` probaba la ruta tal cual contra `$ROOT`, sin
  rechazar `..`: una cita del doc podía resolver a un archivo fuera del repo y el guard
  imprimía su contenido en el log de CI. Hoy no hay fuga activa (el manifiesto sólo cita
  fuentes internas y el paso es advisory), pero el guard no confinaba lo que leía.
- **R1 (SUGGESTION)** — la clave de la caché de archivos normalizados traducía `/` y `.`
  al mismo `_`: dos rutas distintas podían compartir entrada, y el containment de C4
  podía satisfacerse con el contenido de otro archivo — dirección de falso-verde.
- **R1 (SUGGESTION)** — `--advisory` no cubría las cuatro salidas estructurales
  tempranas (doc ausente, bloque ausente, fence ausente, manifiesto vacío): el contrato
  "nunca falla" lo sostenía el `|| true` del llamador, no el script.

Corregidos en un commit encima (la historia pusheada no se reescribe): `confina` (léxico)
+ `canon_under_root` (`realpath -m`, que también atrapa un symlink que apunta afuera),
clave de caché inyectiva (`norm.$N`), y `abort_finding` para que **ningún** camino de
salida devuelva 1 en modo advisory. El harness pasó de 28 a **45 casos**, y el neutering
de cada corrección pone rojo su caso y sólo el suyo.

**L17 (2026-10-20, un descubrimiento del worker que va más allá del hallazgo):** la caché
de archivos normalizados era **código muerto**. `nf="$(norm_file "$res")"` corre en un
subshell, así que `NORM_KEYS+=` nunca llegaba al shell padre: el lookup era siempre
vacío y cada llamada reescribía el archivo. La colisión de clave estaba enmascarada por
el orden escritura→lectura. La corrección la volvió viva (`NF_PATH`, sin subshell) — y
con la caché viva, la clave no inyectiva se vuelve un falso verde **alcanzable**, que es
lo que el caso 18 ahora demuestra. Lección: un `$(...)` que envuelve una función bash
corre en un subshell, y toda mutación de arrays globales se pierde.

**L18 (2026-10-20, dos lentes no pudieron correr — defecto de runtime, no del cambio):**
las lentes de legibilidad y fiabilidad devolvieron `opencode_task_output_empty` dos
veces cada una (Task del reviewer completa sin resultado capturable). Es la familia
documentada en `Gentleman-Programming/gentle-ai` #1867. Causa probable: el modelo de las
lentes no tiene `limit` en el config de opencode, así que OpenCode omite
`maxOutputTokens` y el JSON largo del reviewer se trunca. El fix es
`provider.<id>.models.<model>.limit.output` **en el config del runtime, no en el review**,
y opencode no recarga config en caliente: requiere reiniciar el editor. Ambas lentes
quedaron declaradas `unachievable` con `capture-unachievable` (motivo
`opencode_task_output_empty`) y la transacción quedó en `stop`, sin recibo de aprobación:
el resultado de R1 es evidencia, no autorización, y la entrega sigue bajo la política
ordinaria del repo.

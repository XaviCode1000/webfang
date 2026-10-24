# Duplicación: los helpers de arrange compartidos (el harness que los tests MCP evitan y el arrange de sitemap que vive en un solo binario de test)

Cierra el lote **L12** del plan de backlog: el ratchet de duplicación (`scripts/check_duplication.sh`,
issue #516) baja por debajo de su baseline porque los **helpers de arrange** —el bloque "sirvo,
parseo, actúo" y el "levanto el servidor de test"— están escritos **una vez por fichero de test**
en vez de una vez por crate, y porque el guard que decide el ratchet confunde *"no medí nada"* con
*"no hay duplicación"*. Rama: `fix/duplication-arrange-helpers`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-duplication-arrange-helpers`.
Origen: #1757 (la fila L12 del plan). Test nombrado del lote: `scripts/check_duplication.sh`.

**Este documento no cierra #1757**: la AC 1 de la issue es un alcance distinto (self-clones de
producción) y queda medida y documentada en el Log (P3, P4, P9). El lote implementa la fila L12
del plan.

**Convención de citas**: los bloques `>` son citas **literales** (de la issue #1757, del plan
madre o del código del repo). Las referencias al código van **por path + símbolo**, nunca por
número de línea.

---

## Specs

**Criterios de aceptación de #1757 (verbatim):**

> - [ ] `jsonl_exporter.rs` y `checkpoint.rs` sin self-clones de ≥50 tokens (jscpd `--min-tokens 50`).
> - [ ] Baseline de `scripts/quality-baselines.json` descendido por debajo de 7602 (el valor real de main pre-batch).
> - [ ] `bash scripts/check_duplication.sh` verde y `cargo nextest run` del área afectada verde.

Y el contexto que los acompaña (verbatim):

> El ratchet de duplicación (`scripts/check_duplication.sh`, issue #516) quedó en 7683 líneas
> duplicadas tras el batch merge. Cada PR pasaba individualmente contra el baseline 8098; la
> combinación con el descenso autorizado de #1740 (8098 → 7577) dejó el árbol combinado en +106
> sobre el baseline. Se autorizó el ajuste upward 7577 → 7683 (cláusula (b) del policy: combined
> bug-fix rollup, precedentes #647/#656/#678/#806) solo para poder aterrizar el rollup; el
> compromiso es deduplicar y devolver el ratchet a su nivel real.

**Qué AC responde cada S#** (el alcance del lote es el de la fila L12 del plan, no el de la AC 1;
el motivo está medido y documentado en el Log, P9): **S1** y **S2** son el "deduplicar … y
devolver el ratchet a su nivel real" sobre el corpus que la issue llama "boilerplate de tests"
(que es, medido, el 43 % de la duplicación del árbol); **S3** es la AC 2 (con el número correcto
para el detector vigente, P2); **S4** es la AC 3 (el guard que la nombra tiene que poder
decidir, y hoy pasa por vacuidad).

### S1 · #1757 — el arrange de parseo de sitemap se comparte entre los tres binarios de test que lo necesitan

**El defecto, medido** (jscpd 5.4.0, `--min-tokens 50 --format rust --mode mild`, sobre
`main@89ff9d46`): el helper de arrange **ya existe** y vive en **un solo** binario de test.

- `crates/webfang_core/tests/sitemap_integration.rs` — `parser()`, `serve_and_parse()` y
  `serve_many_and_parse()` (extraídos por #1889, "slice-14").
- `crates/webfang_core/tests/sitemap_behavioral.rs` — **reescribe el mismo arrange** inline en sus
  ocho tests: `EnvGuard::entry_guard_off()` + `MockServer::start()` + `Mock::given(...)` + mount +
  `SitemapParser::new().unwrap()` + `parse_from_url(&url, &CorrelationId::new())`.
- `crates/webfang_core/tests/orchestrator_unit_tests.rs` — **idem**, cinco veces, con los mismos
  nombres de test (`sitemap_deduplicates_urls` aparece en los dos ficheros con el mismo cuerpo).

Duplicación medida entre esos ficheros: **159 líneas en 9 clones**
(`orchestrator_unit_tests.rs` ↔ `sitemap_behavioral.rs`) + **17 líneas en 2 clones**
(`sitemap_behavioral.rs` ↔ `sitemap_integration.rs`) = **176 líneas en 11 clones**. A eso se suman
**60 líneas en 4 clones** de self-clone dentro de `sitemap_integration.rs` (el arrange de
sitemap-index, repetido en los cuatro tests de índice: `test_parse_sitemap_index_recurses`,
`test_sitemap_index_partial_failure_continues`, `test_sitemap_index_all_children_failed`,
`test_sitemap_index_mixed_valid_and_loop`).

**El fix por raíz**: el arrange es **una sola cosa** (servir un cuerpo XML en un mock y parsearlo)
y su copia está en tres binarios porque **el helper nació dentro de un binario de test**, donde no
puede ser importado por otro. El fix no es "borrar líneas repetidas": es **mover el arrange a un
módulo compartido del crate** y dejar que los tres binarios lo usen.

**Decisión de diseño (del plan y del repo, no de la issue)**: el módulo compartido vive en
`crates/webfang_core/tests/common/`, que es **el** módulo compartido de los tests de integración
del crate (`#[path = "common/mod.rs"] mod common;`) y ya expone submódulos públicos
(`cli_harness`, `fixture_server`, `fixtures`, `mock_http`). **No** va en `webfang_test_utils`: ese
crate sólo depende de `regex` y `webfang_core` (verificado en su `Cargo.toml`), así que un helper
que necesite `wiremock`/`tokio` exigiría **dependencias nuevas** ⇒ prohibido por el scope de este
lote (`AGENTS.md`: *"Ask first: Adding or removing dependencies"*).

**La forma del `mod`Import (detalle que no es libre)**: en `webfang_core` la convención es
`#[path = "common/mod.rs"] mod common;` (20 de sus 24 targets la escriben; ver
`crates/webfang_core/tests/common/mod.rs`), y el helper nuevo se alcanza como
`common::sitemap_arrange::…`. En `webfang_mcp` la convención es **bare** `mod common;` (sus 15
targets), y el propio `crates/webfang_mcp/tests/common/mod.rs` avisa de que *"the two crates'
harnesses are not interchangeable, so do not copy one crate's header shape into the other"*. Cada
lote usa la forma de **su** crate.

**Código a cambiar:**

- `crates/webfang_core/tests/common/sitemap_arrange.rs` (nuevo): los tres helpers con **los mismos
  nombres y las mismas firmas** que hoy tiene `sitemap_integration.rs` — `parser()`,
  `serve_and_parse(mock, mount_path, response, entry_path)`,
  `serve_many_and_parse(mock, mounts, entry_path)` — y además el helper del arrange de
  sitemap-index, extraído de los cuatro tests de índice (una función que recibe la lista de
  `(mount_path, ResponseTemplate)` del índice y de sus hijos y devuelve las URLs parseadas). El
  cuerpo de `serve_and_parse` **no se reescribe**: es el mismo texto que hoy pasa el test, movido
  de sitio.
- `crates/webfang_core/tests/common/mod.rs`: `pub mod sitemap_arrange;` (una línea, el mismo
  patrón que los otros cuatro submódulos).
- `crates/webfang_core/tests/sitemap_integration.rs`: se borran los helpers locales `parser()`,
  `serve_and_parse()` y `serve_many_and_parse()` y se usan los del módulo compartido. Los cuatro
  tests de índice pasan a usar el helper de índice.
- `crates/webfang_core/tests/sitemap_behavioral.rs`: sus ocho tests pasan a usar el arrange
  compartido. **Las aserciones no se tocan** (mismo número de URLs, mismos mensajes, mismo
  presupuesto de tiempo).
- `crates/webfang_core/tests/orchestrator_unit_tests.rs`: sus cinco tests pasan a usar el arrange
  compartido. El **único** test que hoy **no** desactiva el entry guard
  (`sitemap_malformed_xml_returns_error`, que no llama a `EnvGuard::entry_guard_off()`)
  **conserva esa diferencia**: el helper no activa el guard por el llamador, es el llamador
  quien lo pide.

**Restricción dura (Criterio de aceptación del lote)**: la extracción es **conservadora de
comportamiento**: cero cambios de aserción, cero cambios de snapshot, cero cambios de producción.
El nombre y el cuerpo de los tests **no** cambian (son el vocabulario de la suite); lo que cambia es
de dónde sale el arrange.

**Prueba nombrada:** el lote reusa la del plan (`scripts/check_duplication.sh`); para S1 en
particular, la prueba es que **los tres binarios de test siguen verdes** y que el clon medido
desaparece. No hay un test de comportamiento nuevo: el arrange es un **detalle de test**, y su
prueba es que los tests que lo usan siguen assertando lo mismo.

### S2 · #1757 — el harness compartido de `webfang_mcp` se usa en vez de esquivarse

**El defecto, literal, del propio código** (esta cita es del repo, no de la issue):

> Imported by name, not `use common::*`: this file keeps its own local
> server harness (`start_test_server`, the session-seeded
> `start_seeded_server`, `init_ssrf_disabled`, `redact_path`, `RelTempDir`)
> — server-bootstrap dedup belongs to the arrange-helpers slice, not this
> one.

(`crates/webfang_mcp/tests/mcp_behavioral_test.rs`, cabecera del fichero.)

⇒ **El propio repo nombra el lote pendiente**: "the arrange-helpers slice". `crates/webfang_mcp/tests/common/mod.rs`
ya tiene **todos** esos helpers (`init_ssrf_disabled`, `serve_on_random_port`, `start_test_server`,
`start_seeded_server`, `anonymous_server_options`, `init_session_with_name`, `call_tool`,
`tool_text`, `is_tool_error`, `redact_path`, `html_page_response`, `mount_page_200`), y **cuatro
ficheros de test lo esquivan** con copias locales.

Duplicación medida entre los cinco ficheros MCP implicados
(`mcp_behavioral_test.rs`, `cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs`,
`export_coverage_test.rs`, `common/mod.rs`): **817 líneas en 58 clones**. En concreto:

| Copia local | Ficheros que la llevan | El helper compartido que ya existe |
|---|---|---|
| `RelTempDir` (struct + `new` + `path` + `Drop`, ~17 líneas) | **4**: `mcp_behavioral_test.rs`, `cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs`, `export_coverage_test.rs` | ninguno: hay que **crearlo** en `tests/common/mod.rs` |
| `webfang_binary()` (resolución del binario CLI, ~20 líneas) | **2**: `cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs` | `webfang_test_utils::webfang_path()` (canónico, y es la regla del repo) |
| `normalized_records()` (normaliza el JSONL de export: timestamp → `[TIMESTAMP]`, ordena) | **2**: los mismos dos | ninguno: hay que **crearlo** en `tests/common/mod.rs` |
| `walk_dir()` (~20 líneas) | **2**: los mismos dos | ninguno: hay que **crearlo** en `tests/common/mod.rs` |
| `start_server()` / `start_seeded_server()` (levanta el router MCP en un puerto libre) | **3** (`mcp_behavioral_test.rs`, `cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs`) | `common::start_test_server`, `common::start_seeded_server`, `common::serve_on_random_port` |
| `init_ssrf_disabled()` | **2** (`mcp_behavioral_test.rs`, `scraping_coverage_test.rs`) | `common::init_ssrf_disabled` |
| `redact_path()` | **1** (`mcp_behavioral_test.rs`) | `common::redact_path` |

**El fix por raíz**: `crates/webfang_mcp/tests/common/mod.rs` es **el** módulo compartido del crate
y sus helpers se **bypassean** en cuatro ficheros ⇒ el arrange vive en cinco sitios. El fix es que
**los helpers compartidos vivan en el módulo compartido** (los que ya existen, se usan; los que
no existen, se crean una vez) y que las copias locales **se borren**.

**Caso especial, `webfang_binary()` → `webfang_test_utils::webfang_path()`**: las dos copias
locales resuelven el binario por una lista de candidatos que incluye
`manifest_dir.join("target").join("debug").join(name)` — **un `target/` que cargo no puebla en este
workspace** (la misma clase de defecto que `webfang_test_utils::workspace_root_from_manifest`
documenta como el bug #1366) — y **entra en pánico** ("webfang binary not found — build it first")
en vez de construir el binario. El helper canónico `webfang_test_utils::webfang_path()` hace
justo lo contrario: resuelve `CARGO_TARGET_DIR`/raíz del workspace y **construye el binario bajo
demanda** (`cargo build -p webfang_cli --bin webfang`). `webfang_test_utils` ya es
`[dev-dependencies]` de `webfang_mcp` (verificado en `crates/webfang_mcp/Cargo.toml`) ⇒ **cero
dependencias nuevas**. Y `AGENTS.md` lo exige: *"always use `webfang_path()`, **never** `assert_cmd::cargo_bin(...)`"*.

**Código a cambiar:**

- `crates/webfang_mcp/tests/common/mod.rs`: añadir los helpers que faltan y son compartidos —
  `RelTempDir` (con su `new`/`path`/`Drop`), `normalized_records`, `walk_dir` — y, si el otro
  cracker lo necesita, el `webfang_binary()` que **delegue** en
  `webfang_test_utils::webfang_path()` (no una tercera copia de la resolución). Se alcanzan con
  la forma **bare** `mod common;` de este crate (no con `#[path]`), y la línea
  `use common::{…}` se deja **en una línea** (`rustfmt.toml` fija `max_width = 100` y reescribe
  un wrap manual: lo dice la cabecera del propio módulo).
- `crates/webfang_mcp/tests/mcp_behavioral_test.rs`: se borran `init_ssrf_disabled`,
  `start_test_server`, `redact_path`, `start_seeded_server` y `RelTempDir`; se importan del
  módulo compartido. **El comentario de la cabecera que dice "this file keeps its own local
  server harness … belongs to the arrange-helpers slice" se borra con ellos**: si sobrevive,
  describe un estado que ya no existe.
- `crates/webfang_mcp/tests/cli_mcp_export_jsonl_parity_test.rs`: se borran `RelTempDir`,
  `webfang_binary`, `normalized_records`, `walk_dir` y el `start_server` local.
- `crates/webfang_mcp/tests/sitemap_crawl_run_staleness_test.rs`: ídem.
- `crates/webfang_mcp/tests/export_coverage_test.rs`: se borra `RelTempDir`.
- `crates/webfang_mcp/tests/scraping_coverage_test.rs`: `init_ssrf_disabled` local → el del módulo
  compartido (el resto de su arrange ya usa el módulo).

**Restricción dura**: los **nombres de los tests** y sus **aserciones** no cambian; los snapshots
(`insta`) no cambian; el binario que se ejecuta es el mismo (el canónico). Si un helper local
hacía algo **distinto** del compartido (y no sólo una copia), el que gana es el **compartido** y
la diferencia se documenta en el Log del lote, no se resuelve con un `if`.

**Prueba nombrada:** la del lote (`scripts/check_duplication.sh`), más la suite de los cinco
ficheros MCP tocados en verde (T5).

### S3 · #1757 — el baseline baja por el delta **medido**, y el delta se mide con el comando del gate

**El estado medido hoy** (comando exacto, sin cargo, sin red):

```bash
jscpd "$PWD/crates/" --min-tokens 50 --format rust --mode mild --silent \
      --reporters json --output "$WORKDIR"
```

Es literalmente el detector que invoca `scripts/check_duplication.sh`. La línea que se lee del
informe es `statistics.formats.rust.duplicatedLines`.

| Árbol | Duplicated lines | Clones | Ficheros rust |
|---|---|---|---|
| `b1673234` (el commit que fijó el baseline 9272, 2026-10-13) | **9272** | 864 | 495 |
| `3a785d8e` (la referencia del plan) | **8889** | 828 | 476 |
| `89ff9d46` (`main` hoy, con #2230 mergeado) | **9009** | 839 | 479 |

El baseline commiteado es **9272** (`scripts/quality-baselines.json`, `duplicated_lines_rust`,
`updated: 2026-10-12`). ⇒ **El baseline está rancio en 263 líneas**: desde que se fijó, la
duplicación **bajó** (9272 → 9009) y nadie bajó el ratchet, así que hoy el gate pasa con 263
líneas de margen (`bash scripts/check_duplication.sh` ⇒ `Duplicated lines (rust): 9009
(baseline: 9272)` ⇒ `OK`).

**El fix por raíz**: el ratchet es un **número escrito a mano** que se deja atrás. El fix es (a)
bajar el número al **valor medido del candidato** y (b) **dejar de medir a mano**: el valor que
se escribe es el que produce el comando del gate, sobre el **candidato**, y la nota dice cuál.

**Código a cambiar:**

- `scripts/quality-baselines.json`:
  - `duplicated_lines_rust` ← **el valor medido del candidato** con el comando de arriba (nunca
    un valor calculado a mano).
  - `notes` ← una entrada nueva con: la fecha, el comando exacto, el número **antes** (9009) y
    **después**, el delta, la versión del detector (`jscpd 5.4.0`) y el motivo (la extracción de
    los helpers de arrange). El estilo es el de las entradas existentes (p. ej. la de #1892:
    *"the arithmetic estimate of 12995 (13016 - 21) overstated the descent by 29 lines, because
    removing the clone also shifted clone boundaries elsewhere in the corpus; the measured value
    is authoritative"*).
  - `updated` ← la fecha del descenso.

**Restricción dura (y es la que el plan insiste en)**: **el baseline sólo baja**. El ratchet es
descendente por política (`scripts/quality-baselines.json`, `description`), y una subida exige
decisión del maintainer (cláusula (b)). El descenso 9272 → (medición del candidato) **es una
bajada** y por tanto no necesita permiso. El delta **medido** es el único criterio: la estimación
aritmética ("las 176 de S1 y las 817 de S2") **no** es el número que se escribe,
porque al desaparecer un clon las fronteras de otros se desplazan (precedente #1892: la
estimación se equivocó en 29 líneas).

**Criterio de aceptación, verificable**: (a) el gate verde sobre el candidato; (b) el valor
commiteado **igual** a la medición del candidato con el comando del gate (re-medido sobre el
árbol final, no sobre un `git stash` de trabajo); (c) el delta **estrictamente positivo**; (d) la
nota dice los tres números.

**Prueba nombrada:** `scripts/check_duplication.sh` (la del plan) — verde sobre el candidato, con
el valor commiteado igual a la medición.

### S4 · #1757 — el test nombrado del lote deja de ser un faro sin prueba

**Lo que `scripts/check_duplication.sh` prueba hoy** (leído): calcula el baseline de
`scripts/quality-baselines.json`, exige el detector exacto (`jscpd 5.4.0`), corre jscpd sobre
`crates/`, lee `statistics.formats.rust.duplicatedLines` y **falla si el número supera el
baseline**. Todo eso está bien. El problema es otro, y es de la clase que este repo ya audits dos
veces:

1. **El guard confunde "no medí" con "no hay duplicación".** Si el informe **no trae** la sección
   `statistics.formats.rust`, el parser cae en `val = 0` y el guard imprime
   `Duplicated lines (rust): 0` ⇒ **verde por vacuidad**. Medido en este árbol (stub de jscpd que
   escribe un informe sin sección `rust`): `exit=0`, `OK: duplication at or below baseline.`
   Es **exactamente** la forma del FAIL-OPEN que L9 encontró en el guard del grafo de build
   (verbatim del Log del plan): *"Un `cargo tree` fallido devolvia 0, y 0 es exactamente el
   valor que significa "el grafo no lleva chromiumoxide""*.
2. **El guard pasa vacío.** Con un corpus de **cero** ficheros rust, jscpd escribe
   `Found 0 exact clones with 0(0.00%) duplicated lines in 0 (0 formats) files`, el guard imprime
   `Duplicated lines (rust): 0` y **sale con 0**. Un semáforo verde cableado a nada. (Es la
   forma que L8 cerró en el guard de targets vacíos.)
3. **El guard no tiene harness.** `scripts/guard_coverage_baseline.txt` lo dice con todas las
   letras:

   > no-harness check_duplication.sh   no harness and no ODD task yet — needs its own issue

   El meta-guard `scripts/test_guard_coverage.sh` exige que **todo** `check_*.sh` cableado tenga
   un harness de negación, con marcador `# NEGATION:`, cableado en un workflow, y que el harness
   **pase con el guard intacto y falle con el guard sustituido por `exit 0`**. La lista de
   baseline **sólo puede encogerse**: una entrada que deja de reproducirse es un fallo
   (*"stale-baseline"*). Este lote es el que cierra esa entrada.

**El fix por raíz**: el guard que decide el ratchet tiene que poder **decidir** (y no pasar por
construcción), y su decisión tiene que estar **probada**. Dos cosas: (a) cerrar la lectura
"no medí" ⇒ "no hay duplicación"; (b) el harness que falta.

**Código a cambiar:**

- `scripts/check_duplication.sh`:
  - la lectura del informe **deja de tener un cero por defecto**: si la sección `rust` no está, o
    si el número no es un entero, o si el corpus medido tiene **cero** ficheros rust, el guard
    **falla con un mensaje que nombre la causa** (no sale con 0). El fallo tiene que distinguir
    "el detector no produjo un informe de Rust" de "el detector falló" y de "no hay ficheros que
    medir": son tres causas y tres mensajes, porque un mensaje único esconde la causa.
  - el **pin del detector** se conserva intacto (`jscpd 5.4.0`): es lo que impide medir con
    jscpd-rs y creer que el árbol está limpio (issue #1893).
- `scripts/test_duplication.sh` (nuevo): el **harness de negación** que el meta-guard exige.
  - Ruta: `scripts/test_duplication.sh` (la convención que `scripts/test_guard_coverage.sh`
    reconoce: `scripts/test_<name>.sh` o `scripts/tests/test_check_<name>.sh`).
  - **Cada caso lleva su marcador `# NEGATION:`** (el marcador que el meta-guard exige con
    `grep -qE '^[[:space:]]*# NEGATION:'`) y dice **qué hacía mal el estado pre-fix** de ese
    caso, en una línea.
  - **Hermético y sin detector real**: el harness mete un **stub** de `jscpd` en el `PATH` que
    escribe el informe que cada caso necesita. Motivo (no es comodidad, es una restricción): el
    meta-guard corre el harness **en el job `repo-guards`**, donde **no hay jscpd** (el detector se
    instala en el job `code-quality`), y un harness que necesita un binario que allí no existe
    sale con error ⇒ el meta-guard lo marca `broken-harness` ⇒ categoría **no baselinable** ⇒ job
    rojo. El harness debe ser verde en un árbol intacto y en una máquina sin jscpd.
  - **Salida**: el resumen `PASS=<n> FAIL=<n>` (forma de `scripts/test_build_graph.sh`) y
    `exit != 0` si hay un solo FAIL.
  - **Casos (mínimum)**:
    1. informe con `duplicatedLines` **por encima** del baseline ⇒ el guard **falla** nombrando
       `Duplication INCREASED` (el RED que el guard existe para; control positivo incluido: el
       mismo informe con el baseline **subido** ⇒ el guard **pasa**, así que se ve que *decide*);
    2. informe **sin** la sección `statistics.formats.rust` ⇒ el guard **no** puede pasar por
       vacuidad (éste es el RED real contra el guard pre-fix, medido hoy: `exit=0`);
    3. corpus con **cero** ficheros rust ⇒ el guard **no** puede pasar vacío (RED real medido hoy:
       `exit=0`);
    4. el detector **falla** (exit ≠ 0) ⇒ el guard falla **ruidosamente**, y **no** lo lee como
       "0 duplicadas" (comprobado hoy: el guard ya lo hace bien — `::error::jscpd exited with
       code 3` — y el caso lo **pina**, no lo arregla);
    5. el detector con **versión distinta** de la fijada ⇒ el guard **rechaza** medir (comprobado
       hoy: `::error::jscpd es 'jscpd 0.1.12', se esperaba 'jscpd 5.4.0'`);
    6. **baseline ausente** ⇒ el guard falla (comprobado hoy: `::error::Missing baseline file`);
    7. **baseline sin la clave** `duplicated_lines_rust` (o no-JSON) ⇒ el guard falla (comprobado
       hoy: sale con `exit=1` y un traceback de Python — el veredicto es correcto, el
       diagnóstico no: el caso lo **pina** y exige que el mensaje nombre la causa).
    Cada caso negativo lleva **su control positivo** en el mismo árbol: un harness que puede dar
    verde por construcción no es evidencia (la lección de L8 y L9, escrita en el propio Log del
    plan).
- `scripts/guard_coverage_baseline.txt`: se borra la línea
  `no-harness check_duplication.sh …`. **La lista sólo puede encogerse**; dejarla ⇒ el meta-guard
  falla con `stale-baseline`.
- `.github/workflows/ci.yml`: un step que corra `bash scripts/test_duplication.sh` en el job
  `repo-guards`, junto a los demás harnesses de semántica (`scripts/test_dead_tests.sh`,
  `scripts/test_mcp_public_surface.sh`, …), con el estilo de sus vecinos (nombre + comentario con
  el número de issue y el porqué). **Coordinación**: el mapa de colisiones del plan lista
  `ci.yml` para L2, L3, L4 y L13 y **no** para L12 ⇒ este paso es un escritor más de ese
  fichero; se aplica la regla del propio plan (cada lote commitea *su* step en su rama y el step
  se re-aplica sobre el `main` nuevo al abrir la PR). El job `code-quality` **no** cambia: ahí
  sigue corriendo el ratchet real contra `crates/`.

**Prueba nombrada del lote**: `scripts/check_duplication.sh` (la del plan) **más** su harness de
negación `scripts/test_duplication.sh`.

---

## Tasks

- [ ] T1 · S1 — `crates/webfang_core/tests/common/sitemap_arrange.rs` (nuevo) +
  `crates/webfang_core/tests/common/mod.rs` (`pub mod sitemap_arrange;`): los helpers
  `parser()`, `serve_and_parse(mock, mount_path, response, entry_path)`,
  `serve_many_and_parse(mock, mounts, entry_path)` y el helper del arrange de sitemap-index, con
  el **mismo cuerpo** que hoy vive en `sitemap_integration.rs`. **Prueba**: los tres binarios de
  test verdes; el clon medido de S1 desaparece del informe de jscpd.
- [ ] T2 · S1 — `crates/webfang_core/tests/sitemap_integration.rs`: se borran los helpers locales
  y se usan los del módulo compartido; los cuatro tests de sitemap-index pasan a usar el helper
  de índice. **Prueba**: `cargo nextest run -p webfang_core --test sitemap_integration` verde,
  **sin** cambio de snapshot.
- [ ] T3 · S1 — `crates/webfang_core/tests/sitemap_behavioral.rs` y
  `crates/webfang_core/tests/orchestrator_unit_tests.rs`: sus 8 + 5 tests pasan a usar el arrange
  compartido; aserciones y nombres intactos; el test que no desactiva el entry guard sigue sin
  desactivarlo. **Prueba**: `cargo nextest run -p webfang_core --test sitemap_behavioral` y
  `cargo nextest run -p webfang_core --test orchestrator_unit_tests` verdes.
- [ ] T4 · S2 — `crates/webfang_mcp/tests/common/mod.rs`: añadir `RelTempDir`, `normalized_records`
  y `walk_dir` (una vez), y el `webfang_binary()` que **delegue** en
  `webfang_test_utils::webfang_path()`. **Prueba**: los cinco ficheros MCP de T5 compilan y sus
  helpers nuevos existen en el módulo compartido.
- [ ] T5 · S2 — `crates/webfang_mcp/tests/mcp_behavioral_test.rs`,
  `cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs`,
  `export_coverage_test.rs`, `scraping_coverage_test.rs`: se borran las copias locales
  (`init_ssrf_disabled`, `start_test_server`, `start_seeded_server`, `redact_path`, `RelTempDir`,
  `webfang_binary`, `normalized_records`, `walk_dir`) y se usan los compartidos; el comentario de
  la cabecera de `mcp_behavioral_test.rs` que reserva el dedup para "the arrange-helpers slice"
  se borra con ellos. **Prueba**: `cargo nextest run -p webfang_mcp --test mcp_behavioral_test`,
  `--test cli_mcp_export_jsonl_parity_test`, `--test sitemap_crawl_run_staleness_test`,
  `--test export_coverage_test`, `--test scraping_coverage_test` verdes, **sin** cambio de
  snapshot; el binario que ejecutan es el de `webfang_test_utils::webfang_path()`.
- [ ] T6 · S3 — `scripts/quality-baselines.json`: `duplicated_lines_rust` ← la **medición** del
  candidato con el comando del gate; `notes` ← la entrada con comando, antes, después, delta,
  versión del detector y motivo; `updated` ← la fecha. **Prueba**: la medición re-hecha sobre el
  árbol final **iguala** el valor commiteado, y el gate sale verde.
- [ ] T7 · S4 — `scripts/check_duplication.sh`: la lectura del informe deja de tener el cero por
  defecto (sección `rust` ausente, número no entero y corpus de cero ficheros ⇒ fallo con mensaje
  que nombre la causa); el pin del detector (`jscpd 5.4.0`) intacto. **Prueba**: `bash
  scripts/check_duplication.sh` verde sobre el árbol, y los tres casos de fallo por vacuidad
  reproducibles sobre un árbol de `mktemp`.
- [ ] T8 · S4 — `scripts/test_duplication.sh` (nuevo): harness de negación, hermético (stub de
  `jscpd` en el `PATH`), con marcador `# NEGATION:` por caso, control positivo por cada
  aserción negativa, los siete casos de S4 y salida `PASS=<n> FAIL=<n>`. **Prueba**: **RED**
  contra el guard pre-fix por la razón del defecto (los casos 2 y 3 dan verde por vacuidad) y
  **GREEN** después de T7; el meta-guard lo acepta (harness verde con el guard intacto, rojo con
  el guard sustituido por `exit 0`).
- [ ] T9 · S4 — `scripts/guard_coverage_baseline.txt`: borrar la línea
  `no-harness check_duplication.sh …`. **Prueba**: `bash scripts/test_guard_coverage.sh` verde
  **sin** `stale-baseline`.
- [ ] T10 · S4 — `.github/workflows/ci.yml`, job `repo-guards`: un step que corra `bash
  scripts/test_duplication.sh`, con el estilo de sus vecinos (nombre + comentario con el issue y
  el porqué + la nota de que no corre cargo). **Prueba**: el meta-guard ve el harness cableado;
  el job `code-quality` no cambia.
- [ ] T11 · S1–S4 — verificación: `bash scripts/test_duplication.sh`; `bash
  scripts/check_duplication.sh`; el comando de medición de S3 (con el número antes y después);
  `bash scripts/test_guard_coverage.sh`; `cargo nextest run -p webfang_core --test
  sitemap_integration --test sitemap_behavioral --test orchestrator_unit_tests`; `cargo nextest run
  -p webfang_mcp --test mcp_behavioral_test --test cli_mcp_export_jsonl_parity_test --test
  sitemap_crawl_run_staleness_test --test export_coverage_test --test scraping_coverage_test`;
  `cargo check` + `cargo clippy --all-targets --all-features -- -D warnings -W
  clippy::cognitive_complexity -W clippy::too_many_lines` + `cargo fmt --all -- --check` + `env
  "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps`.

---

## Log

L1 — Petición del owner (verbatim, del plan madre `odd/tasks/backlog-implementation-plan.md`):

> tengo muchas y  varias issues abiertas y necesito un plan para ir implementando utliza la
> skill adecuada para triage de issues

> y crea on plan odd para ir implementando todo

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

> te dejo la responsabilidad de decision a ti como experto, y si tienes que investigar en
> internet para buscar la mejor opcion o mejor alternativa tienes mi permiso total.

> pero tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro producto.

L2 — La fila L12 del plan madre (verbatim):

> | **L12** | Duplicación | #1757 | helpers de arrange compartidos; bajar el baseline por el delta
> **medido** (9272) | `scripts/check_duplication.sh` | −60/+12 | inline |

L3 — Qué se leyó antes de escribir: el plan madre completo
(`odd/tasks/backlog-implementation-plan.md`, hasta el final de su `## Log`), de donde salen la fila
L12, el mapa de colisiones (`scripts/quality-baselines.json` → L10, L12 y #2230) y la regla de
calidad de harnesses; la issue #1757 **íntegra** vía REST (`gh api
repos/XaviCode1000/webfang/issues/1757 --jq .body`); `scripts/check_duplication.sh`,
`scripts/quality-baselines.json`, `scripts/guard_coverage_baseline.txt` y
`scripts/test_guard_coverage.sh` (el meta-guard que decide qué harness existe); los ficheros de
test de los cinco puntos de copia, con `codedb` 0.2.5849 (`outline`, `find`, `callers`) y
`codegraph` sobre el checkout principal (`/home/xavi/Projects/Rust/webfang`, `main@89ff9d46`).

L4 — Herramientas y estado del árbol: el índice de referencia es el checkout principal
(`main@89ff9d46`, `codedb status` → `files 1071 indexed`, `head 89ff9d46ec8a`). La rama del plan
(`fix/merge-when-green-merged-state`) está en `71df1c62`; el árbol de `crates/` difiere entre
ambos (el merge de #2230 y el resto de lotes ya aterrizados), así que **las mediciones de
duplicación de este Log son sobre `main@89ff9d46`**, que es de donde parte la rama del lote. **No
se corrió cargo** (el lote lo prohíbe) y **no se commiteó nada**: sólo este documento y su
espejo.

L5 — El bloqueo del lote ya no existe: **#2230 mergeó** (`main@89ff9d46` es
`fix(exporter): un JSONL no-UTF8 degrada y salta en vez de abortar el export (#1812) (#2230)`), que
era la condición que el plan anotaba en la fila L12 (*"PENDIENTE (espera #2230, que sigue
abierta)"*). El mapa de colisiones lista `scripts/quality-baselines.json` para L10, L12 y #2230:
**#2230 ya está mergeado** y L10 no toca ese fichero (su baseline es
`scripts/duplicate-crate-baseline.txt`, otro fichero) ⇒ **L12 es hoy el único escritor** de
`scripts/quality-baselines.json`.

L6 — **La evidencia medida** (comando único, el mismo que invoca el gate; jscpd 5.4.0, sin red,
sin cargo):

```bash
jscpd "$PWD/crates/" --min-tokens 50 --format rust --mode mild --silent \
      --reporters json --output "$WORKDIR"
# de ahí: statistics.formats.rust.duplicatedLines
```

- `b1673234` (2026-10-13, el commit que fijó el baseline 9272): **9272** líneas duplicadas, 864
  clones, 495 ficheros rust ⇒ **el baseline era exacto cuando se escribió**.
- `3a785d8e` (la referencia del plan): **8889** líneas, 828 clones, 476 ficheros.
- `89ff9d46` (`main` hoy): **9009** líneas, 839 clones, 479 ficheros ⇒ **el baseline commiteado
  (9272) está rancio en 263 líneas**: desde que se fijó, la duplicación bajó y el ratchet no
  bajó con ella. `bash scripts/check_duplication.sh` sobre `main` ⇒ `Duplicated lines (rust): 9009
  (baseline: 9272)` ⇒ `OK` (con 263 líneas de margen).
- Reparto de las 9009 líneas: **3864 en ficheros de test** (`/tests/`), **5145 fuera de test**.
- Los cinco ficheros MCP de S2: **817 líneas duplicadas en 58 clones** entre sí.
- El trío de sitemap de S1: **176 líneas en 11 clones** (`orchestrator_unit_tests.rs` ↔
  `sitemap_behavioral.rs` 159/9, `sitemap_behavioral.rs` ↔ `sitemap_integration.rs` 17/2), más
  **60 líneas en 4 clones** de arrange de índice dentro de `sitemap_integration.rs`.

L7 — **El FAIL-OPEN del guard, medido** (sobre copias en `mktemp` del guard y del baseline, con
un stub de `jscpd` en el `PATH`; **el repo no se tocó**):

| Escenario | Salida observada | Veredicto |
|---|---|---|
| informe **sin** `statistics.formats.rust` | `Duplicated lines (rust): 0` ⇒ `OK` ⇒ **exit 0** | **FAIL-OPEN** (el guard pasa por vacuidad) |
| corpus con **cero** ficheros rust (`crates/` vacío) | `Found 0 exact clones … in 0 (0 formats) files` ⇒ `0` ⇒ **exit 0** | **FAIL-OPEN** (verde sin medir nada) |
| detector que **falla** (exit 3) | `::error::jscpd exited with code 3` ⇒ exit 1 | correcto (ya lo hace bien) |
| detector con **versión distinta** (`jscpd 0.1.12`) | `::error::jscpd es 'jscpd 0.1.12', se esperaba 'jscpd 5.4.0'` ⇒ exit 1 | correcto |
| `crates/` **inexistente** | `::error::jscpd exited with code 1` (`path does not exist`) ⇒ exit 1 | correcto |
| baseline **ausente** | `::error::Missing baseline file` ⇒ exit 1 | correcto |
| baseline **sin la clave** `duplicated_lines_rust` | traceback de Python (`KeyError`) ⇒ exit 1 | veredicto correcto, **diagnóstico feo** (lo pina el caso 7 del harness) |
| fixture duplicado real, baseline 0 | `Duplication INCREASED: 9 > 0` ⇒ exit 1 | correcto (el RED del guard) |
| el mismo fixture, baseline 99999 | `OK: duplication at or below baseline.` ⇒ exit 0 | correcto (el control positivo) |

La causa de las dos primeras filas es **una sola**: el parser del guard convierte
"no hay sección `rust`" en `0`, y `0` es exactamente el valor que significa "no hay
duplicación". Es la misma forma que el FAIL-OPEN que L9 encontró en el guard del grafo de build
(verbatim del Log del plan: *"Un `cargo tree` fallido devolvia 0, y 0 es exactamente el valor que
significa "el grafo no lleva chromiumoxide""*) y la misma que el guard de targets vacíos que L8
cerró.

L8 — **El guard no tiene harness, y el repo lo dice** (`scripts/guard_coverage_baseline.txt`):

> no-harness check_duplication.sh   no harness and no ODD task yet — needs its own issue

El meta-guard `scripts/test_guard_coverage.sh` exige, para **todo** `check_*.sh` cableado: (1)
harness en `scripts/test_<name>.sh` o `scripts/tests/test_check_<name>.sh`; (2) harness cableado
en un workflow; (3) marcador `# NEGATION:` en el harness; (4) el harness **verde con el guard
intacto y rojo con el guard sustituido por `exit 0`**. Y su lista de baseline **sólo puede
encogerse**: una entrada que deja de reproducirse falla con `stale-baseline`. Este lote es el
issue que esa línea pide.

L9 — **Decisiones que se toman en este lote** (todas dentro del código; ninguna pide permiso al
owner):

1. **El helper compartido vive en el crate, no en `webfang_test_utils`.** Un arrange que necesita
   `wiremock`/`tokio` no puede vivir en `webfang_test_utils` (sus únicas dependencias son `regex`
   y `webfang_core`) sin **dependencias nuevas** ⇒ prohibido por el scope. El módulo compartido de
   cada crate (`crates/webfang_core/tests/common/`, `crates/webfang_mcp/tests/common/mod.rs`) ya
   existe y ya es el sitio donde vive el harness de esa crate.
2. **Los nombres de los helpers no cambian.** `parser`, `serve_and_parse`, `serve_many_and_parse`
   son el vocabulario de #1889; moverlos es un *move*, no un rediseño. Los nombres de los **tests**
   tampoco cambian: son el índice de la suite.
3. **`webfang_binary()` local ⇒ `webfang_test_utils::webfang_path()`.** La copia local resuelve
   por un `target/` que cargo no puebla (la forma del bug #1366) y **entra en pánico** en vez de
   construir; el helper canónico hace lo contrario. `webfang_test_utils` ya es dev-dependency de
   `webfang_mcp` ⇒ cero dependencias nuevas. Y es la regla escrita del repo.
4. **El harness de negación es hermético (stub de `jscpd`), no usa el detector real.** Motivo
   duro: el meta-guard corre el harness en el job `repo-guards`, donde el detector **no está**
   (se instala en `code-quality`). Un harness que necesita jscpd saldría con error allí ⇒
   `broken-harness`, categoría **no baselinable** ⇒ job rojo. El reparto es explícito: **el
   harness prueba la decisión del guard; la fidelidad del detector real la prueba el ratchet que
   corre en `code-quality`**, que este lote también baja. Un harness verde **no** es prueba de que
   el detector real encuentra duplicados, y la spec no lo afirma.
5. **El baseline baja al valor medido, y la nota dice los tres números.** La estimación
   aritmética no se escribe (precedente #1892: se equivocó en 29 líneas). La corrección del
   **desfase de 263 líneas** respecto de la medición real se registra como lo que es: una
   corrección de error de medición (la categoría (a) de la política del propio fichero), no una
   relajación del ratchet, porque el valor nuevo sigue siendo **el número medido**.
6. **No se abre PR y no se commitea nada en esta entrega** (decisión del owner, verbatim en L1).

L10 — **Premisas de #1757: qué dice la issue, qué dice el árbol, cuál es la verdad** (medido el
2026-10-22 sobre `main@89ff9d46` con el comando de L6):

| # | Qué dice la issue #1757 | Qué dice el árbol (medido) | La verdad |
|---|---|---|---|
| P1 | "El ratchet … quedó en **7683** líneas duplicadas" | El baseline commiteado es **9272** (`scripts/quality-baselines.json`, `updated: 2026-10-12`, subido por #1983) | **Caducado.** 7683 era el valor de la escala del detector viejo; el valor vigente es 9272 y es ése el que L12 baja |
| P2 | "Baseline … descendido **por debajo de 7602** (el valor real de main pre-batch)" | 7602 pertenece a la escala del detector **anterior**: el 2026-10-06 (#1867) recalibró la escala de 7683 a **13310** al cambiar de detector (jscpd-rs → kucherenko/jscpd v5 5.4.0), y el propio `notes` del baseline dice que *"the measurement is incomparable with the previous baseline"* | **Falso como criterio.** "Por debajo de 7602" es inalcanzable y no comparable bajo el detector actual (el árbol entero mide 9009). El criterio comparable es: **por debajo de la medición del candidato y por debajo del baseline vigente (9272)** |
| P3 | "`jsonl_exporter.rs`: **~109** líneas de self-clones" | **96 líneas en 8 self-clones** (`crates/webfang_core/src/infrastructure/export/jsonl_exporter.rs`) | **Orden de magnitud, no cifra exacta.** La AC 1 de la issue (que ese fichero no tenga self-clones de ≥50 tokens) **hoy no se cumple** |
| P4 | "`checkpoint.rs`: **~27** líneas de self-clones" | **22 líneas en 3 self-clones** (`crates/webfang_core/src/application/crawler/checkpoint.rs`) | Ídem: la AC 1 **hoy no se cumple** |
| P5 | "`fingerprint.rs` (**~9**)" | **9 líneas en 1 self-clone** (`crates/webfang_core/src/infrastructure/persistence/fingerprint.rs`) | **CONFIRMADA** |
| P6 | "`batch_flow.rs`/`discovery_phase.rs` (**~40** combinado)" | `crates/webfang_core/src/cli/batch_flow.rs`: **0 self-clones**; `crates/webfang_core/src/cli/discovery_phase.rs`: **6 líneas en 1 self-clone** | **NO CONFIRMADA** (6, no 40) |
| P7 | "Los dos primeros ítems son duplicación real en código de producción (~136 líneas); **el resto es boilerplate de tests/bench**" | Las 136 líneas de producción son **118** medidas (96 + 22); y el **43 %** de las 9009 líneas duplicadas del árbol (**3864**) está **en ficheros de test** | **A medias.** El "boilerplate de tests" no es residuo: es la masa mayor, y es donde el repo ya tiene una campaña establecida de extracción de helpers de arrange (#1877, #1888, #1889, #1890, #1944) |
| P8 | El plan: "bajar el baseline por el delta **medido** (9272)" | 9272 es el **baseline commiteado**, no una medición (la medición de hoy es 9009) | **CONFIRMADA como valor del baseline.** El "delta medido" es lo que este lote tiene que producir |
| P9 | (alcance) La AC 1 pide deduplicar **self-clones de producción** en `jsonl_exporter.rs` y `checkpoint.rs` | La fila L12 del plan define el fix como **"helpers de arrange compartidos"**, y el propio código de `mcp_behavioral_test.rs` reserva ese trabajo para *"the arrange-helpers slice"* | **El alcance del lote es el del plan, no el de la AC 1.** La AC 1 (producción) **no** la cubre este lote ⇒ **este lote NO cierra #1757**; su cierre necesita la palabra del owner y, si se quiere, un lote aparte para los self-clones de producción |
| P10 | (encargo del plan) "helpers de arrange compartidos" | El arrange duplicado está en **dos** sitios: el de sitemap (3 binarios de test de `webfang_core`) y el del servidor MCP (5 ficheros de test de `webfang_mcp`), y el propio repo dice que esa dedup está pendiente | **CONFIRMADA**, y con más masa de la que el plan estimaba (176 + 817 = 993 líneas duplicadas en los ficheros afectados) |

L11 — **Lo que este lote NO hace** (para que nadie lo lea después como hecho):

- **No** deduplica los self-clones de producción de `jsonl_exporter.rs` (96 líneas) ni de
  `checkpoint.rs` (22 líneas): es la AC 1 de #1757 y es un alcance distinto (P9). Queda medido y
  documentado; el owner decide si abre un lote aparte.
- **No** sube el baseline en ningún caso (la política del fichero es descendente; una subida
  exige decisión del maintainer).
- **No** toca `Cargo.toml` de ninguna crate: el lote no mete ninguna dependencia (verificado: los
  helpers compartidos viven en módulos de test que ya dependen de `wiremock`/`tokio`/`tempfile`).
- **No** toca `docs/` (fuera de alcance salvo autorización expresa), ni `CHANGELOG.md` (un solo
  escritor: la Release PR).
- **No** abre PR ni commitea código: este documento es sólo la especificación (L1, verbatim).

L12 — **Estado de esta entrega**: este documento y su espejo
(`odd/duplication-arrange-helpers/tasks.md`) son **lo único** que produce esta entrega. No se
escribió código de producción, no se corrió `cargo`, no se commiteó nada. La implementación es
del worker del lote, sobre su propio worktree, a partir de esta spec.

---

L13 — **Implementación de T1–T5** (la escribe el worker del lote, sobre el worktree
`fix/duplication-arrange-helpers`; el árbol queda **sin commitear**, L1). T6–T11 **no** se
tocaron: `scripts/`, `.github/`, `Cargo.toml` y `Cargo.lock` están intactos.

**L13.1 — S1 · T1–T3: el arrange de sitemap, una sola vez**

- `crates/webfang_core/tests/common/sitemap_arrange.rs` (nuevo): `parser()`,
  `serve_and_parse(mock, mount_path, response, entry_path)`,
  `serve_many_and_parse(mock, mounts, entry_path)` — **el mismo texto** que hoy vivía duplicado en
  `sitemap_integration.rs` (S1: *"el cuerpo de `serve_and_parse` **no** se reescribe"*) — más el
  helper del arrange de sitemap-index, `serve_index_and_parse(mock, index_path, index_xml,
  child_mounts)`, y `sitemap_index_with_base(base_url)`.
- `crates/webfang_core/tests/common/mod.rs`: `pub mod sitemap_arrange;` (una línea, el patrón de
  los otros cuatro submódulos). La forma del `mod` import es la de esta crate:
  `#[path = "common/mod.rs"] mod common;` (11 binarios de `webfang_core` lo usaban; ahora son 14).
- `sitemap_integration.rs`: fuera los tres helpers locales; los **seis** tests de índice usan
  `serve_index_and_parse` (los cuatro que la spec nombraba + los dos que comparten la misma
  forma: `test_sitemap_index_malformed_child_included_in_error` y
  `test_sitemap_index_deduplicates_across_children`).
- `sitemap_behavioral.rs` (8 tests) y `orchestrator_unit_tests.rs` (5 tests): usan el arrange
  compartido. **Nombres de test y aserciones intactos.**
- **El helper no toca el entorno**: la postura del entry guard sigue siendo del llamador, así que
  `sitemap_malformed_xml_returns_error` (el único que hoy **no** llama a
  `EnvGuard::entry_guard_off()`) **conserva esa diferencia** (S1 lo pedía explícitamente).
- **Honestidad del cronómetro**: `sitemap_large_1000_plus_urls` cronometra ahora el
  `serve_and_parse` completo (mount + parse) en vez de sólo el parse. El presupuesto (2 s) y el
  mensaje de aserción quedan **intactos**; lo medido se ensancha en el mount (milisegundos).

**L13.2 — S2 · T4–T5: el harness MCP compartido, una sola vez**

- `crates/webfang_mcp/tests/common/mod.rs`: `RelTempDir` (struct + `new` + `path` + `Drop`),
  `normalized_records`, `walk_dir` y `webfang_binary()` — **una vez cada uno** — y
  `init_ssrf_disabled` pasa a `pub`.
- `webfang_binary()` **delega** en `webfang_test_utils::webfang_path()` (T4): cero dependencias
  nuevas (`webfang_test_utils` ya era dev-dependency de `webfang_mcp`) y la regla del repo
  (`webfang_path()`, nunca `assert_cmd::cargo_bin`).
- Copias locales borradas (sitios de la copia → módulo compartido):

  | Copia local borrada | Sitios | Queda en |
  |---|---|---|
  | `init_ssrf_disabled` | 2 (`mcp_behavioral_test.rs`, `scraping_coverage_test.rs`) | `common::init_ssrf_disabled` |
  | `start_test_server` | 1 (`mcp_behavioral_test.rs`) | `common::start_test_server` |
  | `start_seeded_server` | 1 (`mcp_behavioral_test.rs`) | `common::start_seeded_server` |
  | `redact_path` | 1 (`mcp_behavioral_test.rs`) | `common::redact_path` |
  | `RelTempDir` | 4 (`mcp_behavioral_test.rs`, `cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs`, `export_coverage_test.rs`) + 1 (`scraping_coverage_test.rs`) | `common::RelTempDir` |
  | `webfang_binary` | 2 (`cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs`) | `common::webfang_binary` → `webfang_test_utils::webfang_path()` |
  | `normalized_records` | 2 (los mismos dos) | `common::normalized_records` |
  | `walk_dir` (módulo `mod walkdir`) | 2 (los mismos dos) | `common::walk_dir` |
  | `start_server` local | 2 (`cli_mcp_export_jsonl_parity_test.rs`, `sitemap_crawl_run_staleness_test.rs`) | `common::start_server(None)` |

- El comentario de cabecera de `mcp_behavioral_test.rs` que reservaba el dedup para *"the
  arrange-helpers slice"* se borró con las copias: describía un estado que ya no existe.

**L13.3 — Las divergencias que la regla "gana el compartido" no cerraba sola** (S2 pide
explícitamente que la diferencia se documente aquí y no se resuelva con un `if`):

1. **`start_seeded_server`: buffer de sesión vs. repositorio.** La copia local de
   `mcp_behavioral_test.rs` sembraba el **buffer de sesión**; la compartida sembraba el
   **repositorio** de resultados. Con la compartida, las herramientas de export —que leen la
   sesión desde #1290— verían una sesión vacía, y `test_export_jsonl_writes_real_file` (y su
   snapshot) se caerían. Se resolvió **sembrando la sesión en el helper compartido**: el sembrado
   del repositorio sólo se ejercitaba con `n = 0` (`export_coverage_test`), o sea, era código
   muerto; y las dos copias sembraban **exactamente el mismo contenido** (`Seed Title {i}`,
   `Seed body content number {i} for export testing.`, `https://seed.example.com/page/{i}`), así
   que **el snapshot no cambia**.
2. **`init_ssrf_disabled`: las dos capas y el `Once`.** Las tres copias no eran iguales: la de
   `mcp_behavioral_test.rs` levantaba las **dos** capas (MCP + entry guard), la de
   `scraping_coverage_test.rs` sólo la **capa MCP**, y la compartida las dos. **Ganó la
   compartida** (las dos capas) y se le puso el `Once` que las copias locales ya traían: sin él, el
   `env_set` reentrante **deadlockea** contra el `EnvGuard` que `scraping_coverage_test.rs`
   sostiene en `mcp_crawl_static_literal_ip_rejected_by_entry_guard` (invariante #1224).
   Consecuencia documentada: en ese fichero el entry guard queda **desarmado en el ambiente del
   proceso**; el test que lo pinna **armado** lo limpia explícitamente (`EnvGuard::clean`), así
   que su contrato sigue intacto (0 páginas, 1 error, 0 peticiones).
3. **`RelTempDir`**: la copia de `sitemap_crawl_run_staleness_test.rs` nombraba el directorio con
   nanos; las otras cuatro con un contador por proceso. **Ganó la del contador** (mismo criterio
   de unicidad: pid + contador).
4. **`normalized_records`**: la copia de `sitemap_crawl_run_staleness_test.rs` ordenaba por `url`;
   la de `cli_mcp_export_jsonl_parity_test.rs` ordenaba por `url` con respaldo en `failed_url`.
   **Ganó la segunda**; para los registros que ambas suites producen (todos con `url`) el orden es
   idéntico.
5. **`webfang_binary()`**: cambio de comportamiento **deliberado** (L9.3): las dos copias sondeaban
   un `target/` que cargo no puebla en este workspace virtual y **entraban en pánico**; el helper
   canónico **construye** el binario bajo demanda.
6. **`start_server` local de los dos ficheros de paridad**: no era una copia del
   `common::start_server` (devolvía además un `TempDir` y usaba un `Container` con `output_dir`
   propio). Se borró y se usa `common::start_server(None)`: ninguna aserción dependía del
   `TempDir` (los exports van a un `RelTempDir` **relativo**, que la puerta de rutas acepta por el
   contrato del CWD) ni del `output_dir` del contenedor (esas suites no usan el pipeline).

**L13.4 — La medición del delta (el número que T6 necesita)** — mismo comando del gate
(jscpd 5.4.0, `--min-tokens 50 --format rust --mode mild`), sobre el **mismo** commit:

| Árbol | `statistics.formats.rust.duplicatedLines` | Clones | Ficheros rust |
|---|---|---|---|
| antes (`HEAD` `05aece2e` = `main@89ff9d46` + el commit de la spec; `crates/` idéntico a main) | **9009** | 839 | 479 |
| después (árbol con T1–T5) | **8515** | 811 | 480 |
| delta | **−494** | **−28** | +1 (el módulo nuevo) |

La estimación aritmética de la spec (176 + 817 = 993) **exageraba el descenso en 499 líneas**:
confirma la regla de S3 (el número medido manda, la estimación no). ⇒ **el baseline de
`scripts/quality-baselines.json` (9272) debe bajar a 8515 en T6.** Este lote **no** tocó ese
fichero (T6 es otra unidad).

**L13.5 — Verificación** (los diez comandos del encargo, en el worktree, con
`CARGO_TARGET_DIR=$HOME/.cache/cargo-target/fix-duplication-arrange-helpers`,
`CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=1` y `$(rustup which cargo)`):

| Comando | Resultado |
|---|---|
| `cargo nextest run -p webfang_core --test sitemap_integration` | `32 tests run: 32 passed, 0 skipped` (exit 0) |
| `cargo nextest run -p webfang_core --test sitemap_behavioral` | `24 tests run: 24 passed, 0 skipped` (exit 0) |
| `cargo nextest run -p webfang_core --test orchestrator_unit_tests` | `21 tests run: 21 passed, 0 skipped` (exit 0) |
| `cargo nextest run -p webfang_mcp --test mcp_behavioral_test` | **`0 tests run`** (exit 4, `no tests to run`) — ver L13.6 |
| `cargo nextest run -p webfang_mcp --test cli_mcp_export_jsonl_parity_test` | **`0 tests run`** (exit 4) — ver L13.6 |
| `cargo nextest run -p webfang_mcp --test sitemap_crawl_run_staleness_test` | **`0 tests run`** (exit 4) — ver L13.6 |
| `cargo nextest run -p webfang_mcp --test export_coverage_test` | **`0 tests run`** (exit 4) — ver L13.6 |
| `cargo nextest run -p webfang_mcp --test scraping_coverage_test` | **`0 tests run`** (exit 4) — ver L13.6 |
| `git status --short` | sólo los 10 ficheros de código + el módulo nuevo; **ningún `.snap.new`**, ningún snapshot modificado |
| `cargo fmt --all -- --check` | exit 0 (sin salida) |

**L13.6 — La advertencia de L13.5: los cinco comandos MCP del encargo son verdes por vacuidad.**
Los cinco ficheros son `#![cfg(feature = "mcp")]` y la feature `mcp` **no** es default en
`webfang_mcp` ⇒ sin ella el binario de test se compila **vacío** y nextest sale con
`error: no tests to run` (exit 4). Es **exactamente la forma del FAIL-OPEN que S4 denuncia** en el
guard de duplicación ("no medí" confundido con "no hay nada"). La evidencia real es la misma
orden **con la feature**:

| Comando (con `--features mcp`) | Resultado |
|---|---|
| `cargo nextest run -p webfang_mcp --features mcp --test mcp_behavioral_test` | `27 tests run: 27 passed, 0 skipped` |
| `cargo nextest run -p webfang_mcp --features mcp --test cli_mcp_export_jsonl_parity_test` | `1 test run: 1 passed, 0 skipped` |
| `cargo nextest run -p webfang_mcp --features mcp --test sitemap_crawl_run_staleness_test` | `2 tests run: 2 passed, 0 skipped` |
| `cargo nextest run -p webfang_mcp --features mcp --test export_coverage_test` | `3 tests run: 3 passed, 0 skipped` |
| `cargo nextest run -p webfang_mcp --features mcp --test scraping_coverage_test` | `15 tests run: 15 passed, 0 skipped` |
| `cargo nextest run -p webfang_mcp --features mcp` (suite completa, no pedida) | `634 tests run: 634 passed, 0 skipped` (36 binarios) |
| `cargo nextest run -p webfang_core --test retention_test` (binario que ya incluía `common/mod.rs`; no pedido) | `19 tests run: 19 passed, 0 skipped` |
| `cargo check -p webfang_core --all-targets` | exit 0, sin avisos |
| `cargo check -p webfang_mcp --all-targets --features mcp` | exit 0, sin avisos |
| `cargo clippy -p webfang_core --all-targets -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | exit 0 |
| `cargo clippy -p webfang_mcp --all-targets --features mcp,chromium -- -D warnings …` | exit 0 |

⇒ **CERO cambios de snapshot** (los `.snap` de `mcp_behavioral_test.rs` —`export_jsonl_success`,
`export_vector_success`, `process_export_pipeline_success`, `export_file_success`— y los de
`sitemap_crawl_run_staleness_test.rs` siguen byte a byte igual: el `git status` no lista ningún
`.snap` y no hay ningún `.snap.new`).

**L13.7 — Dos efectos secundarios documentados** (no son defectos, pero cambian el coste de las
suites):

1. Los tres binarios de `webfang_core` que ahora incluyen `common/mod.rs` heredan también los
   **tests internos de `cli_harness`** (4 por binario), y uno de ellos **construye el binario
   `webfang`** la primera vez (~3 min; después, no-op). Antes esos binarios eran de 8 tests; ahora
   son de 11/24/32 según el fichero.
2. `common::init_ssrf_disabled` es ahora **una vez por proceso**: `common::start_test_server` ya no
   rearma las dos capas en cada llamada. Ningún otro fichero de `webfang_mcp` limpia esas variables
   y luego llama a `start_test_server` (el único que limpia es
   `mcp_ssrf_knob_matrix_test.rs`, que usa `start_test_server_ssrf_enabled`, que no rearma), así
   que la suite completa de la crate (634 tests) queda verde.

**L13.8 — Lo que este lote NO tocó**: `Cargo.toml` (de ninguna crate), `Cargo.lock`, `scripts/`,
`.github/`, `docs/`, `CHANGELOG.md`. No se commiteó nada (L1).

---

L14 — **Implementación de T6–T11** (la escribe el worker de esta unidad, sobre el mismo worktree
`fix/duplication-arrange-helpers`; el árbol queda **sin commitear**, L1). T1–T5 ya estaban
commiteadas (`b833fcef`, `b4b347a4`) ⇒ el árbol sobre el que se mide ya tiene el dedup aplicado.

**L14.1 — T6 · S3: el baseline baja al número MEDIDO, y la nota dice los tres números.**

| Árbol | `statistics.formats.rust.duplicatedLines` | Clones | Ficheros rust |
|---|---|---|---|
| antes (`main@89ff9d46`, `crates/` extraído con `git archive 89ff9d46 crates`) | **9009** | 839 | 479 |
| después (árbol con T1–T5, worktree `fix-duplication-arrange-helpers`) | **8515** | 811 | 480 |
| delta | **−494** | **−28** | +1 (el módulo nuevo) |

El **antes** se re-medió en esta unidad, no se copió: `git archive 89ff9d46 crates | tar -x -C
<tmp>` y el comando del gate sobre ese `crates/` ⇒ 9009 / 839 / 479, idéntico a la medición de
L6 y de L13.4. El **después** es la medición sobre el árbol de este worktree ⇒ 8515 / 811 / 480.
⇒ **La medición coincide exactamente con los 8515 de la estimación de la spec**: la cifra que el
encargo fijó (8515) es la medida, y la **estimación** de la spec (993) era la que se equivocaba
(499 líneas de exceso), como ya anticipaba L13.4 y la precedente de #1892.

- `scripts/quality-baselines.json`: `duplicated_lines_rust` **9272 → 8515**; `updated` →
  **2026-10-22** (`date -I`); `notes` → entrada nueva con la fecha, el comando exacto
  (`jscpd "$PWD/crates/" --min-tokens 50 --format rust --mode mild --silent --reporters json
  --output "$WORKDIR"`, leyendo `statistics.formats.rust.duplicatedLines`), el **antes** (9009), el
  **después** (8515), el **delta** (−494), la **versión del detector** (jscpd 5.4.0,
  kucherenko/jscpd v5) y el **motivo** (la extracción de los helpers de arrange), más la
  confirmación de que el 9272 estaba rancio en 263 líneas (el árbol ya medía 9009 cuando se
  escribió ⇒ el gate pasaba con 263 de margen sin que nadie hubiera deduplicado nada) ⇒ la
  bajada es una corrección de error de medición (cláusula (a)) **y** un paso descendente, no una
  relajación: el número escrito es el medido.
- El pin del detector (`jscpd 5.4.0`) queda **intacto**: no se tocó esa parte del guard.

**L14.2 — T7 · S4: el guard deja de tener el CERO POR DEFECTO.** La lectura del informe pasó de
"imprime 0 si no sé" a "cada causa tiene su estado y su mensaje". Los tres casos de vacuidad de
S4, reproducidos sobre un árbol de `mktemp` (copia del guard + copia del baseline + stub de
`jscpd`), con el mensaje **literal** que ahora imprime:

| Caso | Antes (pre-fix) | Ahora |
|---|---|---|
| informe sin `statistics.formats.rust` | `Duplicated lines (rust): 0` ⇒ `OK` ⇒ **exit 0** | `::error::El informe de jscpd no trae la sección 'statistics.formats.rust': el detector no produjo un informe de Rust (medición no realizada).` ⇒ **exit 1** |
| corpus con **cero** ficheros rust | `Duplicated lines (rust): 0` ⇒ `OK` ⇒ **exit 0** | `::error::jscpd no midió ningún fichero Rust (corpus con 0 ficheros): no hay medición sobre la que decidir.` ⇒ **exit 1** |
| `duplicatedLines` **no entero** | `int(val)` reventaba ⇒ traceback ⇒ exit 1 (diagnóstico feo) | `::error::'statistics.formats.rust.duplicatedLines' no es un número entero de líneas ('200'): la medición es inválida.` ⇒ **exit 1** |

Los **cinco** caminos que ya eran fail-closed se **verificaron** (no se tocaron): detector que
falla (`::error::jscpd exited with code 3`), versión equivocada (`::error::jscpd es 'jscpd
0.1.12', se esperaba 'jscpd 5.4.0' …`), `crates/` inexistente (`::error::jscpd exited with code
1`), baseline ausente (`::error::Missing baseline file …`) y baseline sin la clave (que antes
salía con un **traceback de Python** — veredicto correcto, diagnóstico feo — y ahora sale con
`::error::Missing baseline key 'duplicated_lines_rust' in …`, que **nombra la causa**). El
"baseline no-JSON" es un sexto caso del mismo grupo y ahora dice `::error::Baseline file … is not
valid JSON: …`.

**L14.3 — T8 · S4: el harness de negación (`scripts/test_duplication.sh`, nuevo).**

- **Hermético**: el detector real **no** se usa; el harness mete un **stub** de `jscpd` en el
  `PATH` que reproduce las dos cosas de las que depende el guard (la línea de `--version` y el
  fichero `<output-dir>/jscpd-report.json`) y el error del binario real cuando el corpus no
  existe. Motivo (restricción, no comodidad): el meta-guard corre en el job `repo-guards`, donde
  **no hay jscpd** (se instala en `code-quality`) ⇒ un harness que exigiera el binario real
  dejaría ese job rojo. El reparto es explícito: **el harness prueba la DECISIÓN del guard; la
  fidelidad del detector real la prueba el ratchet** que corre en `code-quality` (que este lote
  también baja). El harness corre el guard como **copia** dentro de un sandbox (su `ROOT` sale
  de su propia ubicación) ⇒ **no escribe en el repo**.
- **9 casos, 18 aserciones**, cada negativa con su **control positivo** en la misma arena:
  1. `Duplication INCREASED` ⇒ falla; *control*: el mismo informe con el baseline subido ⇒ pasa.
  2. informe sin sección `rust` ⇒ falla; *control*: el mismo corpus con la sección ⇒ pasa.
  3. corpus con cero ficheros rust ⇒ falla (en sus dos formas: `formats` vacío con `sources: 0`
     — la forma que escribe el detector real — y sección `rust` con `sources: 0`);
     *control*: un fichero medido ⇒ pasa.
  4. detector que falla (exit 3) ⇒ falla ruidosamente y **no** se lee como "0 duplicadas"
     (la aserción prohíbe explícitamente la cadena `Duplicated lines (rust): 0`);
     *control*: el mismo guard con un detector que sale bien ⇒ pasa.
  5. versión equivocada (`jscpd 0.1.12`) ⇒ rechazada; *control*: la versión fijada ⇒ aceptada.
  6. baseline ausente ⇒ falla; *control*: con el baseline de vuelta ⇒ pasa.
  7. baseline sin la clave / no-JSON ⇒ falla nombrando la causa; *control*: baseline bien formado
     ⇒ pasa.
  8. corpus inexistente ⇒ falla; *control*: corpus presente ⇒ pasa.
- **Salida**: `PASS=18 FAIL=0` (forma de `scripts/test_build_graph.sh`), `exit != 0` con un solo
  FAIL, y marcador `# NEGATION:` por caso (el que el meta-guard exige).

**Evidencia RED (real, contra el guard pre-fix `HEAD:scripts/check_duplication.sh`):** el
harness se ejecutó con la copia del guard **anterior** a T7 ⇒ **`PASS=13 FAIL=5`**, con estos
cinco fallos, que son exactamente el defecto y su diagnóstico:

```
FAIL a report without the rust section fails          expected FAIL, got rc=0
     Duplicated lines (rust): 0  (baseline: 100) / OK: duplication at or below baseline.
FAIL a corpus with zero Rust files fails              expected FAIL, got rc=0
     Duplicated lines (rust): 0  (baseline: 100) / OK: duplication at or below baseline.
FAIL a rust section without files fails               expected FAIL, got rc=0
     Duplicated lines (rust): 0  (baseline: 100) / OK: duplication at or below baseline.
FAIL a baseline without the ratchet key fails         the message does not name the cause
     KeyError: 'duplicated_lines_rust'   (traceback de Python)
FAIL a baseline that is not JSON fails                the message does not name the cause
     json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)
```

⇒ los **casos 2 y 3 dan verde contra el guard pre-fix** (vacuidad) y el **GREEN** posterior es
`PASS=18 FAIL=0` con el guard de T7.

**Control positivo de la unidad (el meta-guard):** con el guard sustituido por `exit 0`, el
harness da **`PASS=8 FAIL=10`** y sale con **1** ⇒ **ROJO**; con el guard intacto, `PASS=18
FAIL=0` ⇒ **VERDE**. El meta-guard `scripts/test_guard_coverage.sh` (que hace esa mutación él
mismo) sale **`OK: 18 wired guard(s) covered (mutation: on)`** ⇒ el harness quedó aceptado.

**L14.4 — T9 + T10: la baseline del meta-guard y el cableado en CI.**

- `scripts/guard_coverage_baseline.txt`: **borrada** la línea `no-harness check_duplication.sh …`
  (y el comentario de cabecera que decía "3 guards … duplication, orphan_snapshots,
  regression_naming, release_dispatch" pasa a "2 guards"). La lista sólo puede encogerse: el
  meta-guard falla con `stale-baseline` si la entrada deja de reproducirse.
- `.github/workflows/ci.yml`, job `repo-guards`: un step nuevo, *"Duplication ratchet semantics
  harness"* (`bash scripts/test_duplication.sh`), con el estilo de sus vecinos: nombre + el
  porqué en comentario (la vacuidad del guard) + la nota de que **no corre cargo ni red** (el
  detector va stubbeado). El job `code-quality` **no cambia**: ahí sigue corriendo el ratchet
  real contra `crates/`.

**L14.5 — T11: la verificación completa** (worktree `fix-duplication-arrange-helpers`, con
`CARGO_TARGET_DIR=$HOME/.cache/cargo-target/fix-duplication-arrange-helpers`,
`CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=1` y `$(rustup which cargo)`):

| Comando | Resultado |
|---|---|
| `bash scripts/check_duplication.sh` | `Duplicated lines (rust): 8515  (baseline: 8515)` ⇒ `OK: duplication at or below baseline.` (exit 0) |
| `bash scripts/test_duplication.sh` | `PASS=18 FAIL=0` (exit 0) |
| `bash scripts/test_guard_coverage.sh` | `guard coverage: 18 wired guard(s)` ⇒ `OK: 18 wired guard(s) covered (mutation: on)` (exit 0, **sin `stale-baseline`**) |
| el comando de medición del gate (L14.1) | antes 9009 / 839 / 479 ⇒ después **8515 / 811 / 480** ⇒ delta **−494** |
| `cargo check --workspace --all-features` | exit 0 (`Finished dev profile … in 2m 35s`) |
| `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | exit 0 (`Finished … in 3m 52s`) |
| `cargo fmt --all -- --check` | exit 0, sin salida |
| `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps` | exit 0 (`Finished … in 15.02s`) |
| `cargo nextest run -p webfang_core --test sitemap_integration --test sitemap_behavioral --test orchestrator_unit_tests` | `77 tests run: 77 passed, 0 skipped` (exit 0) |
| `cargo nextest run -p webfang_mcp --features mcp --test mcp_behavioral_test --test cli_mcp_export_jsonl_parity_test --test sitemap_crawl_run_staleness_test --test export_coverage_test --test scraping_coverage_test` | `48 tests run: 48 passed, 0 skipped` (exit 0) — **con `--features mcp`**, por la razón de L13.6: sin la feature los cinco binarios son `#![cfg(feature = "mcp")]` y nextest sale con `no tests to run` (verde por vacuidad) |

**L14.6 — Dos cosas del entorno, para que nadie las lea como defectos del lote.**

1. El meta-guard exige que **todos** los harnesses de los guards cableados estén verdes sobre el
   árbol intacto, y el de `scripts/test_intra_crate_direction.sh` necesita el extractor
   (`tools/intra-crate-lint`) **construido**: en este worktree no lo estaba ⇒ el meta-guard
   reportaba `broken-harness: scripts/test_intra_crate_direction.sh fails on an intact tree`.
   Es una **precondición ambiental**, no un efecto de este lote (el lote no toca ese harness ni
   ese guard); se resolvió como lo resuelve el propio job de CI, construyendo la herramienta
   (`env -u CARGO_TARGET_DIR cargo build --manifest-path tools/intra-crate-lint/Cargo.toml
   --locked`, que deja el binario en `tools/intra-crate-lint/target/debug/`, directorio
   gitignorado ⇒ el árbol sigue limpio). Con la precondición cumplida, el meta-guard sale verde.
2. `scripts/duplicate-crate-baseline.txt` tiene **cinco líneas obsoletas** (`console`, `dirs`,
   `dirs-sys`, `indicatif`, `redox_users`): es la baseline de otro guard y la mide el owner del
   plan con la baseline regenerada por L9 ⇒ **esta unidad no la tocó** (queda fuera de su
   superficie de edición), y se reporta aquí para que no se pierda.

**L14.7 — Lo que esta unidad NO tocó**: `crates/` (T1–T5 ya commiteadas), `Cargo.toml` de
ninguna crate, `Cargo.lock`, `docs/`, `CHANGELOG.md`, `scripts/duplicate-crate-baseline.txt`, y
el job `code-quality` de `.github/workflows/ci.yml`. La PR del lote es `Closes part of #1757`:
**no** cierra la issue entera (su AC 1 es el alcance de producción medido en P3/P4/P9, que queda
pendiente de decisión del owner). No se commiteó nada (L1).

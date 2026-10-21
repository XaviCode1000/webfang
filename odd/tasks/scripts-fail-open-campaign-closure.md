# Campaña de remediación fail-open — cierre de las olas 3 a 6

> Este doc cierra las olas 3 a 6 del plan global del audit
> `AUDIT-SCRIPTS-FAIL-OPEN-20261018-222141` (umbrella #2127). Las olas 1 y 2 están cerradas en
> `odd/tasks/release-fail-open-ola1-closure.md`.
>
> La regla de la campaña no cambió: un guard que no puede hacer su trabajo **falla cerrado**. Lo
> que cambió con las olas 3 a 6 es que el guard deja de *parecer* que puede hacerlo: el escáner
> awk del gate intra-crate llevaba meses ciego, y ningún test afirmaba lo que el gate decidía.

## Ola 3 — gates de TOML y Rust (B33–B39) · Ola 4 — seed + artefactos (B18–B31) · Ola 5 — policy + advisory (B32, B40–B44) · Ola 6 — gates de CI (B45–B61)

## Specs

- **S1** — "the inter-crate gate parses TOML by hand; the real graph is available" (B33–B35, audit).
  El gate inter-crate lee el grafo real (`cargo metadata --no-deps --offline`) y la identidad real
  del paquete, target, tier e inventario del workspace. Un miembro desconocido del workspace es un
  error nombrado, nunca un auto-allow.
- **S2** — "the intra-crate gate scans Rust with a text scanner" (B36–B39, audit).
  El escáner de texto se **borra**: el gate consume un extractor con `syn`, la política se queda en
  el shell, y **no hay fallback** — si el extractor no está o falla, el gate falla cerrado.
- **S3** — "the checker never verifies a pattern matched ≥1 site; a stale entry only emits
  `::warning::` and exits 0" (B44, audit).
  Una entry del allowlist que matchea **cero** sitios es un fallo, no un aviso. Se implementa una
  sola vez (en la Ola 5, sobre el gate con awk) y el flip la arrastra al gate nuevo.
- **S4** — "the seed chain's error paths degrade into 'no seed, build cold' with rc 0" (B18–B25, audit).
  Todo error-path de la cadena de seed falla cerrado: perfil derivado de la receta, digest de
  workspace obligatorio, modo de publicación restaurado, publish no-force que no reemplaza, clon
  verificado, política de strip leída y hasheada, y valores con newline que sobreviven al canal.
- **S5** — "the producers of artifacts can leave a publishable partial" (B21, B26–B31, audit).
  Render atómico con validación del conjunto esperado, fecha derivada de `SOURCE_DATE_EPOCH`,
  fences que cierran con su opener, el trace id como dato de jq, zip con replace atómico, y la
  expresión de licencia que no se acorta.
- **S6** — "the advisory parser takes the first quoted id on the line" (B40, B40b, B41, audit).
  El id capturado es el ligado a la clave `id`, con cualquiera de los dos estilos de comilla, y el
  header del TOML se reconoce con su comentario y su sangrado — sin relajar el fallo cerrado.
- **S7** — "the policy guards pass in silence" (B32, B42, B43, audit).
  Un `#[ignore]` en un comentario no rompe CI, las bases `support/*` exigen forma versionada, y un
  fallo de lectura no se degrada a "no hay nada que hacer".
- **S8** — "the CI gates decide less than they claim" (B45–B61, audit).
  Una base irresoluble ensancha a FULL en vez de caer en `HEAD~1`; los hooks con shebang se
  lintean; el lane FULL corre los harnesses del lane CI; "no pude escanear" no es "no encontré
  nada"; y todo timeout, lectura fallida o muestra truncada dice lo que pasó en vez de fingir.

## Tasks

| ID | Specs | Ruta | PR | Commit |
|---|---|---|---|---|
| T1 | S1 | `scripts/check_dependency_direction.sh` + harness | #2158 | `deae59cb` |
| T2 | S2, S3 | `tools/intra-crate-lint/` + `scripts/check_intra_crate_direction.sh` + harness | #2165, #2190 | `451d1b32`, `0ac8ebf5` |
| T3 | S2 | higiene del harness intra-crate (rename, marker `# NEGATION:`, wiring en CI) | #2159 | `0787fc80` |
| T4 | S2 | la violación real que el parser estricto revela: la constante del hatch SSRF al dominio | #2164 | `2f583ec1` |
| T5 | S4 | `scripts/seed_*.sh` + 6 harnesses | #2171 | `db29ca76` |
| T6 | S5 | `scripts/{build-llm-artifact,generate-third-party-notices,gen-cli-reference,analyze-trace,make-deterministic-zip}.sh` + 5 harnesses + `docs.yml` | #2170 | `46a86e75` |
| T7 | S6 | `scripts/check_advisory_policy.sh` + harness | #2169 | `ac370322` |
| T8 | S7 | `scripts/{check-ignored-guard,check-topology,check_intra_crate_direction,check_target_isolation}.sh` + 2 harnesses + `AGENTS.md` | #2174 | `97ef162f` |
| T9 | S8 | `scripts/{merge-when-green,apply-tag-ruleset,ci-health-close,ci_metrics}.sh` + 3 harnesses | #2176 | `c6cf92cd` |
| T10 | S8 | `scripts/{check_mcp_public_surface,check_concurrency_lints,check_dead_tests,check_archive_reproducibility,check_no_ai_attribution}.sh` + 5 harnesses + baseline encogido | #2172 | `7278f0ac` |
| T11 | S8 | `scripts/{ci_fast_gate,ci_path_classifier}.sh` + 2 harnesses | #2173 | `717b62e5` |
| T12 | S2, S8 | el gate local construye el extractor, los harnesses nuevos en el lane CI, y el job nuevo bloquea el merge | #2204 | `0236b295` |
| T13 | S1–S8 | certificación: las ocho clases del canon a 0 confirmed sobre `main` limpio, dos corridas idénticas | — (sin PR: es una medición) | `09050142` |

## Log

- **L1** (2026-10-21, petición del mantenedor): "tengo un agente mergeando las pr's abiertas continuamos con las siguientes olas?"
- **L2** (Ola 3, la decisión que cambió el plan): el gate intra-crate se escanea con `syn`, no con
  texto. La premisa del plan —que se podía corregir con un parser nuevo sin tocar el gate— era
  falsa: **el parser nuevo no ve más sitios que el escáner viejo; lo que cambia es que el escáner
  viejo deja de poder fingir**. La regla del shell (conjunto de capas, allowlist, veredicto) se
  queda; el shell consume el TSV del extractor y decide. El tool es un proyecto cargo **standalone**
  excluido del workspace: no toca el grafo de build del producto.
- **L3** (la unfinished business que el parser reveló): el gate llevaba meses ciego a una infracción
  REAL —`domain/ssrf_guard.rs:135`, un módulo de domain importando application— porque un
  doc-comment que menciona `#[cfg(test)]` truncaba el escaneo. La primera medición diferencial
  (awk 0 infracciones vs AST 1, con el allowlist count idéntico 44 = 44) es lo que demuestra que
  el conjunto AST reproduce los sitios compartidos y añade exactamente el que faltaba. La
  infracción se corrigió primero (#2164) y el flip después: al revés, el required check se habría
  puesto rojo por un defecto que llevaba meses en el árbol.
- **L4** (coordinación interna): **B44 se implementa una sola vez**. La Ola 5 lo implanta en el gate
  con awk (#2174) y el flip lo arrastra al gate nuevo. Implementarlo en los dos sitios sobre
  `scripts/check_intra_crate_direction.sh` creaba un conflicto de merge seguro. Por eso el flip
  espera a que #2174 esté en `main` y su rama se construye desde ese `main` con cherry-pick de
  los commits del worker (los merges son squash: su historia no se reutiliza tal cual).
- **L5** (un fallo real de un harness, no del código): los dos harnesses de wiring inyectaban su
  fixture con el **literal** de la lista narrow-scope del job `gate`. Al añadir `artifact-guards` a
  esa lista, el literal dejó de casar y el harness reportaba *"fixture injection did not apply —
  this case proved nothing"*: el harness se negaba a contar un caso que no probaba nada (bien
  hecho) y el CI quedaba rojo por un cambio de wiring legítimo. La inyección pasó a ser
  **semántica** (quita `repo-guards` de la primera lista `for job in`, esté donde esté) y los dos
  harnesses siguen teniendo dientes: contra un wiring roto fallan 2/12 y 2/8.
- **L6** (el fallo que el certificado de merge de la cadena destapó): #2164 movía código Rust, y el
  **golden set** del extractor es una medición del árbol → dejó de casar (1638 sitios contra 1630).
  Debajo había un segundo problema más fino: el test que probaba que el extractor ve lo que el
  gate awk no veía usaba como testigo una **infracción viva** del repo —justamente la que #2164
  arreglaba—, así que el testigo desaparecía al cerrar el defecto. Un testigo que se pudre cuando
  se cierra el defecto hace fallar la suite justo cuando el repositorio queda bien. El testigo
  pasó a ser un árbol **sintético** con el mismo punto ciego.
- **L7** (la lección de fondo, y la que gobierna todo el resto): **un fixture que dependa del árbol
  real es un temporizador**. El golden set del extractor, el contador de sitios del árbol y el
  testigo basado en una infracción viva son tres versiones del mismo error de diseño. Lo que
  sobrevive es lo que compara dos implementaciones **sobre el mismo árbol, en el mismo momento**:
  la aceptación del flip es la identidad byte a byte entre el gate nuevo y el gate awk (175 bytes,
  2 líneas, `cmp` sin diferencias), no un número memorizado.
- **L8** (un caso del canon que era **vacuo** y por lo tanto no certificaba nada): el caso B27 del
  canon de artefactos grepeaba el capítulo por una cadena que el fixture nunca escribía —`grep -c`
  imprime `"0"`, que es NO vacío, así que la condición era siempre cierta— y el fixture carecía de
  los marcadores que el script exige, así que el script abortaba antes de llegar al código del fix:
  reportaba CONFIRMED contra un script ya arreglado. Se reescribió con un discriminante limpio
  (un `$TMPDIR` inservible) y se verificó **en los dos sentidos**: RED contra el script pre-fix y
  GREEN con el arreglado. La regla: un caso del canon sólo vale si sabe decir "fixed"; hay que
  exigir RED contra el pre-fix antes de tocarlo.
- **L9** (un job de CI que corre pero no decide): `artifact-guards` y `merge-metrics-guards` son
  jobs que corren los harnesses nuevos, pero un job que **no está en `gate.needs`** deja el
  required context `CI Gate` en verde aunque el harness falle — la aserción existe, el gate nunca
  la ejecuta, y el fix se puede revertir en silencio. Los dos están en `needs` **y** en la lista de
  producers always-applicable del narrow scope (el job no depende del diff, así que en narrow scope
  su resultado tiene que contar).
- **L10** (el comportamiento que hay que conocer para trabajar en local): el gate **falla cerrado**
  si el extractor no está construido, por diseño (no hay escáner de respaldo). En CI no hay
  problema: el job `repo-guards` construye el extractor antes de correr el gate y el CI no fija
  `CARGO_TARGET_DIR`, así que el binario cae donde el gate lo busca. En local, con
  `CARGO_TARGET_DIR` pineado a un target dir compartido, hay que apuntar el gate con
  `$INTRA_CRATE_LINT_BIN` o construir con `env -u CARGO_TARGET_DIR` — y el mensaje de fallo dice
  las dos cosas.
- **L11** (la aritmética de la campaña): 61 hallazgos, seis olas, once PRs. Por clase, el canon
  queda en: release 15/0 · dep_direction 8/0 · intra_crate 11/0 · advisory 6/0 · policy 4/0 ·
  seed 11/0 · artifact 7/0 · CI 17/0. Los ocho clases a **cero** defectos confirmados.
- **L12** (proceso): los merges son squash y borran la rama remota, así que la historia de un PR no
  se puede reutilizar tal cual: el cherry-pick de los commits del worker sobre una rama nueva desde
  `main` es el mecanismo, y es lo que permite que el diff de un PR contenga sólo lo suyo. Las
  entregas que tocan `.github/workflows/ci.yml` se serializaron y cada una ancló sus steps en un
  job distinto, para que los merges salgan limpios sin depender del orden.
- **L13** (higiene del checkout compartido): una copia *untracked* de un doc que `main` ya tiene
  bloquea el fast-forward de la rama de `main` (el working tree se niega a sobrescribirla). Las
  copias se refrescan con `git checkout origin/main -- <ruta>` antes de editarlas, y un doc que
  cambia después de estar en `main` se modifica con un commit encima, nunca editando la copia
  local.
- **L14** (el matiz que costó un susto, y que conviene que quede escrito): **borrar la copia local
  de algo que `main` ya trackea no es borrar una copia: es borrar el fichero versionado.** El
  working tree queda con una eliminación (`D` en `git status`) que un commit desde ese worktree se
  llevaría, y el fast-forward de la rama `main` se queda atascado. La comprobación que faltaba no
  era "existe el fichero" —que es cierto también para un fichero trackeado— sino "**está
  untracked**", que es lo que dice `git status --porcelain`. Para apartar un untracked se mira
  primero el estado; para refrescar uno, `git checkout origin/main -- <ruta>`; y para lo que ya
  está en `main`, la vía es un commit encima, nunca el borrado local.

- **L15** (el certificado hay que renovarlo, y la regla que lo evita): el certificado de la
  campaña (79 casos, 0 confirmados, dos corridas idénticas) se emitió sobre `0236b295`. `main`
  avanzó nueve commits después —la release `v5.0.0` #2222, la declaración de la línea 5.0
  STABLE #2223, el contrato `TenantAiContext` retirado #2220, las trampas FFI/pacing
  #2211/#2213/#2216 y el router de `AGENTS.md` #2208—, y cinco ficheros de
  `crates/webfang_core/src` cambiaron: el **golden set** del extractor es una medición del
  árbol, así que era candidato a caducar. La cadena lo refrescó **en los mismos PR que movieron
  las líneas** (#2194 y #2213) → el `Repo guards` del CI quedó verde. Recertificado con el ritual
  entero sobre `09050142` (worktree detached limpio, extractor construido EN ese worktree, las
  ocho clases dos veces): **release 15/0 · dep_direction 8/0 · intra_crate 11/0 · advisory 6/0 ·
  policy 4/0 · seed 11/0 · artifact 7/0 · ci 17/0** — 79 casos, cero confirmados, dos corridas
  idénticas en cada clase. Dos reglas que quedan: **un certificado vale sobre el `main` que se
  certificó, no sobre el que vino después**; y **el golden set se refresca en el mismo PR que
  toca `src/`**, porque refrescarlo después es trabajo que el merge anterior dejó pendiente.

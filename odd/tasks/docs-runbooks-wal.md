# L14 — Docs y runbooks: la recuperación de WAL/SQLite y la nota de Obscura (#1609, #2061, #1826, #1710)

Plan padre: `odd/tasks/backlog-implementation-plan.md` (12 clusters, 16 lotes; vive en el branch
del Lote 2, `fix/merge-when-green-merged-state`, y viaja con él a `main`).
Este doc es la especificación de **un** lote: la documentación operativa que **no existe en
ningún lado**, más el triage de las tres issues de documentación que el plan dejó vivas. Sin
PR: el trabajo queda commiteado en el worktree.

Rama: `fix/docs-runbooks-wal` · Worktree: `~/Projects/Rust/webfang-worktrees/fix-docs-runbooks-wal`
Issues: #1609, #2061, #1826, #1710 (la parte de docs).

---

## Specs

**S1.** El plan padre define el lote (verbatim de su tabla):

> **L14** | Docs y runbooks | #1609, #2061, #1826 | cerrar #1609 con veredicto por fila y abrir ≤2
> slices vivas; **rechazar** el auto-PR de rotación (el gate `published` ya corre dentro de
> `Validate PR metadata`); el WAL/SQLite no tiene runbook en ningún lado |
> `state_store.rs::test_load_or_default_*` | +55/−40 | inline | PENDIENTE

**S2.** Autorización del owner (verbatim de lo que me relayed el owner del plan, sesión
`01a1607b`; **el texto original del owner no quedó capturado en esta sesión**, así que se cita
el texto del plan owner):

> docs/ autorizado (runbook WAL/SQLite + nota de #1710)

Con lo que `docs/` deja de estar fuera de alcance para este lote, y sólo para este lote.

**S3.** El runbook de WAL/SQLite: **cada afirmación del runbook tiene que estar pineada por un
test que ya existe** (el plan nombra `state_store.rs::test_load_or_default_*` como la evidencia).
Un runbook que afirma un comportamiento que ningún test cubre es una promesa nueva — la misma
regla que el repo le aplica al código. El worker tiene que **citar el test** de cada
afirmación que escriba, y si una afirmación no tiene test que la respalde, no la escribe: se
documenta como comportamiento no verificado, o se agrega el test (si el worker tiene la
superficie para hacerlo).

**S4.** Dónde vive: el runbook va al **libro** (`docs/src/`), con su entrada en
`docs/src/SUMMARY.md`: un runbook que no está en el índice es un runbook que nadie encuentra.
El worker tiene que verificar la convención leyendo `docs/src/SUMMARY.md` y una página
vecina antes de escribir, y no inventar una convención nueva.

**S5.** La nota de #1710 (la variante de build). El issue pide:

> Decisión documentada sobre la variante de build: exigir `-stealth` para sostener el claim
> anti-WAF, o aceptar L2 como "solo render barato" (actualizar `docs/src/cli-reference.md` y el
> comentario del módulo en consecuencia).

**Decisión (mía, como experto, con el owner delegándome las decisiones — S1 del plan padre:
"tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro producto")**:
**se acepta L2 como "solo render barato"** y se corrige lo que la documentación promete.

Fundamento: exigir `-stealth` agrega un verbo nuevo al contrato del binario (una variante
obligatoria del downloader) y con él una clase nueva de falla para los usuarios y para el
flujo de release, a cambio de sostener una **afirmación de marketing** (el claim anti-WAF). La
alternativa no agrega ninguna promesa: la documentación dice lo que L2 da. Un guard que exige
más de lo que el producto necesita es un guard que se vuelve un bloqueo sin salida (el mismo
razonamiento que el plan aplicó en D13 y D14).

Consecuencias de la decisión, que la nota tiene que decir:
- `docs/src/cli-reference.md` y el comentario del módulo quedan corregidos: L2 es un render
  barato, no una garantía anti-WAF.
- El hit-rate de L2 se **deriva de los logs por capa**; no hay contadores nuevos ni modo
  estricto (S5 del lote L11 y D-list del plan).
- El veredicto `Unknown` del preflight **nombra el mínimo esperado** (lote L11) y el piso
  vigente es **0.2.5** (el AC de #1710 que pedía 0.2.3 está obsoleto).
- #1710 **no se cierra** (D17): se comenta con el estado real.

**S6.** El triage de las tres issues de documentación (lo escribe el **padre**, no el worker:
el worker no toca GitHub):
- **#1609**: cerrar con **veredicto por fila** (qué fila está resuelta, qué fila no, y por
  qué) y abrir **≤2 slices vivas** — las slices que de verdad quedan, no un'issue' por fila.
- **#2061**: **rechazar** el auto-PR de rotación (D15): el scope `published` ya corre dentro
  de `Validate PR metadata` (fail-closed), así que un bot de issues/PR sería un escritor nuevo
  con su propia máquina de estados. Se responde en el issue con el fundamento.
- **#1826** y **#1710**: se **comentan, no se cierran** (D17).

**S7.** La parte de GitHub usa la API **REST** (`curl -sS https://api.github.com/repos/XaviCode1000/webfang/issues/<N> | jq .`): la GraphQL está floja en este momento.

**S8.** Build: este lote **no necesita compilar** (es documentación, y el cambio de Rust es un
comentario). Ningún `cargo build`/`check`/`nextest`. `cargo fmt --all -- --check` sí puede
correr si se toca un `.rs` (no compila: sólo parsea). Nada de `docs/` fuera de los archivos
nombrados en S4/S5, y nada de `CHANGELOG.md`.

---

## Tasks

- [ ] T1 · S3, S4 — runbook de recuperación de WAL/SQLite en `docs/src/`, con entrada en
  `docs/src/SUMMARY.md`, y **cada afirmación con el test que la respalda** citado.
- [ ] T2 · S5 — la nota de #1710: `docs/src/cli-reference.md` y el comentario del módulo
  corregidos a "L2 = solo render barato", con las cuatro consecuencias de S5.
- [ ] T3 · S6, S7 — evidencia para el triage del padre: el veredicto por fila de #1609
  (leído de la issue por REST), el argumento de #2061 y los comentarios de #1826 y #1710
  redactados para que el padre los publique.
- [ ] T4 · S1-S8 — verificación: los archivos existen, el SUMMARY enlaza, `cargo fmt --all --
  --check` si hubo `.rs`, y **diferencial**: ninguna otra página del libro quedó con una
  promesa que el runbook contradiga (el grep del libro por las promesas tocadas).
- [ ] T5 · S1-S8 — commit de work-unit en `fix/docs-runbooks-wal` (sin PR: la abre el owner
  cuando el plan entero esté implementado).

Ruta: **worker** (`gentle-ai-worker`) para T1-T2 y la redacción de T3; el padre publica en
GitHub y commitea. Motivo: el lote encadena lectura del `StateStore` y sus tests, escritura de
dos documentos y su verificación — varios pasos encadenados.

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

L2 — Reparto con el otro agente (sesión `01a1607b`): él toma L3 y las decisiones del owner que
tocan sus lotes; este lote es de docs y no colisiona con sus paths
(`ci-health.yml`, `test_ci_health_close.sh`, `ci-health-close.sh`, `ci.yml`).

L3 — Dónde vive hoy la información de persistencia: `docs/persistence-resume.md` existe (el
`--resume` del crawler) y `docs/src/` es el libro con `SUMMARY.md`. El runbook de **recuperación
de WAL/SQLite** (que es lo que #1826 y el AC de #1817 piden) no está en ningún lado: hay un
`docs/adr/0012-B-RUNBOOK.md` que es un ADR, no un runbook operativo. Por eso el lote existe.

L4 — El plan de este lote dice "inline" (ruta) y "+55/−40" (delta): el inline era una
estimación previa a que el owner autorizara `docs/` y a que se supiera que el triage de
#1609 exige lectura por REST. Con la autorización, la ruta pasa a worker y el padre hace las
escrituras en GitHub.

L5 — Trabajo del worker (route: `gentle-ai-worker`, superficies acotadas a cuatro archivos),
**verificado por el padre** (no por el informe): el runbook nuevo
`docs/src/state-recovery-runbook.md` con su entrada en `docs/src/SUMMARY.md`; la sección de
Obscura en `docs/src/cli-reference.md` **fuera** del bloque auto-generado (después de
`<!-- CLI-REFERENCE:END -->`, así que `gen-cli-reference.sh` no la pisa); y el comentario del
módulo de `obscura_downloader.rs`, que además tenía el **piso viejo** (0.2.0) contra el real
(0.2.5) — un hallazgo que el worker reportó y el padre verificó contra la constante.

L6 — **Corrección del padre (1/3): dos afirmaciones de la nota NO eran ciertas hoy.** La nota
decía que el veredicto `Unknown` "nombra el mínimo esperado"; el código de hoy dice
`"obscura_version_unreadable: --version probe failed or unparseable — continuing best-effort"`,
o sea que el mínimo lo nombra el **lote L11**, que todavía no aterriza. Una documentación que
promete lo que el código no hace es un defecto: si el owner abre la PR de L14 antes que la de
L11, la afirmación queda falsa en `main`. Se reescribieron las dos frases para que sean ciertas
hoy ("degrada a warning y la corrida sigue best-effort") y la consecuencia #2 de la lista
sigue siendo una consecuencia (la lista sigue teniendo cuatro). La frase que sí describirá el
naming del mínimo llega **con el código**, en el lote L11.

L7 — **Corrección del padre (2/3): el conteo de tools del libro.** `docs/src/overview.md`
decía "MCP server (35 tools)" mientras el número real y pineado es **36** (lo fija
`crates/webfang_mcp/tests/stdio_handshake.rs` y lo dice `AGENTS.md`). Corregido en este lote
(el drift de docs es de este lote; la consolidación del conteo en una sola fuente es de L7).

L8 — **Corrección del padre (3/3): una sugerencia de `troubleshooting.md` era falsa.** La
sección de WAF sugería "Try a different TLS fingerprint profile (`--tls-emulation`) **or JS
rendering**" contra un desafío WAF. Pero `hybrid_router.rs` lo dice: *"WAF detection at **any**
layer short-circuits with an error — escalation is never attempted against a WAF challenge"* →
nunca se escala a la capa que renderiza, así que el render con JavaScript no puede
esquivar un desafío. Se quitó esa mitad de la sugerencia y se dejó el hecho.

L9 — **El lado issues es del owner del plan** (reparto final: specs + libro mayor + lado issues
son suyos; toda la implementación es del padre). El worker redactó los comentarios de #2061,
#1826 y #1710 y el veredicto por fila de #1609 con sus ≤2 slices; el padre los revisó y los
dejó en `odd/docs-runbooks-wal/issue-drafts.md` para que el owner del plan los publique. Ninguna
de las cuatro issues se cierra: dos quedan comentadas con su estado real y #1609 se cierra con
veredicto por fila más las slices (su tabla de sub-issues está desactualizada: #1647 y #1622
ya están cerradas).

L10 — Verificación del padre, en el worktree: el diff toca exactamente cuatro archivos y las
superficies declaradas; `cargo fmt --all -- --check` en verde (el único `.rs` es un comentario);
la sección de `cli-reference.md` fuera del bloque auto-generado; el piso del comentario
(0.2.5) igual a la constante real; el conteo del libro (36) igual al pineado; la afirmación
del router sobre el WAF, verificada en `hybrid_router.rs`; y los enlaces del runbook resueltos
(sin `mdbook build`, que escribiría `docs/book/`, fuera de superficie).

L11 — **Corte de slice de este lote**: el branch suma ~650 líneas autorales repartidas en tres
commits (spec 157 · docs 284 · evidencia 209), cada uno debajo del corte de ~400. La estrategia
de entrega: **un PR por lote** (la documentación del libro y su evidencia ODD viajan juntas, como
en los lotes anteriores), re-aplicado sobre el `main` nuevo. Si el owner prefiere dividirlo al
abrir la PR, el corte natural es (1) spec + evidencia (`docs(odd)`) y (2) runbook + nota +
correcciones (`docs`). El orden de los PRs lo fija el owner (O7) y este lote **no** toca
`ci.yml`, así que no colisiona con la re-aplicación de los steps de L9/L7/L15.

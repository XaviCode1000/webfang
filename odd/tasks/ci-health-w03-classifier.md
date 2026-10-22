# El canal del CI Health deja de reportar los fallos reales

Objetivo: el `CI Health Observer` clasifica hoy **todo** run de "Production Bug Discovery" como
"Known FP only (W-03)", así que los hallazgos reales se reportan como known-false-positive y
nadie los ve. Y los trackers que él mismo abre son **huérfanos permanentes**: el título
retirado nunca se cierra y el título por origen sólo se cierra con el run del mismo origen.
Rama: `fix/ci-health-w03-classifier`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-ci-health-w03-classifier`.
Cierra: #2233 (el clasificador), #2162 y #1980 (los trackers huérfanos). Deja #1936 abierto con
el estado real (su contenido lo arregla el lote de hermeticidad, #2234).

---

## Specs

**S1.** El clasificador W-03 decide mirando una **línea de fallo**, no cualquier mención del
nombre del test (de #2233):

> La clasificación matchea una línea de **fallo** del test, no una mención de su nombre.

> Caso de harness: un log que sólo contiene la línea de filtro (sin fallo) ⇒ `known_only=false`;
> un log con la línea de fallo ⇒ `known_only=true`.

Causa verificada: `.github/workflows/ci-health.yml` hace
`gh run view "$RUN_ID" --log | grep -E "$KNOWN_RED_TEST"` sobre **todo** el log, y
`.github/workflows/bug-discovery.yml` imprime su propio filtro `-E 'not(test(... |
test(/sitemap_entry_max_pages_truncates_many_url_fixture/))'` ⇒ el nombre aparece aunque el
test no haya fallado ⇒ `known_only=true` en cada run.

**S2.** El RED conocido por W-03 sigue sin abrir issue por su cuenta, y un fallo real **sí** lo
abre (de #2233):

> El RED conocido por W-03 sigue sin abrir issue por su cuenta, y un fallo real sí lo abre.

**S3.** El observer cierra sus trackers por **subtítulo** de título, así que cualquier corrida
verde retira tanto el issue por origen como el del título retirado (de #2162 y #1980):

> Una única implementación de cierre que matchea un **subtítulo** de título (`"CI failed on
> main"` es subtítulo del título por origen y del título retirado) para que cualquier corrida
> verde de cualquier origen retire todos los issues con clave CI, sin agregar un segundo camino
> de cierre.

Causa verificada: `ci-health.yml` deriva la clave del **último** run (`TITLE_CI: "CI failed on
main ({origin}) [ci-health]"`) y el título retirado (`RETIRED_TITLE_CI: "Scheduled CI failed on
main [ci-health]"`) está marcado "detect, never close" ⇒ #1980 no tiene salida automática
(dead end) y #2162 necesita una corrida `workflow_dispatch` verde a mano.

**S4.** El contenido de esas dos corridas ya está arreglado en `main`: el UB de Miri lo cerró
#2187/#2211 (`54ebd481`) y el RUSTSEC-2026-0383 lo cerró #2148 (`1ec9369f`). Los dos issues son
deuda de bookkeeping, no defectos (de #2162 y #1980).

> El contenido de ambas corridas ya está corregido en `main` ... así que los issues son deuda
> de bookkeeping, no defectos.

**S5.** El harness del clasificador se cablea donde el observer corre, y su caso nuevo se ve
**RED** contra el clasificador roto (de #2233 + la disciplina del repo):

> Un caso del canon que no puede distinguir un script roto de uno arreglado no es evidencia.

---

## Tasks

- [x] T1 · S1, S2 — `.github/workflows/ci-health.yml`: el clasificador matchea una **línea de
  fallo** del test (no cualquier mención). Sin flags nuevos, sin estados nuevos: se cambia el
  patrón que decide.
- [x] T2 · S1, S2, S5 — `scripts/test_ci_health_close.sh` (o el harness que ya cubre el
  observer): caso nuevo "un log que sólo contiene la línea de filtro ⇒ `known_only=false`" y
  "un log con la línea de fallo ⇒ `known_only=true`", **RED** contra el clasificador roto.
- [x] T3 · S3 — el cierre del observer matchea el **subtítulo** `"CI failed on main"`, de modo
  que una corrida verde cierra tanto el issue por origen como el del título retirado. Sin un
  segundo camino de cierre.
- [x] T4 · S3, S4 — cerrar #2162 y #1980 con la evidencia (contenido ya arreglado en main) y
  comentar #1936 con el estado real: su texto ("Known FP only") describe el síntoma del
  clasificador, no el de la lane; el resto vive en #2234.
- [x] T5 · S1–S5 — verificación: el harness nuevo en verde, el harness existente del observer en
  verde, `bash -n` de los scripts tocados, y el RED demostrado (contra el clasificador roto).

### Borde de slice (registrado por el hook de pre-commit)

El hook mide 1163 lineas autorales en el commit de este lote: pasa el corte de ~400
lineas. El corte NO es de logica nueva, sino de volumen:

| Que | Lineas | Como se revisa |
|---|---|---|
| Logica nueva (el clasificador por linea de fallo, la clave de cierre, `reconcile_one` con open/close key) | ~140 en `.github/workflows/ci-health.yml` | se lee entero: es el cambio de comportamiento |
| Harness: 104 checks (3 secciones nuevas) | ~300 | se lee por seccion; los checks extraen y ejecutan el codigo real del workflow |
| Libreria nueva (`scripts/ci-health-fake-gh.sh`) | ~474 | es un **movimiento** de la maquinaria que ya estaba en el harness, mas la semantica real de la busqueda `in:title` |

La regla del repo es "tests y docs van con el comportamiento", asi que partir el
harness de su workflow dejaria una PR con checks que fallan. Si el owner quiere el
corte, la via es **entrega secuencial** (este repo no tiene PRs apiladas):

1. slice 1: el clasificador por linea de fallo (workflow + la seccion del harness que
   lo cubre + la libreria nueva),
2. slice 2 desde el `main` nuevo: el cierre por subtitulo (workflow + su seccion).

O una PR sola, que para un revisor humano es mas barata: el grueso del diff es un
movimiento y el nucleo a revisar son ~140 lineas. La decision es del owner (O7).

## Log

L1 — Petición del owner (verbatim):

> tengo muchas y  varias issues abiertas y necesito un plan para ir implementando utliza la
> skill adecuada para triage de issues

L2 — Corrección del owner sobre el estado del sistema (verbatim):

> debes tener en cuenta que nuesto sistema ha cambiado sobre todo en lo relacionado con
> modelos de *AI LOCAL* Y LAS ISSUES pueden decir cosas o indormacion antigua o falsa,

> utiliza siempre tu y ordenar a  todos los workers las potentes tools *codedb* y *codegraph*
> en lugar de las otras tools

L3 — Reparto y formato de entrega (verbatim):

> ya tengo un agente trabajando en lote k1 en
> /home/xavi/Projects/Rust/webfang-worktrees/fix-ssrf-this-network-justification

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

L4 — Evidencia del triage (worker del set D + verificación propia del padre sobre
`main@3a785d8e`):
- el clasificador y el filtro están como se describen en S1 ⇒ `known_only=true` en cada run;
- hay 7+ corridas de Bug Discovery rojas desde 2026-10-14 con fallos genuinos, etiquetadas
  "Known FP only (W-03)" ⇒ el canal de reportes está enmudecido;
- `RETIRED_TITLE_CI` está marcado "detect, never close" ⇒ #1980 es un dead end sin salida
  automática, y #2162 sólo se cierra con una corrida `workflow_dispatch` verde a mano;
- el contenido de ambas corridas ya está arreglado en `main` (#2187/#2211 y #2148) ⇒ son deuda de
  bookkeeping.

L5 — Implementación (rama `fix/ci-health-w03-classifier`):

- **T1/S1** — el step "Classify Bug Discovery result" matchea la LÍNEA DE FALLO de nextest
  (`FAILURE_LINE_RE`: `FAIL [ ... ] <test>`), la misma forma que greppea `bug-discovery.yml`
  en "Publish failure names". El nombre del test ya no basta: el filtro de nextest que
  `bug-discovery.yml` imprime en su log contiene el nombre, y por eso matchaba en cada run.
- **T3/S3** — el cierre del observer usa la clave `CI_CLOSE_KEY: "CI failed on main"`, un
  subtítulo del título por origen Y del título retirado. `reconcile_one` recibe ahora la
  clave de cierre como parámetro (7º) y el step de upsert cierra con la clave de cierre:
  una sola implementación (`close_mine`), sin segundo camino de cierre.
- **T2/S1,S2,S5** — el harness de `scripts/test_ci_health_close.sh` pasó de 96 a 104 checks:
  (a) el `gh` falso modela ahora la búsqueda real de GitHub (`in:title` ANDa palabras, no
  compara byte a byte) porque el cierre por subtítulo depende de esa semántica; (b)
  clasificación W-03 ejecutada contra dos logs-fixture; (c) el upsert ejecutado contra el
  `gh` falso (un fallo real se abre para triage, el RED conocido se etiqueta
  `known-false-positive`); (d) el cierre por subtítulo, tanto por el helper como por el
  camino de cierre real del workflow.
- **RED demostrado**: con el patrón viejo restaurado en `ci-health.yml` el caso nuevo falla
  (`known_only=true` con un log que sólo tiene la línea de filtro); con el cierre viejo
  (título por origen) el tracker del título retirado no se cierra. Con el fix: 104/0.
- **T4/S4** — #2162 y #1980 cerrados con la evidencia (`54ebd481` y `1ec9369f`, ambos
  ancestros de `origin/main`); #1936 comentado con el estado real y **abierto** (su contenido
  lo arregla #2234).
- **Hallazgo colateral**: el título real de #1980 es "Scheduled CI failed on main" **sin** el
  sufijo `[ci-health]`, así que el watcher de la clave retirada (que busca el título
  completo) nunca lo vio: sólo el cierre por subtítulo lo alcanza.

# L15 — Lane de UB: que el detector de Miri vuelva a correr — Tasks

Spec: `odd/tasks/miri-ub-lane.md` (#2235: el UB de `scraper_service.rs` con gate por test, cada
skip con motivo escrito, y la lane dejando resultado en vez de abortar el proceso). Plan padre:
`odd/tasks/backlog-implementation-plan.md` (vive en el branch del Lote 2,
`fix/merge-when-green-merged-state`).

- [x] T1 · S1, S3 — el test hermano de `scraper_service.rs` que dispara el UB (familia
  `servo_arc`/`lol_html`) queda gateado **por test** con
  `#[cfg_attr(miri, ignore = "<motivo escrito>")]`, como el gate de #2187.
- [x] T2 · S1, S4 — los tests que de verdad necesitan FFI/asm en `wreq_downloader`,
  `obscura_downloader` y `chromiumoxide_downloader` quedan gateados **por test** con su motivo,
  y los tres `--skip` de submódulo **se eliminan** de los jobs Miri.
- [x] T3 · S5 — el bloque de comentario de diseño de los jobs Miri describe el mecanismo nuevo
  (gate por test con motivo; sin skip-net) y por qué el UB queda atribuido a un test.
- [x] T4 · S7 — harness estático (shell, sin build) que fija: todo skip de un job Miri con motivo
  escrito y test asociado; todo `#[cfg_attr(miri, ignore)]` con motivo no vacío; y el filtro de
  `Miri (core)` cubriendo `application::`. Con su caso RED pre-fix. El harness invoca el guard
  como **subproceso** (`MIRI_LANE_ROOT` + `MIRI_LANE_CHECK`, una invariante por fixture), no
  como librería: un guard cargado con `.` hereda su `exit`, y un guard sustituido por `exit 0`
  se llevaría el harness por delante y sus controles RED no correrían. El par cumple el
  contrato del meta-guard (#2056): marcador `# NEGATION:` y **rojo** con el guard sustituido.
- [x] T5 · S6, S8 — verificación: harness estático en verde, y scoped Miri con el toolchain
  pinneado (`nightly-2026-08-27`) y los flags de la lane: el módulo del UB completa y los
  submódulos antes skipeados corren. La corrida schedule completa la da el CI al abrir la PR.
- [x] T6 · S1-S9 — commit de work-unit en `fix/miri-ub-lane` (sin PR: la abre el owner cuando el
  plan entero esté implementado).
- [x] T7 · S7 — el par guard/harness cumple el meta-guard `scripts/test_guard_coverage.sh`:
  `bash scripts/test_guard_coverage.sh` termina en `OK: 19 wired guard(s) covered (mutation:
  on)`. Antes de este arreglo el job `Repo guards` era rojo por dos violaciones del meta-guard
  sobre este harness (`no-marker` y `no-mutation`), que son las dos caras del mismo defecto:
  el harness no llevaba marcador de caso negativo y, al cargar el guard con `.`, no podía
  ponerse rojo con un guard roto. El arreglo encontró además un defecto mayor del propio
  guard: ejecutado como programa terminaba **siempre con exit 0** (el último comando del
  runner era un `printf`), así que `bash scripts/check_miri_lane_invariants.sh` no podía
  fallar nunca — sólo la carga como librería dejaba ver el fallo. El runner ahora cierra con
  `exit "$real_rc"` y el harness lo invoca como subproceso, con `MIRI_LANE_CHECK` para aislar
  una invariante por fixture.

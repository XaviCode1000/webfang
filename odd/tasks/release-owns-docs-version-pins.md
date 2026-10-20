# Feature: el flujo de release es dueño de los pines de versión

**Issue:** #2028 (`status:approved` ya presente en el issue)
**Labels:** `type:chore` en el issue; el PR decidirá su label propio (mapping
commit→label en AGENTS.md § "PR creation")
**Branch:** `fix/release-pins` (base `main`; `check-topology.sh` admite `fix/*`
→ `main` ✓; worktree hermano `webfang-worktrees/fix-release-pins` — el proxy
directorio↔rama del AGENTS.md exige que el directorio sea la rama con `/` → `-`)
**Base:** `main @ 3e315e64` (workspace `3.0.0`)
**Ruta de entrega:** `single-pr`. Este repo no admite stacked PRs (AGENTS.md §
"No stacked PRs in this repo"): el cambio es un solo comportamiento y se commitea
en 2 work units revisables, ambos sobre `main`.
**Test runner:** bash puro — harnesses offline sobre fixtures `mktemp`
(`scripts/test_*.sh`), el gate `scripts/check_docs_version_pins.sh`, y la lane
CI-only de `scripts/ci_fast_gate.sh`. **No hay código Rust en este feature**: no
aplica `cargo check` / `clippy` / `nextest`.

## Objetivo

Que el bump de los pines de versión en `README.md` y `docs/src/*.md` lo haga el
flujo que corta el release, no un humano.

Hoy el step `Repo guards → Docs version pins gate` es **rojo en cada Release
PR**: release-plz bumpea `[workspace.package] version` en `Cargo.toml` y no
toca la narrativa, así que el gate —que compara los literales `vX.Y.Z` de la
narrativa contra la versión del workspace— falla por construcción. El PR queda
"estructuralmente inejecutable" sin un commit manual. Y ese commit manual es
efímero: release-plz **regenera** la rama del Release PR en cada push a `main`
(advance no rápido: `67a30441` → `921f7a05`), así que borra el bump humano y el
PR vuelve a rojo. Medido tres veces en una tarde (v2.12.0 ×2, v3.0.1 ×1).

## Specs

- **S1** — "Un Release PR (`chore/release-*`) puede quedar verde sin intervención
  humana, por (a) o por (b)." (issue #2028, acceptance criteria)
  → Se implementa **(a): el flujo de release es dueño del bump**.
  La opción (b) —exención por rama en el gate— queda **descartada** por el
  comentario del owner: *"La exención `chore/release-*` que tiene
  `pr-validation.yml` **no** debería replicarse en `repo-guards`: eximir sólo
  mueve el rojo a `main` tras el merge, sin arreglar los pines."*

- **S2** — "La decisión queda escrita donde un futuro agente la encuentre
  (AGENTS.md § Releases, o el header de `scripts/check_docs_version_pins.sh`)."
  → Se escribe en **ambos** lugares.

- **S3** — "Si es (b), el gate sigue fallando para ramas normales: el harness
  (`scripts/test_docs_version_pins.sh`) cubre el caso `chore/release-*`."
  → Al ser (a), el gate **no se debilita**: sigue fallando para toda rama,
  incluidas las de release. `scripts/check_docs_version_pins.sh` no cambia de
  semántica; sólo su header (S2).

- **S4** — "Cualquier bump de pines pushed a esa rama es ***efímero***: la
  siguiente corrida del bot lo borra y el PR vuelve a rojo." (comment
  `#issuecomment-6282505559`)
  → El bump lo aplica **el propio flujo de release**, en la misma corrida que
  regenera la rama, y es **idempotente**: se reaplica en cada corrida, sin
  importar cuántas veces release-plz regenere la rama.

- **S5** — "Durante un Release PR los pines no pueden coincidir con el workspace
  ya bumpeado, así que el PR es estructuralmente inejecutable sin ese paso
  manual." (comment)
  → El PR queda ejecutable sin pasos manuales.

- **S6** — El escritor de pines es el **inverso exacto** del gate: después de
  correr, `scripts/check_docs_version_pins.sh` **pasa** sobre ese árbol.
  Respeta el marcador `<!-- version-stale:allow -->` (línea completa), es
  idempotente, no toca ningún archivo fuera de `README.md` + `docs/src/*.md`, y
  **falla cerrado** ante una línea ambigua (dos pines distintos en la misma
  línea) sin escribir nada.

- **S7** — Los harnesses nuevos están **cableados** en `.github/workflows/ci.yml`
  (job `repo-guards`, sin `paths:` y sin `if:`) y en la lane `lane_ci` de
  `scripts/ci_fast_gate.sh`. Un harness que nadie corre es exactamente la clase
  de defecto de #2017 (guard semántico correcto, sin edges: inerte un release
  entero).

- **S8** — El push a la rama del Release PR usa el **App token**
  (`steps.app-token.outputs.token`), nunca `secrets.GITHUB_TOKEN`: sólo el App
  token dispara los eventos `pull_request` que producen el CI del Release PR
  (con `GITHUB_TOKEN` el push no dispara workflow → el head SHA se queda sin
  checks → branch protection bloquea el merge).

- **S9** — El gate no se toca más que en su header: **no** hay ventana en vuelo,
  **no** hay exención por rama, **no** se afloja la comparación. El árbol de
  `main` después del merge del Release PR tiene los pines correctos porque el
  Release PR los traía — no porque el gate los tolere.

## Decisiones de diseño (con su por qué)

1. **El flujo de release empuja los pines a la RAMA del Release PR**, no a
   `main`. Razón: el Release PR es el PR que mueve la versión; los pines tienen
   que moverse en el mismo PR, o `main` queda con pines stale entre el bump y el
   merge — que es exactamente el defecto que la opción (b) "mueve a `main`".
2. **Un commit aparte, no un `--amend` del commit de release-plz.** Razón:
   release-plz regenera la rama desde `main` en cada push; su commit se
   reescribe y el nuestro se reaplica encima. Un amend sobre un commit que el
   bot va a reescribir es trabajo perdido igual, y encima pelearía con el bot
   por la rama.
3. **Sin ventana en vuelo en el gate.** Una tolerancia "los pines pueden lagging
   una release" abriría un hole: un PR cualquiera que bumpee la versión del
   workspace y no toque la narrativa pasaría el gate. El costo de NO tenerla es
   **un ciclo de CI extra por push a `main` mientras el Release PR está
   abierto** (el push de release-plz deja los pines stale → CI rojo → el paso
   nuevo los arregla → CI verde). Ese costo ya existe hoy con el re-bump
   manual: **el fix no agrega ciclos de CI, les quita el paso humano.**
4. **El escritor falla cerrado ante una línea ambigua.** Dos pines distintos en
   la misma línea no se pueden reescribir sin inventar prosa ("de v2.4.0 a
   v3.0.0" → "de v3.0.1 a v3.0.1"). El gate ya exige que esa línea esté marcada
   como historia; el escritor se niega a adivinarla y lo dice con
   `archivo:línea`.

## Contratos (lo que el implementador debe escribir)

### T1 — `scripts/bump_docs_version_pins.sh` — el escritor

Espejo exacto de `scripts/check_docs_version_pins.sh` (mismo conjunto de
archivos, mismo marcador, misma gramática de versión), pero en vez de comparar
reescribe. Idioma: **español**, como la familia del gate.

- Raíz: `ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"` — el script
  opera sobre **su propio** repo (así funciona dentro del worktree de la rama
  del Release PR).
- Versión: se lee igual que el gate —
  `sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1`
  — y debe_validar `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$`.
  Sin manifest, sin versión o con formato inesperado → **fallo cerrado**
  (exit 1) nombrando el motivo.
- Archivos: `README.md` + `find docs/src -name '*.md' -type f` (idéntico al
  gate). Si no hay ningún documento → fallo cerrado.
- Gramática de pin: `v` + la misma `VER_ERE` del gate (incluye el sufijo
  `-rc.N`; el sufijo se compara entero, no se pela).
- Por línea:
  - si la línea contiene `<!-- version-stale:allow -->` → **se salta entera**
    (mismo alcance que el gate: el marcador es de línea);
  - pines distintos en la línea = 0 → no hace nada;
  - = 1 → se reescriben **todas** sus ocurrencias a `v$VERSION`;
  - > 1 → **fallo cerrado** nombrando `archivo:línea`, y **no se escribe
    nada** (todo o nada: el árbol queda intacto).
- Idempotente: si el contenido resultante es idéntico al original, no escribe
  nada y lo dice.
- Salida: una línea por cambio (`archivo:línea  vviejo → vnuevo`) y un resumen
  final estilo `OK: N líneas actualizadas a vX.Y.Z en M documentos` o
  `OK: los pines ya declaraban vX.Y.Z (sin cambios)`.
- Exit 0 ok · 1 fallo (con `::error::` en stderr, como el gate).
- **No** toca ningún otro archivo (ni `CHANGELOG.md`, ni `Cargo.toml`).

### T2 — `scripts/apply_release_pins.sh` — el orquestador del flujo

Es el paso que el workflow invoca. Idioma: **inglés**, como la familia de
release (`check_release_dispatch.sh`, `verify-release-tag.sh`).

- Corre desde el checkout del job (raíz de `main`). `RELEASE_PINS_REPO_ROOT`
  (default: raíz del propio script), `RELEASE_PINS_BRANCH_PREFIX` (default:
  `chore/release-`), `RELEASE_PINS_REMOTE` (default: `origin`) permiten
  ejercitarlo offline.
- **Descubrir el Release PR**: `gh pr list --state open --json number,headRefName,updatedAt`
  → se queda con los que tienen `headRefName` con el prefijo → elige el de
  `updatedAt` más reciente. Si no hay ninguno → aviso y **exit 0** (release-plz
  decidió que no hay release que preparar: no hay nada que hacer).
- **Re-leer el PR antes de empujar**: `gh pr view <n> --json state` → si ya no
  está `OPEN` → aviso y exit 0 (se mergeó mientras tanto).
- **Fetch** de la rama: `git fetch --no-tags "$REMOTE" "+refs/heads/$BRANCH:refs/remotes/$REMOTE/$BRANCH"`.
- **Worktree** detached en un directorio temporal (`mktemp -d`, fuera del
  repo) sobre la rama; el `trap` lo quita (`git worktree remove --force`).
  Nada de `git checkout`/`git switch` (prohibidos por AGENTS.md).
- **Correr el escritor** dentro del worktree: `bash scripts/bump_docs_version_pins.sh`
  (el script del worktree resuelve su propia raíz).
- **Si no cambió nada** → mensaje "sin cambios" y exit 0 (idempotencia: no hay
  commit, no hay push, no hay CI disparado).
- **Si cambió** → `git add -A` (sólo los archivos del worktree cambiaron),
  commit con identidad explícita
  (`git -c user.name='github-actions[bot]' -c user.email='bot@github.com'`)
  y mensaje `chore(docs): actualizar los pines de versión a vX.Y.Z`
  (sin trailers, sin atribución a IA — `scripts/check_no_ai_attribution.sh`
  escanea el bloque de trailers), y push a `refs/heads/$BRANCH`.
- **Auth del push**: encabezado `http.extraheader` con el token de
  `GITHUB_TOKEN` (App token). El token **no** va en la URL del remote ni queda
  en `.git/config`. Si el remote es local (ruta) el harness puede correr sin
  token; si el remote no es local y no hay token → **fallo cerrado**.
- **Un reintento**: si el push es rechazado (no fast-forward, porque release-plz
  regeneró la rama entre el fetch y el push), re-fetch + re-correr el escritor +
  re-push **una** vez; si vuelve a fallar → exit 1 con el diagnóstico.
- Imprime el SHA resultante.

### T3 — `.github/workflows/release-plz.yml` — el cableado

- Un paso nuevo **después** del paso `Run release-plz` del job `release-plz-pr`
  (el orden importa: el PR del Release tiene que existir antes de buscarlo).
- Nombre del paso: `Own the docs version pins`.
- Corre `bash scripts/apply_release_pins.sh`.
- Token: `GITHUB_TOKEN: ${{ steps.app-token.outputs.token }}` (App token, S8) y
  `GH_TOKEN: ${{ steps.app-token.outputs.token }}` para el `gh` del script.
  **Prohibido** `secrets.GITHUB_TOKEN` en este archivo (lo niega
  `scripts/check_release_dispatch.sh`, check 3).
- El job conserva `permissions: contents: write` + `pull-requests: write`.

### T4 — Harnesses

- `scripts/test_bump_docs_version_pins.sh` — semántica del escritor. Offline,
  fixtures `mktemp`, **nunca** toca el repo real. Casos mínimos:
  1. pin stale en `README.md` → reescrito a la versión del workspace;
  2. pin stale en `docs/src/*.md` → reescrito;
  3. línea con `<!-- version-stale:allow -->` → **no** se toca;
  4. el marker en OTRA línea no exime a esta → la línea sin marker se
     reescribe y la marcada queda igual;
  5. idempotencia: la segunda corrida no cambia nada y sale 0;
  6. árbol ya consistente → sin cambios, exit 0;
  7. línea con **dos** pines distintos → **fallo cerrado** (exit 1), el árbol
     queda **intacto**;
  8. manifest ausente / sin versión / versión con formato inesperado → fallo
     cerrado;
  9. sin documentos → fallo cerrado;
  10. workspace `X.Y.Z-rc.N` → el pin se reescribe a `vX.Y.Z-rc.N` (sufijo
      intacto);
  11. **propiedad inversa**: tras correr el escritor sobre un fixture
      desviado, `scripts/check_docs_version_pins.sh` **pasa** sobre ese fixture
      (es el caso que ata los dos scripts);
  12. el escritor **no** toca un archivo ajeno (p.ej. `CHANGELOG.md` con un pin
      stale queda intacto).
  Estilo: marcador `# NEGATION:` por caso (qué se rompe sin el fix), resumen
  `printf '\nharness: %d ok, %d fail\n'`, salida no-cero si algún caso falla
  (convención de `scripts/test_docs_version_pins.sh`).
- `scripts/test_apply_release_pins.sh` — semántica del orquestador. Offline:
  remote bare local + clon + rama `chore/release-...` + `gh` stub en el PATH.
  Casos: (a) camino feliz → la rama remota queda con los pines del workspace y
  **exactamente un** commit nuevo; (b) segunda corrida → **ningún** commit
  nuevo (idempotencia); (c) no hay Release PR abierto → exit 0 y la rama
  intacta; (d) el escritor falla (línea ambigua) → el orquestador sale
  no-cero y **no empuja nada**.
- `scripts/test_release_pins_wiring.sh` — el wiring, al estilo de
  `scripts/test_docs_version_pins_wiring.sh` (casos + **controles
  negativos** con fixtures mutados):
  1. el `release-plz.yml` commiteado invoca `scripts/apply_release_pins.sh` en
     el job `release-plz-pr`;
  2. ese paso va **después** del paso `Run release-plz`;
  3. el token del paso es el App token (`steps.app-token.outputs.token`), no
     `secrets.GITHUB_TOKEN`;
  4. `repo-guards` corre los tres harnesses, sin `paths:` y sin `if:`;
  5. `lane_ci` del fast gate los corre, con un trigger que los matchea;
  6. control negativo: borrar el paso del orquestador de `release-plz.yml` →
     el harness falla y nombra el archivo;
  7. control negativo: cambiar el token por `secrets.GITHUB_TOKEN` → el
     harness falla y nombra el token;
  8. control negativo: borrar un harness de `ci.yml` → el harness falla.
- **Cableado de los harnesses** (S7):
  - `.github/workflows/ci.yml`, job `repo-guards`: un paso por harness, con el
    mismo estilo de comentario que los pasos vizinhos (por qué corre en EVERY
    PR, por qué es offline).
  - `scripts/ci_fast_gate.sh`, lane `lane_ci`: `run_step` por harness, con el
    trigger (la unión de archivos cambiados) que matchea los scripts nuevos y
    `.github/workflows/release-plz.yml`.

### T5 — La decisión, escrita donde un agente la encuentre (S2)

- `AGENTS.md`, sección **"Release automation"**: un párrafo (o subsección corta)
  que diga, sin ambigüedad: **quién** bumpea los pines (el flujo de release,
  paso `Own the docs version pins` del job `release-plz-pr`), **por qué** el
  bump manual no es un lugar estable (release-plz regenera la rama), **por qué**
  el gate NO tiene exención por rama (mueve el rojo a `main` sin arreglar los
  pines) y **dónde** vive el escritor.
- `scripts/check_docs_version_pins.sh`: sólo el header — una nota que apunte
  al dueño del bump y a `scripts/bump_docs_version_pins.sh`. **Cero cambios de
  comportamiento** en el gate.

## Tasks

| ID | Specs | Ruta | Entrega | Commit |
| --- | --- | --- | --- | --- |
| T1 | S6 | delegada (worker) | `scripts/bump_docs_version_pins.sh` + `scripts/test_bump_docs_version_pins.sh` | `96aaa5b4` |
| T2 | S4, S5, S8 | delegada (worker) | `scripts/apply_release_pins.sh` + `scripts/test_apply_release_pins.sh` | `8a309aa9` |
| T3 | S1, S8 | delegada (worker) | `.github/workflows/release-plz.yml` | work unit 3 |
| T4 | S7 | delegada (worker) | `scripts/test_release_pins_wiring.sh` + `ci.yml` + `ci_fast_gate.sh` | work unit 3 |
| T5 | S2, S9 | delegada (worker) | `AGENTS.md` + header de `check_docs_version_pins.sh` | work unit 3 |
| T6 | S1..S9 | inline (padre) | verificación, commits por work unit, assessment de riesgo | — |

**Orden dentro del writer**: T1 → T2 → T3 → T4 → T5 (T4 y T5 necesitan que T1/T2
existan para poder referenciarlos).

## Log

- **L1** (2026-10-20, pedido del usuario, verbatim):
  > "vamos a analiza a fondo la issue #2028 y su comentario para implementar una
  > solucion final.
  >
  > utiliza context7 para documentacion y *JINA --help* si necesitas busqueda web"

- **L2** — Decisión de diseño: se implementa **(a)**, el flujo de release es dueño
  del bump. La (b) queda descartada por el comentario del owner (#2028,
  `#issuecomment-6282505559`): una exención por rama "sólo mueve el rojo a
  `main` tras el merge, sin arreglar los pines".
- **L3** — Evidencia de que release-plz **no** puede editar los pines: la
  documentación oficial (`release-plz.dev/docs/config`) no expone ninguna opción
  de archivos extra en el Release PR; el patrón que la propia upstream
  documenta (`/docs/github/output.md`, "Committing Files to Release PR") es
  `persist-credentials: true` + commit posterior — es decir, exactamente el
  diseño de T2/T3.
- **L4** — Decisión de diseño: **sin ventana en vuelo** en el gate. La tolerancia
  "los pines pueden lagging una release" abriría un hole para cualquier PR que
  bumpee la versión del workspace sin tocar la narrativa. Costo aceptado: un
  ciclo de CI extra por push a `main` mientras el Release PR está abierto (costo
  que ya existía con el re-bump manual).
- **L5** — Decisión de diseño: el push usa el **App token** con
  `http.extraheader`. Un push con `GITHUB_TOKEN` no dispara workflow → el head
  SHA del Release PR se queda sin checks → branch protection bloquea el merge.
  Además, `scripts/check_release_dispatch.sh` (check 3) prohíbe
  `secrets.GITHUB_TOKEN` en `release-plz.yml`.
- **L6** — Medición sobre `main @ 3e315e64`: el gate pasa y hay **3** líneas con
  pines en **11** documentos — `README.md:39` (`VERSION=v3.0.0`),
  `docs/src/installation.md:67` (`Observed output for \`v2.11.0\`: <!--
  version-stale:allow -->` → historia, se salta) y
  `docs/src/installation.md:198` (`$Version = "v3.0.0"`). El escritor debe ser
  no-op sobre este árbol.
- **L7** (2026-10-20, worker) — RED→GREEN observado, por harness:
  `test_bump_docs_version_pins.sh` RED `harness: 0 ok, 1 fail` → GREEN
  `harness: 14 ok, 0 fail`; `test_apply_release_pins.sh` RED `0 ok, 1 fail` →
  GREEN `11 ok, 0 fail`; `test_release_pins_wiring.sh` RED `FAILED: 5/7 casos`
  → GREEN `OK: 7/7`. El caso de reintento se probó por mutación
  (`max_attempts=2→1` → `10 ok, 1 fail`) y se restauró con sha256 idéntico.
- **L8** (2026-10-20, padre) — Verificación independiente del padre (spot check
  sobre lo reportado por el worker), en el worktree:
  `test_bump_docs_version_pins.sh` → `14 ok, 0 fail`;
  `test_apply_release_pins.sh` → `11 ok, 0 fail`;
  `test_release_pins_wiring.sh` → `7/7`;
  `check_docs_version_pins.sh` → `OK: 3 líneas … 11 documentos` (exit 0);
  `check_release_dispatch.sh` → `OK: single native-push hand-off intact` (exit 0);
  `bash -n` y `shellcheck -x --exclude=SC2010,SC2012,SC2027,SC2034,SC2046,SC2086,SC2126,SC2129`
  sobre los 5 scripts nuevos → 0 findings;
  `scripts/ci_fast_gate.sh` → `fast-gate summary: PASS=13 FAIL=0 SKIP=7` /
  `fast-gate: GREEN` (lane CI-only, **0 invocaciones de cargo**).
- **L9** (2026-10-20, padre) — Prueba end-to-end del defecto y del fix, sobre una
  copia del árbol con el workspace en `3.0.1` (simula el bump de release-plz):
  1) el gate **falla** (`README.md:39` y `docs/src/installation.md:198` en
  `v3.0.0` contra workspace `v3.0.1`) → el defecto #2028 reproducido;
  2) el escritor actualiza esas 2 líneas y **no** toca la línea de historia
  (`installation.md:67`, `v2.11.0` con el marker);
  3) el gate **pasa** sobre el mismo árbol;
  4) `CHANGELOG.md` byte-idéntico (md5 antes/después);
  5) segunda corrida → `OK: los pines ya declaraban v3.0.1 (sin cambios)`.
- **L10** (2026-10-20, padre) — La rama se renombró a `fix/release-pins` (de
  `fix/release-owns-docs-version-pins`) para que el proxy directorio↔rama del
  AGENTS.md pase: el directorio del worktree es `fix-release-pins` y el proxy
  exige que sea **la rama completa** con `/` → `-`. La rama era local (nunca
  pusheada), así que el rename no desincroniza nada.
- **L11** (2026-10-20, padre) — El `.envrc` del worktree se escribió con `$TREE`
  literal (no sustituido) y el guard de aislamiento de targets lo rechazaba con
  `reason=unregistered-target`, making the fast gate exit 2 before any lane. El
  guard expande `$HOME` pero no `$TREE`: el bootstrap correcto lo sustituye al
  escribir. Corregido a `export CARGO_TARGET_DIR=$HOME/.cache/cargo-target/fix-release-pins`
  (con `$HOME` literal) → el fast gate cae en la lane CI-only y da GREEN.

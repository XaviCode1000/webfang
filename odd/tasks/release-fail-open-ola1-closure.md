# Campaña de remediación fail-open — cierre de las olas 1 y 2

**Ola 1 (B5, B6, B7, B8, B10)** · **Ola 2 (B9, B11, B12, B13, B14, B15)**.

> Este doc cubre el cierre de ambas olas del plan global del audit
> `AUDIT-SCRIPTS-FAIL-OPEN-20261018-222141` (umbrella #2127). Las olas 3+ (TOML/Rust,
> seed+artifact, policy+advisory, CI) siguen abiertas en #2130–#2133.

## Ola 1 — release pipeline producer-fail aware (B5, B6, B7, B8, B10)

**Feature doc de la Ola 1 del plan global de remediación del audit
`AUDIT-SCRIPTS-FAIL-OPEN-20261018-222141`.** Umbrella: #2127 · Ola 1: #2128.
PR-A: #2135 (merged) · PR-B: #2140 · PR-C: #2141 · reparación de drift: #2142 → #2143 (merged).

## Specs

- **S1** — "un producer que no pudo correr no puede leerse como 'no tags'" (B6, audit).
  La lista de tags ilegible falla-cerrada, con error nombrado; el rc del productor se captura.
- **S2** — "la lista de verificación de assets no puede encogerse en silencio" (B7, audit).
  El paso de firma/verificación/publicación exige el inventario canónico completo y nombra
  el artefacto faltante.
- **S3** — "'complete' significa la superficie de atestación completa" (B8, audit).
  La completitud de una release exige los 7 assets canónicos, sus bundles `.sigstore.json`
  y tamaños no nulos; un payload ilegible falla-cerrada.
- **S4** — "la comparación de versión no puede cruzar las familias de gramática" (B5, audit).
  La comprobación L1.2 compara el semver completo, sinSuffix Strip; una divergencia RC/stable
  se nombra en ambas direcciones.
- **S5** — "una línea inexistente no puede borrarse y luego reportarse como EOL" (B10, audit).
  `eol-line.sh` valida la línea antes de cualquier mutación y escribe el estado antes de
  borrar la branch.

## Tasks

| ID | Specs | Ruta | PR | Commit |
|---|---|---|---|---|
| T1 | S2 | `.github/workflows/release.yml` | #2135 | `3b697812` |
| T2 | S3 | `scripts/ensure-release.sh` + 2 harnesses | #2135 | `4a4a9092` |
| T3 | S4 | `scripts/check_release_provenance.sh` + harness | #2135 | `a8b6d7fc` |
| T4 | S1 | `scripts/release-plz-tags.sh` | #2140 | `1914fff6` |
| T5 | S5 | `scripts/eol-line.sh` | #2141 | `ab2d7c4e` |
| T6 | — | `.github/support.json` + `SUPPORT.md` (drift que tumbaba todo PR) | #2143 | `ff21a275` |
| T7 | — | automatización del drift (capa 1 preventiva) | pendiente | #2144 |

## Log

- **L1** — El usuario pide avanzar la Ola 1 del plan global: abrir el PR de B5+B7+B8 en
  `fix/release-verify-producer-fail` desde `main` 2865dab8, con `Closes part of #2128`.
- **L2** — T1 (B7): `RELEASE_EXPECTED_ASSETS` con los 7 nombres canónicos + prechecks
  fail-closed en sign/verify/publish. El drive vivo (pyYAML + stubs) encontró un
  falso-positivo propio: el heredoc añade una línea vacía final que el `while read`
  procesaba como un faltante → guard de tail-safety con `if` explícito (el `&& continue`
  además es vacío con `set -e`).
- **L3** — T2 (B8): inventario de completitud = 7 nombres + bundle por asset + tamaño > 0,
  JSON crudo con jq y fail-closed no-parsable. Harnesses: reconcile 35/0, squash 24/0
  (la parte squash la delegó un subagent-worker; el resto, inline).
- **L4** — T3 (B5): L1.2 compara semver completo verbatim; I.10 corregida al gramática RC
  exacta + I.10b/I.10c (divergencias nombradas). Provenance 75/0.
- **L5** — T4 (B6): `release-plz-tags.sh` captura el rc del productor (`git tag -l` /
  `git tag --points-at HEAD`) y sale con `exit 6` nombrado cuando la lista es ilegible.
  El repro del audit lo lleva a *matched*; sweep/release harnesses intactos.
- **L6** — T5 (B10): `eol-line.sh` valida la línea con `jq -e` antes de mutar, escribe el
  estado antes de borrar la branch, y nombra el retry cuando el borrado remoto falla.
  El repro del audit lo lleva a *matched*; `test_support_drift.sh` 50/0.
- **L7** — T6: al mergear la Ola 1, `Validate PR metadata` quedó rojo para **todo** PR del
  repositorio: el release v4.0.1 (#2139) publicó el tag `v4.0.1` desde `main` y ningún flujo
  actualizó `support.json` (la línea 4.0 seguía declarando `.latest = 4.0.0`), y el gate
  enforcing `check_support_drift.sh --scope=published` vive dentro de ese required job.
  Reparado con el escritor sancionado `refresh_support_latest.sh` (#2142 → #2143).
- **L8** — T7: el hueco tiene causa estructural verificada — `bump-support-patch.sh:192`
  **sí** declara la publicación pendiente (`refresh_support_latest.sh --set`), y el camino
  `main` → release-plz → tag **no**. El gate ya tolera la ventana correcta ("one patch
  above the newest published patch of its own minor"), así que la capa 1 preventiva
  (declarar en el Release PR, antes del corte del tag) cierra la ventana en su origen;
  la capa 2 (workflow `push: tags` que recomputa y abre PR) es la red de reparación.
  Issue #2144.
- **L9** — Cierre de Ola 1: recuento final del repro canónico sobre `main` con los cinco
  defectos (B5, B6, B7, B8, B10) caídos, y los que quedan vivos (B9, B11–B15) explícitamente
  fuera de esta ola.
- **L10** — Cierre verificado: el harness canónico (`webfang-audit/.../repro/release_repro.sh`,
  artefacto local — `.gitignore:164` excluye `webfang-audit/`) corre **9 matched / 6 confirmed**
  sobre `main` post-merge, con dos corridas idénticas. Los 6 confirmados (B9, B11, B12, B13, B14,
  B15) son exactamente el alcance de la Ola 2 (#2129).
- **L11** — El harness de B7 evolucionó: antes congelaba en el script el cuerpo pre-fix del step
  de `release.yml` ('Driven on the verbatim step body'), así que reportaba CONFIRMED con el fix
  ya vivo — un falso positivo del harness, no un defecto vivo. Ahora **extrae el step vivo** del
  `release.yml` del mini-repo con PyYAML (stub de cosign; `UNAVAILABLE` honesto si falta el
  parser) y el caso cae a *matched*. Lección: un harness que congela el código bajo prueba no
  puede certificar su propio fix.
- **L12** — Incidente de soporte: el drift de `.latest` (v4.0.1 publicado sin declarar) dejó el
  required check «Validate PR metadata» en rojo para **todo** PR del repositorio. Asimetría
  estructural verificada: `bump-support-patch.sh:192` sí declara la publicación pendiente
  (`refresh_support_latest.sh --set`) y el camino `main` → release-plz → tag no. Issue #2144 con
  la automatización en dos capas (capa 1 preventiva: declarar en el Release PR antes del corte
  del tag; capa 2: workflow `push: tags` que recomputa y abre PR).

---

# Ola 2 — el gate de support y los pines de docs (B9, B11, B12, B13, B14, B15)

**Issues**: Ola 2 = #2129. PRs: #2150 (B13, B14, B15), #2151 (B9, B11, B12).
Automatización relacionada: #2153 (capa 1 preventiva del drift, issue #2144).

## Specs

- **S6** — "una declaración con la misma minor dos veces no puede pasar" (B13).
  El gate de soporte valida unicidad de `.minor` y nombra la minor repetida.
- **S7** — "un productor que no puede leer no puede devolver una lista vacía con éxito" (B14).
  El productor de minors declaradas entrega orden de versión y falla cerrado.
- **S8** — "un tag `v*` que no es `vX.Y.Z` debe llegar al reporte" (B15).
  Un candidato `-rc.N` no es una publicación y no se reporta; cualquier otra cosa sí.
- **S9** — "un render fallido no puede dejar la proyección rastreada truncada" (B9).
  El generate escribe por rename atómico sólo tras un render exitoso.
- **S10** — "un guard que no puede decidir no puede degradarse a 'no hay nada que mirar'" (B11).
  Un fetch fallido y un estado de línea ilegible son rechazos nombrados, no tips.
- **S11** — "un árbol de documentación ausente no es un árbol sin pines" (B12).
  El árbol ausente se nombra y se rechaza, en el gate **y** en el escritor.

## Tasks

| ID | Specs | Ruta | PR | Commit |
|---|---|---|---|---|
| T8 | S6, S7, S8 | `scripts/check_support_drift.sh` + harness | #2150 | `5799535f` |
| T9 | S9, S10, S11 | `scripts/render-support-md.sh`, `scripts/bump-support-patch.sh`, `scripts/check_docs_version_pins.sh`, `scripts/bump_docs_version_pins.sh` + 3 harnesses | #2151 | `e693a673` |
| T10 | — | automatización del drift (capa 1) | #2153 | `e0f34639` |

## Log

- **L13** — Ola 2 abierta con dos unidades disjuntas delegadas en paralelo (superficies
  no solapadas: el gate de soporte por un lado, el render y los pines por otro) y una
  tercera (la automatización) con el script ya escrito y el harness + cableado pendientes.
- **L14** — El harness del gate pasó de **54 a 60** casos. B13 falla también en el scope
  `published` (es el gate *requerido*: restringir el fix al scope no requerido dejaría
  pasar el duplicado por la puerta que CI consulta). De paso, el mensaje "no se encontró
  ningún tag vX.Y.Z" documentado era inalcanzable (grep salía 1 con lista vacía y mataba
  el script bajo `set -euo pipefail`: un repo sin tags fallaba en silencio).
- **L15** — El hallazgo fuera de alcance más valioso de la Ola 2: **el ESCRITOR** de los
  pines de docs (`bump_docs_version_pins.sh`) tenía el mismo patrón B12 que el gate, y
  corre **primero** en el flujo del Release PR — su punto ciego ocultaba el del lector.
  Corregido en el mismo PR, con dos filas de harness propias.
- **L16** — **Tres casos del harness canónico eran frágiles** y no certificaba ninguno de sus fixes:
  B7 congelaba el cuerpo pre-fix del step de `release.yml`; B14 contaba un **comentario** que
  menciona el patrón defectuoso (la misma ceguera que B3 del propio audit); B9 medía
  "truncado" por la presencia de una fila, confundiendo *truncado* con *nunca renderizado*.
  Los tres se corrigieron en el harness (artefacto local: `webfang-audit/` está gitignored) y
  ahora **discriminan**: CONFIRMED sin el fix, matches con él (control ejecutado contra `main`
  y contra el worktree que no toca ese defecto).
- **L17** — Incidentes de proceso: (a) una branch `chore/release-*` **choca con el namespace
  del bot de release-plz** (`release-plz.toml:38` → `pr_branch_prefix = "chore/release-"`): el
  bot cerró el PR #2149 al abrir su propio Release PR; la rama se renombró a
  `chore/support-declare-pending-publication` → #2153. (b) El advisory **RUSTSEC-2026-0383**
  dejó el required check `Security Audit` en rojo para **todo** PR del repo: se corrigió
  subiendo `webpki-root-certs` a 1.0.10 en **los dos** lockfiles (la auditoría escanea
  `Cargo.lock` **y** `fuzz/Cargo.lock`) → #2148.
- **L18** — Cierre verificado: el harness canónico corre **15 matched / 0 confirmed** sobre
  `main` post-merge, con dos corridas idénticas.

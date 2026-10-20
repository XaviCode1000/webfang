# Ola 1 — release pipeline producer-fail aware (B5, B6, B7, B8, B10)

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

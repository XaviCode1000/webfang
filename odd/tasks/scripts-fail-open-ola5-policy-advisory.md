# Campaña de remediación fail-open — Ola 5 (policy guards + advisory)

**Ola 5 del plan global del audit `AUDIT-SCRIPTS-FAIL-OPEN-20261018-222141`** (umbrella #2127, ola #2132).

Dos PRs: **PR-K** = advisory (B40, B41) y **PR-L** = policy guards (B42, B43, B44). Son *quick wins*
(T0.5): cinco defectos, dos ficheros de producción, sin Rust.

El canon de verificación es `webfang-audit/.../repro/{advisory,policy}_repro.sh`:
`advisory` debe pasar de **3 matched / 3 confirmed** a **6/0** y `policy` de **1 matched / 3 confirmed**
a **4/0**, con dos corridas idénticas cada uno.

## Specs

- **S1** — "the parser takes the first quoted RUSTSEC id on the line, not the one after `id =`; TOML
  inline tables are unordered, so `{ reason = "same path as RUSTSEC-2025-0141", id = "RUSTSEC-2026-9999" }`
  suppresses the wrong advisory. A single-quoted id is dropped entirely." (B40, audit)
  El id capturado es el que sigue a `id[[:space:]]*=`, tolerando comentarios; un id entre comillas
  simples no se descarta.
- **S2** — "`[advisories] # policy` or a leading-space header are valid TOML but do not match
  `/^\[.*\]$/`; nothing is parsed and the script fails with 'No se pudo extraer ningún aviso'." (B41, audit)
  El header se reconoce con su comentario y su espacio inicial; el fallo de parseo sigue siendo
  fail-closed, pero no por no haber entendido un TOML válido.
- **S3** — "an `#[ignore]` line inside a block comment is classified as a test attribute, pairs with the
  next `fn`, and fails CI on a comment." (B42, audit)
  La clasificación de atributo/prosa distingue el estado de comentario; un `#[ignore]` comentado no
  empareja ninguna función.
- **S4** — "`support/*` and `release/*` accept malformed refs (`support/garbage`, `support/foo/bar`,
  since `*` crosses `/`) although the error text says `support/X.Y`." (B43, audit)
  La forma `support/[0-9]+\.[0-9]+` es un requisito, y una base hotfix que cruce `/` se rechaza.
- **S5** — "the checker never verifies a pattern matched ≥1 site; a stale entry only emits
  `::warning::` and exits 0." (B44, audit)
  Una entry del allowlist que matchea **cero** sitios es un fallo, no un aviso.

## Tasks

| ID | Specs | Ruta | PR | Commit |
|---|---|---|---|---|
| T1 | S1–S2 | `scripts/check_advisory_policy.sh` + `scripts/test_advisory_policy.sh` | PR-K (#2132) | pendiente |
| T2 | S3–S5 | `scripts/check_ignored_guard.sh`, `scripts/check-topology.sh`, `scripts/check_intra_crate_direction.sh` + harnesses | PR-L (#2132) | pendiente |
| T3 | S1–S5 | Verificación canónica: `advisory_repro.sh` a 6/0 y `policy_repro.sh` a 4/0, dos corridas idénticas sobre `origin/main` limpio | — | pendiente |

## Log

- **L1** (2026-10-21, petición del mantenedor): "tengo un agente mergeando las pr's abiertas continuamos con las siguientes olas?"
- **L2** (coordinación con la Ola 3): **S5 (B44) se implementa aquí, en el gate con extractor awk**, y
  el flip de la Ola 3 (S4, `scripts/check_intra_crate_direction.sh` reescrito sobre el tool con `syn`)
  **arrastra la misma regla** al gate nuevo. Implementarlo en los dos sitios por separado crearía
  un conflicto de merge seguro sobre el mismo fichero.
- **L3** (dependencia de orden): T2 toca el harness del gate intra-crate, cuyo nombre cambia en el
  PR de la Ola 3 S1 (`test_intra_crate_gate.sh` → `test_intra_crate_direction.sh`). T2 se lanza
  **después** de que ese PR entre en `main`, para no editar un fichero que va a renombrarse.
- **L4** (decisión del padre sobre B43, 2026-10-21): la forma exigida es la **estricta y versionada**
  (`support/[0-9]+\.[0-9]+`, `release/[0-9]+\.[0-9]+`), no la laxa `support/[^/]+`. Motivo: la política
  viva del repo crea esas ramas con `cut-support-branch.sh X.Y` (numérico) y la propia tabla de
  AGENTS.md nombra `support/X.Y`; una regla laxa seguiría aceptando `support/garbage`. Consecuencia
  asumida: un control del repro que usa el placeholder `support/main-support` sale ahora rc=1 (no es
  veredicto, es texto observado) y **el doc queda desfasado** en la tabla head→base de AGENTS.md y en
  el comentario de `scripts/bump-support-patch.sh:129` → se alinean en este mismo PR (docs junto al
  comportamiento).
- **L5** (metaguard): `scripts/test_guard_coverage.sh` falla con "HARNESS NOT WIRED" si un harness no
  lo nombra ningún job de CI. Los dos harnesses nuevos (`test_topology.sh`,
  `test_intra_crate_allowlist.sh`) se cablean en el job `repo-guards` de `.github/workflows/ci.yml` en
  este PR — al FINAL de los steps del job, para dejar distancia de hunks con el PR #2159 (Ola 3 S1),
  que inserta un step justo después de "Intra-crate direction gate (strict)".

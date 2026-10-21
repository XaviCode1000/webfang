# Campaña de remediación fail-open — Ola 4 (seed + artifact)

**Ola 4 del plan global del audit `AUDIT-SCRIPTS-FAIL-OPEN-20261018-222141`** (umbrella #2127, ola #2131).

Dos PRs, superficies disjuntas: **PR-I** = cadena de seed (B18–B25, 3 HIGH + 5 MED) y
**PR-J** = artefactos (B21, B26–B31, 1 HIGH + 6 MED). El canon de verificación es el harness
canónico `webfang-audit/.../repro/{seed,artifact}_repro.sh`, que debe pasar de
`seed: 3 matched, 8 confirmed` / `artifact: 0 matched, 7 confirmed` a **0 confirmed** en ambas clases.

## Specs

- **S1** — "The prune hardcodes `debug/`, while the recipe supports `--profile` and `--target` and the key encodes both." (B18, audit)
  El perfil del prune se deriva de la receta y del target; un seed release o cross-target es publicable.
- **S2** — "`--config-digest` is required, `--workspace-digest` is not; omitting it emits `workspace_contract = ""`, so every workspace sharing rustc/config/lock collides." (B19, audit)
  `--workspace-digest` no vacío es un requisito duro de la clave de compatibilidad.
- **S3** — "The retire path does `chmod -R u+w` before the rename, and recovery does it again and renames back — so a restored seed's contents are writable, unlike a published seed (555)." (B20, audit)
  Sólo la entrada superior se chmoddea antes del rename; tras restaurar, el árbol queda `a-w` con la cima `u+w`, y el modo se aserta en el test.
- **S4** — "a non-force publish replaces a seed that appeared during the build; `publish_atomically` ignores `FORCE`." (B22, audit)
  Un publish no-force nunca reemplaza un seed que apareció durante el build.
- **S5** — "`cp -a "$SEED" "$TARGET_DIR"` into an existing empty dir nests one level (`$TARGET_DIR/<key>/debug`), Cargo builds cold, yet the verdict is `seeded`." (B23, audit)
  El consumo reporta `seeded` sólo cuando el árbol semilla quedó en su sitio; un anidamiento es un fallo, no un acierto.
- **S6** — "`SEED_RECIPE_STRIPPED_ENV` is never read (the real strip is by prefix), and the key hashes the literal `"prescribed-empty"`, so editing the policy does not re-key. `SCCACHE_*` entries in the list are not actually removed." (B24, audit)
  La política de variables eliminadas se lee de verdad y la clave cambia cuando cambia.
- **S7** — "newline-delimited arg/env serialization breaks any value containing a newline; multi-line `RUSTFLAGS` becomes several `mapfile` entries and `env` sees the tail as options." (B25, audit)
  Cada valor sobrevive a su forma serializada en el punto real de consumo.
- **S8** — "`rm -rf` of the output directory precedes non-atomic per-crate rendering. A failure on crate 3 leaves `01-…`/`02-…` with no `00-narrative`, no `06-cli-reference` and no `llms.txt` — a publishable partial artifact." (B21, audit)
  El render ocurre en un directorio temporal, se valida el conjunto de ficheros esperado, y sólo entonces se intercambia.
- **S9** — "`license_value[:34]`, `name[:38]`, `ver[:20]` truncate without an ellipsis; `MIT OR Apache-2.0 WITH LLVM-exception` loses `-exception` and disagrees with the SBOM." (B26, audit)
  El recorte declara la truncación y no altera el valor comparado con el SBOM.
- **S10** — "`mktemp` in `$TMPDIR` then `mv` into the repo is not atomic across filesystems." (B27, audit)
  El fichero se escribe en el mismo sistema de ficheros y se intercambia de forma atómica.
- **S11** — "the trace id is interpolated into the jq **program**: a quote breaks it, a crafted value rewrites the filter. Use `jq --arg t \"$TRACE\"`." (B28, audit)
  El trace id viaja como argumento, nunca como programa.
- **S12** — "fence handling toggles on any ``` ``` ```/`~~~` line regardless of opener, so a `~~~` inside a ``` ``` ``` block flips the state." (B29, audit)
  El estado del fence depende del opener que lo abrió.
- **S13** — "wall-clock `BUILD_DATE` is embedded in every generated file, so the artifact is not reproducible." (B30, audit)
  Con el mismo `SOURCE_DATE_EPOCH`, el artefacto es byte-idéntico.
- **S14** — "the zip is written in place; a mid-write failure leaves a valid-looking partial zip that `:106-111` then refuses to overwrite." (B31, audit)
  El zip se escribe en un temporal y se renombra: una ejecución muerta no deja un parcial creíble.

## Tasks

| ID | Specs | Ruta | PR | Commit |
|---|---|---|---|---|
| T1 | S1–S7 | `scripts/seed_{publish,compat_key,recover,target,recipe}.sh` + harnesses `scripts/test_seed_*.sh` | PR-I (#2131) | pendiente |
| T2 | S8–S14 | `scripts/{build-llm-artifact,generate-third-party-notices,gen-cli-reference,analyze-trace,make-deterministic-zip}.sh` + harnesses `scripts/test_artifact_*.sh` | PR-J (#2131) | `fix/artifact-determinism` (sha en el PR; el propio commit no puede contener su sha) |
| T3 | S1–S14 | Verificación canónica de la clase: `seed_repro.sh` a 11/0 y `artifact_repro.sh` a 7/0, dos corridas idénticas sobre `origin/main` limpio | — | pendiente |

## Log

- **L1** (2026-10-21, petición del mantenedor): "tengo un agente mergeando las pr's abiertas continuamos con las siguientes olas?"
- **L2** (decisión de la Ola 3 que se arrastra aquí): el gate intra-crate (S4/S5 de la Ola 3) y la Ola 4 son trabajo de un padre
  distinto; esta ola no toca el gate intra-crate, ni `scripts/ci_fast_gate.sh`, ni los scripts del CI — para que los
  tres workers de la campaña puedan convivir sin solapar superficies.
- **L3** (T2 / PR-J, 2026-10-21): el caso B27 del harness canónico **no discrimina** — su fixture escribe un
  `cli-reference.md` sin marcadores, así que el script aborta en el check de marcadores antes de llegar al código del
  stage, y su condición "moved" es un `grep -c` que devuelve `"0"` (no vacío) — el veredicto sale `CONFIRMED` igual
  con el script pre-fix y con el post-fix. El harness canónico queda intacto (no se reescribe ni se debilita); la
  evidencia que discrimina vive en `scripts/test_artifact_cli_reference.sh` (staging en el directorio del capítulo,
  `$TMPDIR` inutilizable y en otro sistema de ficheros, capítulo byte-idéntico tras un aborto).
  Los cinco harnesses nuevos son **no vacíos**: contra los scripts pre-fix fallan (8, 3, 4, 3 y 2 casos
  respectivamente).

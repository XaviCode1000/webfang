# ADR-0004 paso 5 — release de deprecación de ONNX local

Anuncia la retirada de ONNX local sin quitar capacidad y arranca el reloj de **2 releases minor** que
el paso 6 necesita. No borra nada: por eso no mejora `main`, lo prepara (ADR:25).

Los pasos 1–4 están mergeados y el criterio técnico que habilita el paso 6 se cumple en sus dos
mitadas (suite wiremock verde, registro reproducible archivado en #1961).

**Rama:** `docs/adr0004-marker-en-paso6` (PR-A, docs) · **Worktree:**
`~/Projects/Rust/webfang-worktrees/docs-adr0004-marker-en-paso6`
**ADR:** `docs/adr/0004-onnx-local-removal.md`

## Specs

- **S1** — El release del paso 5 es un **minor sin marker de ruptura**. El marker `BREAKING CHANGE:`
  viaja en el commit del paso 6, que es donde el cambio es real. *(Decidido por el owner
  2026-10-16, #1998.)*
- **S2** — La etiqueta de recuperación del árbol ONNX existe **antes** de publicar el paso 5 y no
  matchea `v*`. *(Hecho: `onnx-tree/pre-step5` → `38ab6a31`.)*
- **S3** — El aviso de deprecación sale **una vez por corrida**, en ejecución, y nombra: qué
  desaparece, qué sobrevive, los shims, el canal de objeción y el schedule.
- **S4** — El aviso del umbral huérfano sale una vez por corrida mientras el umbral sea el default y
  el modelo no esté calibrado, apoyado en el `threshold_used` que el summary ya expone.
- **S5** — Existe una guía de migración enlazable y el README declara la pérdida del zero-config
  offline-total (condición 3 del owner).
- **S6** — El canal de objeción de la señal 2 es una issue **abierta ≥2 minors** que ningún PR cierra.
- **S7** — `CHANGELOG.md` no se toca en ningún work PR (lo escribe release-plz).

## Tasks

| # | Tarea | S# | Ruta | Estado |
|---|---|---|---|---|
| T1 | Etiqueta de recuperación `onnx-tree/pre-step5` | S2 | inline (git) | ✅ `38ab6a31`, pusheada |
| T2 | Issue de objeción #1997 + issues de trabajo #1998/#1999 | S6 | inline (gh) | ✅ creadas, abiertas |
| T3 | Enmienda del ADR: minor sin marker, tag y canal concretos | S1, S2, S6 | inline (docs) | ✅ en este PR |
| T4 | Aviso en ejecución + snapshot | S3 | delegado | ✅ en este PR |
| T5 | Aviso del umbral huérfano + test | S4 | delegado | ✅ en este PR |
| T6 | Guía de migración + README | S5 | delegado | ✅ en este PR |
| T7 | Verificar que el minor sale minor | S1 |inline (release) | 🔲 al cortar el release |

## Details

| Tema | Decisión |
|---|---|
| T1 — nombre del tag | No-`v*` a propósito: `release.yml:34-35` dispara con `tags: v*` y `RELEASE_TAG_NAME_RE` (`scripts/release-tag-trust.sh:50`) es `^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$`, inerte en los tres consumidores. Sin precedente: `git tag \| rg -v '^v'` salía vacío. El `--version` del binario sale de `CARGO_PKG_VERSION`, no de `git describe`, así que no lo contamina |
| T1 — a qué commit | `38ab6a31`, un commit después de `v2.11.0`. El paso 5 no toca código ONNX, así que apuntar al commit previo al release no pierde nada |
| T2 — por qué issue y no discusión | Discussions deshabilitadas (`has_discussions: false`) |
| T2 — la trampa | `pr-validation.yml` exige que todo PR cierre una issue. Si el PR del aviso cerrara #1997, GitHub cerraría también el canal de objeción al mergear y la señal 2 perdería su destino. El PR referencia #1997 con `Refs` y cierra #1999 |
| T4 — vehículo | `preflight_notes::record()` (`crates/webfang_core/src/cli/preflight_notes.rs:68`), replayado en `crates/webfang_cli/src/main.rs:200-202` — el mismo sitio del aviso de `--max-tokens`, por el motivo #796: argv se resuelve antes de que exista el subscriber |
| T4 — honestidad del texto | Hoy `--clean-ai` **sin** la feature `ai` es error de configuración (`check_clean_ai_feature`, `crates/webfang_core/src/cli/preflight.rs:1404-1419`, exit 78). El des-gating es trabajo del paso 6; el aviso anuncia el estado futuro |
| D1 — cerrado | El cleaner resuelve su `EmbeddingPort` por **`--embedding-provider`** (env `WEBFANG_EMBEDDING_PROVIDER`, flag > env) tras el borrado. Decidido por el owner 2026-10-16. Descartado `--tier2-embedding-provider`: el §Egress de Tier 2 prohíbe que un slot herede la resolución de otro. Descartado un flag nuevo: sería un shim más dentro de la misma ventana |

## Fuera de alcance (lo hace el paso 6)

Borrar `webfang_ai`, la feature `ai`, `check_clean_ai_feature` y sus tests · des-gatear
`--clean-ai`/`--offline` al build por defecto · re-indexado de vaults y su coste por usuario.

## Verificación

```bash
bash scripts/ci_fast_gate.sh                 # lane-aware
cargo fmt --all -- --check                   # --check, nunca el fixer
cargo nextest run --test max_chars_bound_test   # el shim sigue funcionando   (T4-T6)
env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps
```

Snapshot del texto del aviso con `redact_nondeterministic()` (patrón de
`crates/webfang_core/tests/max_chars_bound_test.rs:371`).

## Checklist

- [x] D2 decidida y anotada en el ADR
- [x] `git tag` de recuperación creado y pusheado, nombre no-`v`
- [x] Issue de objeción creada, abierta, `type:chore`
- [x] Ningún PR cierra el issue de objeción
- [x] Aviso sale una vez por corrida y nombra canal + schedule + shims
- [x] Aviso del umbral: una vez por corrida, solo con umbral default
- [x] Guía de migración enlazada desde el aviso; README declara la pérdida de offline-total
- [x] `status:approved` en #1999 (la pone el maintainer); en #1998 moot — se autocerró al mergear PR-A
- [ ] Al cortar el release: verificar que salió minor y no major

## Log

- **L1 (owner, 2026-10-16):** "cuáles el paso pendiente del ADR 0004?" → el paso 5, con el 6 bloqueado
  detrás. Verificado en `main` @ `38ab6a31`: pasos 1–4 mergeados, sin aviso de ONNX en el CHANGELOG,
  sin etiqueta de recuperación, sin issue de objeción.
- **L2 (owner, 2026-10-16):** "adelante" → se arma este plan, sin mutaciones.
- **L3 (owner, 2026-10-16):** "ok vamos a por (recomendado)" → D2 resuelto: minor sin marker +
  enmienda del ADR. Ejecutado: T1 (tag `onnx-tree/pre-step5` → `38ab6a31`, pusheada), T2 (#1997 canal,
  #1998 enmienda, #1999 trabajo), T3 en curso en este PR.
- **L4 (owner, 2026-10-16):** D1 **cerrado**: `--embedding-provider`. Queda escrito en el ADR como
  especificación del paso 6, con el gap de hoy declarado (`build_ai_cleaner` solo resuelve ONNX local).
  Con esto el texto del aviso ya se puede escribir y T4-T6 dejan de estar bloqueados.
- **L5 (2026-10-16):** PR-A abierto (#2000, cierra #1998), CI Gate en verde; el commit de D1 se suma
  encima porque el ADR es la fuente normativa y dejar el knob sin fijar obligaría al aviso del paso 5
  a nombrarlo por su cuenta.
- **L6 (2026-10-16):** PR-A mergeado como `4286d921` (squash), #1998 cerrado por autocierre; #1997
  sigue abierta a propósito porque es el reloj de la señal 2. T4-T6 quedan hechas en este PR (segundo del par,
  sobre `main` ya avanzado): la rama nunca se pusheó, así que `git rebase origin/main` es seguro y no
  obliga a force-push — y no comparte ni un archivo con PR-A, que fue lane docs puro, así que el
  rebase fue limpio. El checklist de `status:approved` se parte en dos: #1999 lo tiene, #1998 quedó
  moot al auto cerrarse.
- **L7 (2026-10-19, cierre del reloj):** el reloj de 2 releases minor se cerró con **3.0.0**, no con
  2.14.0: el borrado es un commit de ruptura (#2072) y la política del repo (`AGENTS.md:1106`) lo
  convierte en major. La señal 2 (#1997) cerró sin comentarios → desenlace 6a. El aviso que este paso
  publicó se reescribió en #2078 porque seguía en futuro una vez ejecutada la retirada — detalle en L12
  y L13 de `adr0004-paso6-borrado-onnx.md`.
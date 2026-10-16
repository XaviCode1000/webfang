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
| T4 | Aviso en ejecución + snapshot | S3 | delegado | 🔲 #1999, bloqueado por `status:approved` |
| T5 | Aviso del umbral huérfano + test | S4 | delegado | 🔲 #1999 |
| T6 | Guía de migración + README | S5 | delegado | 🔲 #1999 |
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
| D1 — abierto | El cleaner hoy no tiene camino remoto: `build_ai_cleaner` (`main.rs:873-978`) vive bajo `#[cfg(feature = "ai")]` y resuelve solo el puerto ONNX local. El ADR no fija qué knob consume `--clean-ai` tras el borrado. Recomendación: `--embedding-provider` (ya resuelve sin `ai`, `main.rs:980-987`). Bloquea el texto del aviso hasta que el owner lo cierre |

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
- [ ] Aviso sale una vez por corrida y nombra canal + schedule + shims
- [ ] Aviso del umbral: una vez por corrida, solo con umbral default
- [ ] Guía de migración enlazada desde el aviso; README declara la pérdida de offline-total
- [ ] `status:approved` en #1998 y #1999 (label protegida: la pone el maintainer)
- [ ] Al cortar el release: verificar que salió minor y no major

## Log

- **L1 (owner, 2026-10-16):** "cuáles el paso pendiente del ADR 0004?" → el paso 5, con el 6 bloqueado
  detrás. Verificado en `main` @ `38ab6a31`: pasos 1–4 mergeados, sin aviso de ONNX en el CHANGELOG,
  sin etiqueta de recuperación, sin issue de objeción.
- **L2 (owner, 2026-10-16):** "adelante" → se arma este plan, sin mutaciones.
- **L3 (owner, 2026-10-16):** "ok vamos a por (recomendado)" → D2 resuelto: minor sin marker +
  enmienda del ADR. Ejecutado: T1 (tag `onnx-tree/pre-step5` → `38ab6a31`, pusheada), T2 (#1997 canal,
  #1998 enmienda, #1999 trabajo), T3 en curso en este PR.
- **L4 (owner, 2026-10-16):** D1 sigue **abierto** — qué knob de provider consume `--clean-ai` tras el
  borrado. No se registró en el ADR porque es decisión de diseño del paso 6, no del paso 5; bloquea el
  texto del aviso (T4).
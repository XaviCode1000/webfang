# Feature: Slice 1 — Inspector semántico de Tier 2 sobre `EmbeddingPort`

**ADR:** `docs/adr/0004-onnx-local-removal.md`, paso 3 (condición 2 resuelta 2026-10-11: **migrar**)
**Branch:** `feat/tier2-port-backed-inspector` (base `main` @ `23b6914e`)
**Worktree:** `~/Projects/Rust/webfang-worktrees/feat-tier2-port-backed-inspector`
**Issue:** #1977
**PR:** #1978

## Objetivo

`GraniteDomInspector` deja de depender de ONNX: consume `Arc<dyn EmbeddingPort>` como el cleaner ya
hace desde el Tramo D (`52c378d6`). Al final del slice, **Tier 2 no toca un solo ONNX runtime**, y su
superficie de código no depende de que el crate `webfang_ai` sobreviva al borrado del paso 6.

**Este slice NO introduce egress.** El inspector queda cableado explícitamente al puerto local. El
salto a endpoint remoto es el slice 2, y con él llega el paquete de egress del ADR §Egress.

## Por qué el inspector no lo heredó el Tramo D

El seam (`EmbeddingPort`, `domain/embedding_port.rs:40`) lo consume el cleaner
(`semantic_cleaner_impl.rs:321`), no Tier 2. `GraniteDomInspector` sigue con
`Arc<dyn InferenceEngine>` + `Arc<MiniLmTokenizer>` (`granite_dom_inspector.rs:33-37`) y su builder
está bajo `#[cfg(all(feature = "ai", feature = "adaptive-selectors"))]` (`main.rs:576`). Sin este
slice, el paso 6 borra ONNX y la reparación semántica desaparece — degradando a Tier 1 en silencio.

## La trampa del slot (condición dura de diseño)

`build_ai_cleaner` (`main.rs:688-700`) construye el cleaner con el **puerto local** del triple
`(port, engine, tokenizer)`. Después, `resolve_remote_embedding` (`:817`) **sobrescribe**
`vault_ports.embedding_port`, que es un `OnceCell` en el Container
(`container.rs:136,872-873`) — se setea una vez y no registra su origen.

Si el inspector leyera ese slot, un usuario con `--embedding-provider remote` (opt-in para *vault
search*) mandaría **fragmentos del DOM a un tercero sin haberlo consentido**.

**Por eso el bind ocurre DENTRO de `build_ai_cleaner`**, clonando el `port` local antes del
overwrite. Después del overwrite no hay forma de distinguir local de remoto.

## Condiciones duras

1. **Bind explícito local** dentro de `build_ai_cleaner`, con guarda de regresión que demuestre que
   `--embedding-provider remote` no cambia el inspector.
2. **Coseno fail-closed propio.** `vault_search.rs:403` es `fn` privada en `application/`;
   importarla desde `infrastructure/` viola el gate intra-crate (`target_rank < src_rank`).
   El de `embedding_ops.rs:72` trunca en silencio (`a.len().min(b.len())`) — un dim mismatch se
   vuelve "similitud nula". El inspector necesita un helper propio, `None` ante mismatch.
3. **Destino: `crates/webfang_core/src/infrastructure/scraper/`**, simétrico a `DefaultDomInspector`.
   Vive en `infrastructure` (rank 0) consumiendo `domain::EmbeddingPort` (rank 3) — legal bajo el
   gate. Moverlo a `application/` violaría la regla sin entrada de allowlist (cap 5, ya_full at 2).
4. **Rename en este slice**: `GraniteDomInspector` → `EmbeddingSemanticInspector`. El tipo pasa a
   aceptar cualquier port; el nombre viejo miente desde el cambio de firma. Renombrar después sería
   una segunda ruptura encima de la primera. Los 2 call sites de
   `crates/webfang_ai/tests/erased_engine_ports_test.rs:109,157` se tocan igual por la firma.
5. **`embed_batch`** para target + fragmentos: un round trip, no N+1. Respetar `batch_max` del
   `ModelProfile` si el perfil lo declara (`domain/model_profile.rs:172`, `None` = sin tope).

## Decisiones tomadas por el owner (2026-10-11)

| Decisión | Valor | Por qué |
| --- | --- | --- |
| Migrar vs eliminar | **Migrar** | Eliminar degrada `--adaptive-selectors` a Tier 1 en silencio — lo que la condición 2 prohíbe |
| Consentimiento (slices 2/3) | **Explícito por corrida** | Precedente: `opt_in_flag_is_required_even_with_key`, `live_gate_test.rs:45-72`. Credencial ≠ consentimiento |
| `adaptive-selectors` default | **No tocar** | `default = []` fue deliberado (#1813 T4). Egress activada por default no se decide por conveniencia |
| Rename | **Sí, en slice 1** | Miente a nivel de firma desde el primer commit |

## Fuera de alcance (slices 2 y 3)

- `correlation_id` en los spans de `remote_embedding.rs:709-739` — hoy emiten `provider_id`/`model`/`dim`/`count` y ningún identificador de correlación (ADR:81).
- Contadores de `usage` — `EmbeddingUsage` se parsea, se loguea en `debug` y se descarta (`remote_embedding.rs:392-397`). No existe acumulador para embeddings.
- Summary por corrida con `threshold_used` — `ScrapeSummary` (`cli/summary.rs:8-23`) es código muerto: cero call-sites de producción.
- Consentimiento por hash `provider|endpoint`.
- Umbral por perfil (`ModelProfile` existe completo en `domain/model_profile.rs:160-186` pero no está cableado al CLI) y recalibración del 0.75 — son **dos** cambios: el gate real es `AdaptiveSelectorOptions::semantic_threshold` (`adaptive_engine/mod.rs:99`), el denominador muerto es `ErrorHintConfig::semantic_threshold` (`extraction_quality.rs:29`).

## Checklist

- [x] T1 — `EmbeddingSemanticInspector` en `core/infrastructure/scraper/`: constructor `Arc<dyn EmbeddingPort>` + threshold, coseno fail-closed propio (`cosine_similarity_checked -> Option<f32>`), `embed_batch` con ventanas `batch_max`, `SemanticMatch`/`None`.
- [x] T2 — 11 tests unitarios con `MockEmbedder`: match sobre umbral, `None` bajo umbral, dim mismatch fail-closed, `dom_fragments` vacío, orden de batch, `embed_batch` llamado una vez (no N+1).
- [x] T3 — Borrado `granite_dom_inspector.rs` + `lib.rs`/`infrastructure_ai/mod.rs` + los call sites de `erased_engine_ports_test.rs`.
- [x] T4 — Bind explícito local: `build_ai_cleaner` devuelve `Option<Arc<dyn EmbeddingPort>>` (antes el bundle engine+tokenizer); `semantic_inspector` consume el port.
- [x] T5 — Guarda de regresión `tier2_inspector_stays_bound_to_the_local_port` (`main.rs`): con el slot de vault sobrescrito por un remoto, el inspector embebe solo por el puerto local (`remote.embedded() == 0`).
- [x] T6 — Gates de CI, todos en verde (ver abajo).

## Corrección al plan: eran TRES call sites, no dos

El doc decía "los 2 call sites" de `GraniteDomInspector`. Son **tres**: el tercero es
`crates/webfang_cli/src/main.rs`, dentro de `semantic_inspector()`, gateado por
`#[cfg(all(feature = "ai", feature = "adaptive-selectors"))]`. Borrar el tipo sin tocarlo deja
`cargo check -p webfang_cli` rojo — o sea, **T4 no era un follow-up, era la mitad del cierre**.
Un rename partido entre work units no es una unidad revisable: es un punto de rotura.

## Decisiones tomadas durante la implementación

| Decisión | Elección | Razón |
| --- | --- | --- |
| Dim mismatch | **Degradar (skip el vector), no `Err`** | El mismatch es propiedad de *un* vector; un `Err` tiraría también los fragmentos comparables del mismo lote. Y `adaptive_engine/mod.rs:446-450` ya degrada `Err` a Tier 1, así que el error no aportaba nada que el skip no dé. Lo garantizado: un vector mal dimensionado **jamás** se reporta como match ni es indistinguible de un score bajo legítimo. Test: `dimension_mismatch_is_skipped_never_scored_as_a_match` (con el coseno troncante ese caso daba **1.0** y ganaba). |
| `batch_max` | **Expuesto** (`with_batch_max(Option<usize>)`) | `EmbeddingPort::embed_batch` no lleva cap; sin exponerlo, T4 no podría cablear el perfil sin una segunda ruptura de firma. Se respeta con ventanas consecutivas reensambladas por índice. |
| Shim `GraniteDomInspector` | **No** | Dejaría Tier 2 en el camino del feature `ai`, que es justo lo que el slice viene a sacar. |
| `cosine_similarity` de `embedding_ops.rs:72` | **No se toca** | Quedan 5 callers (`relevance_scorer.rs:146,164`, `semantic_cleaner_impl.rs:683`, `embedding_ops.rs:197,221`, `tests/p0_001_measure.rs:274`). |
| ¿Footer `BREAKING CHANGE:` por borrar `GraniteDomInspector`? (F5) | **NO** | Decisión del owner (2026-10-11). Las crates **no se publican a crates.io** (`publish = false` en `release-plz.toml`, distribución solo por binarios en GitHub Releases), así que no hay consumidor externo cuyo código pueda romperse. El footer dispararía un **major bump** en `main` por un rename interno de un tipo que ninguna superficie publicada expone. El rename sí es un cambio real y worthy de changelog — lo produce el conventional commit (`refactor(ai):`), no el footer. |

## Capa de corrección sobre el commit de slice 1 (2026-10-11)

Un reviewer más el CI unieron cinco correcciones sobre el commit ya pusheado. Se aplican
**en capa encima** — no se reescribe historia.

### F1 — La guarda era decorativa: el invariante pasa a ser un tipo

`build_ai_cleaner` devolvía `Option<Arc<dyn EmbeddingPort>>` y `VaultAiPorts.embedding_port` es
**el mismo tipo**. Consecuencia medida: cambiar `semantic_inspector(&shared, …)` por
`semantic_inspector(&vault_ports.embedding_port, …)` en `main.rs` compilaba con exit 0 (solo un
warning de `shared` sin uso). El borrado a `dyn EmbeddingPort` — que es lo que habilita el slice
— es también lo que borra la evidencia de origen.

**Fix (estructural, no convención).** Newtype `LocalEmbeddingPort` en
`domain/embedding_port.rs`, con constructor `pub` y campo privado + accessor `port()`.
`semantic_inspector` toma `&Option<LocalEmbeddingPort>`; `VaultAiPorts.embedding_port` **no
cambia**. La línea equivocada deja de compilar:

```text
error[E0308]: mismatched types
  --> crates/webfang_cli/src/main.rs:997:39
   |
997 |     let semantic = semantic_inspector(&vault_ports.embedding_port, &engine_options);
   |                    ------------------ ^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected `&Option<LocalEmbeddingPort>`, found `&Option<Arc<dyn EmbeddingPort>>`
```

Sin sealers, tokens ni friend-crates: la visibilidad en Rust es por crate, un `pub(crate)` en
`webfang_core` no alcanzaría a `webfang_cli`/`webfang_ai`, y un `pub` es lo correcto acá. La
frontera que queda es la **review** de su única call site de construcción.

### F1 (cont.) — El comentario del test decía más de lo que el test prueba

`tier2_inspector_stays_bound_to_the_local_port` construía `vault_slot` y nunca se lo pasaba a
nada, así que `remote.embedded() == 0` era trivialmente verdadero. Post-fix el invariante
anti-slot lo sostiene el **sistema de tipos**, y el test pina la FORMA del wiring. El doc
comment ahora dice exactamente eso, sin sobrevender ni humildad forzada.

`CountingEmbedder` quedó `#[cfg(feature = "ai")]`: su único consumidor es un test gateado por
`ai`, y el módulo `wiring_tests` compila con `adaptive-selectors` solo — sin el gate, el fixture
integro era dead code en esa combinación.

### F1 (cont.) — Dónde se envuelve, y por qué NO en la raíz ONNX

El brief pedía que `build_onnx_embedding_port` devolviera el port ya envuelto, para que
`LocalEmbeddingPort::new` quedara con una sola call site en el workspace. **No se hizo, y no es
una preferencia.** Medido:

```text
error[E0308]: mismatched types
   --> crates/webfang_mcp/src/mcp_server/mod.rs:191:29
    |
190 |  Arc::new(webfang_ai::SemanticCleanerImpl::from_parts(
191 |      port,
    |      ^^^^ expected `Arc<dyn EmbeddingPort>`, found `LocalEmbeddingPort`
```

`crates/webfang_mcp/src/mcp_server/mod.rs` está **fuera de las superficies de edición de esta
capa**. Envolver en la raíz ONNX rompe el daemon MCP, y arreglarlo excede el mandato — el
experiment se revirtió y el archivo quedó byte-idéntico.

Lo que sí se cumplió: `LocalEmbeddingPort::new` tiene **una sola call site en el workspace**, en
`build_ai_cleaner` — el composition root de la CLI, que es donde el bind ocurre igual que antes
del slice. El objetivo de la capa 2 (una sola construcción, revisable) se sostiene; lo que cambia
es el módulo que la hospeda. Si el owner prefiere la raíz ONNX, el diff es una línea en
`embedding_adapter.rs` más `.port().clone()` en `mod.rs:191` — pero `mod.rs` necesita entrar en
las superficies.

### F2 — El coseno fail-closed sube a `domain/`

`vault_search.rs:403` y `embedding_semantic_inspector.rs:217` eran el mismo helper carácter por
carácter (29 líneas clonadas por jscpd). Movido a `domain/embedding_port.rs` como
`cosine_similarity_fail_closed` — `domain` es la única capa que puede leer tanto `application`
como `infrastructure` sin violar `target_rank < src_rank`. **Ambos** consumidores se rewiren; si
`vault_search` hubiera conserva su copia privada, el clon habría sobrevivido mudado de
credenciales.

### F3 — `docs/test-inventory.md` desactualizado

Borrar `granite_dom_inspector.rs` se llevó su mención de `#[ignore]` en prosa y el catálogo no se
enteró. Rebaselineado 46 → 45 filas (35 atributos + 10 doc/comment), fila de
`granite_dom_inspector.rs` eliminada y renumeración de filas.

### F4/F6 — Ratchet de duplicación y estado del plan

Ver "Verificación observada" (abajo): 9282 → 9244, exactamente la medición local de `main`.

## Verificación observada — capa de corrección (2026-10-11)

Todos los comandos corridos en el worktree con
`CARGO_TARGET_DIR=$HOME/.cache/cargo-target/feat-tier2-port-backed-inspector`,
`CARGO_INCREMENTAL=0`, `RUSTC_WRAPPER` unset y `~/.local/share/mbx/bin` al frente del PATH
(`command -v cargo` → `/home/xavi/.local/share/mbx/bin/cargo`).

| Comando | Resultado |
| --- | --- |
| `cargo check -p webfang_cli --features ai,adaptive-selectors --all-targets` | exit 0 |
| `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | exit 0 (gate exacto de CI, ratchets #516 incluidos) |
| `cargo fmt --all -- --check` | exit 0 (tras el fixer, que tocó solo los 2 archivos en superficie) |
| `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps` | exit 0 (tras un `redundant_explicit_links` y un `broken_intra_doc_links` propios) |
| `bash scripts/check_ignored_guard.sh` | exit 0 — `OK: 45 mentions (35 attributes + 10 doc/comment)` |
| `cargo nextest run -p webfang_core --lib application::vault_search` | 19 passed |
| …de los cuales los 6 de `cosine_similarity` (incl. `dimension_mismatch` → `None`, #1125) | 6 passed |
| `cargo nextest run -p webfang_core --features webfang_core/adaptive-selectors --lib infrastructure::scraper` | 34 passed |
| `cargo nextest run -p webfang_cli --features ai,adaptive-selectors --bin webfang` | 8 passed |
| `bash scripts/check_duplication.sh` | exit 0 — `Duplicated lines (rust): 9244 (baseline: 9250)` |

### Duplicación: el delta, medido con jscpd v5 5.4.0

Los tres árboles medidos con los MISMOS flags del ratchet (`--min-tokens 50 --format rust
--mode mild`), extrayendo `crates/` con `git archive` para no tocar el worktree:

| Árbol | Líneas duplicadas (rust) | Δ |
| --- | --- | --- |
| `main` @ `23b6914e` | **9244** | referencia |
| slice 1, HEAD `1a206357` | **9282** | +38 vs main |
| slice 1 + esta capa | **9244** | **+0 vs main** |

Dos correcciones al brief, ambas medidas:

1. **La atribución de los +32 estaba incompleta.** 29 eran el clon
   `vault_search.rs:403 ↔ embedding_semantic_inspector.rs:217`. Los 3 restantes no eran ruido:
   main mide **9244** en local, y el slice introducía además un **clon intra-archivo de 9
   líneas** en `embedding_semantic_inspector.rs` (el literal `MockEmbedder::new(2, &[…])` de
   4 entradas repetido en `batch_order_is_preserved` y
   `batch_max_splits_into_capped_windows_and_keeps_order`). Con solo el primer arreglo quedaban
   9253 — **rojo todavía**. Extraído como helper de arrange `target_with_one_winner()`; las dos
   pruebas conservan sus propias aserciones, solo se comparte el arrange (el mismo remedio que
   cada descenso de baseline de este repo).
2. **El baseline comprometido (9250) está 6 líneas por encima de la medición local de `main`
   (9244).** No se bumpeó — el ratchet queda en 9244, debajo de los dos.

## Ruta

**Delegated direct** — un writer. Toca ≥2 archivos no triviales (`core/infrastructure/scraper/`,
`webfang_cli/main.rs`, `webfang_ai/{lib,infrastructure_ai}`, test) → dispara el writer trigger del
protocolo ODD. La capa de corrección es un segundo writer sobre el mismo worktree.

## Estado

- [x] Mapeo de superficie (10 puntos, verificado contra `main` @ `23b6914e`)
- [x] Worktree + `.envrc` + `direnv allow` (target aislado: `~/.cache/cargo-target/feat-tier2-port-backed-inspector`)
- [x] T1–T6
- [x] Issue #1977 creado con `status:approved`; PR #1978 abierto
- [x] Commit de work-unit `1a206357` (pusheado)
- [x] Capa de corrección (F1–F6) verificada — commit encima, sin reescribir historia
- [ ] Review nativo + merge (RDD `on`, decidido por global)
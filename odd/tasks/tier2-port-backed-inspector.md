# Feature: Slice 1 — Inspector semántico de Tier 2 sobre `EmbeddingPort`

**ADR:** `docs/adr/0004-onnx-local-removal.md`, paso 3 (condición 2 resuelta 2026-10-11: **migrar**)
**Branch:** `feat/tier2-port-backed-inspector` (base `main` @ `23b6914e`)
**Worktree:** `~/Projects/Rust/webfang-worktrees/feat-tier2-port-backed-inspector`
**Issue:** TBD (lo crea el owner; requiere `status:approved` — label protegida, el agente NO se auto-aprueba)

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

## Verificación observada

Todos los comandos corridos en el worktree con
`CARGO_TARGET_DIR=$HOME/.cache/cargo-target/feat-tier2-port-backed-inspector`.

| Comando | Resultado |
| --- | --- |
| `cargo check -p webfang_cli --features ai,adaptive-selectors --all-targets` | exit 0 |
| `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | **exit 0** (gate exacto de CI, con los ratchets de #516) |
| `cargo fmt --all -- --check` | exit 0 |
| `RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps` | exit 0 (tras corregir un link a `EmbeddingPort` en mi propio doc comment) |
| `cargo nextest run -p webfang_cli --features ai,adaptive-selectors --bin webfang` | 8 passed |
| `tier2_inspector_stays_bound_to_the_local_port` | PASS (aislado) |
| `cargo nextest run -p webfang_core --features webfang_core/adaptive-selectors --lib infrastructure::scraper` | 34 passed (del writer) |
| `cargo nextest run -p webfang_ai --features ai --test erased_engine_ports_test` | 3 passed (del writer) |
| `scripts/check_intra_crate_direction.sh` | exit 0, `allowlisted 44 (entries: 2)` — sin entrada nueva (del writer) |

## Ruta

**Delegated direct** — un writer. Toca ≥2 archivos no triviales (`core/infrastructure/scraper/`,
`webfang_cli/main.rs`, `webfang_ai/{lib,infrastructure_ai}`, test) → dispara el writer trigger del
protocolo ODD.

## Estado

- [x] Mapeo de superficie (10 puntos, verificado contra `main` @ `23b6914e`)
- [x] Worktree + `.envrc` + `direnv allow` (target aislado: `~/.cache/cargo-target/feat-tier2-port-backed-inspector`)
- [x] T1–T6
- [ ] Commit de work-unit
- [ ] PR + review nativo (RDD `on`, decidido por global)
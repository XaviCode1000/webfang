# ADR-0004 tramo D — cleaner sobre EmbeddingPort

## Objetivo
Tramo D del ADR-0004 (pendiente del paso 6): reescribir `SemanticCleanerImpl` sobre el `EmbeddingPort` existente, con traslado, batch/bisección, caché y relativo opt-in (matryoshka). Fija la versión de schema de perfiles (`chars_per_token`/`matryoshka` los consumen los pasos 2–3). **No borra código ONNX aún** (eso es el paso 6).

Rama: `tramo-d/cleaner-embedding-port`
Worktree: `~/Projects/Rust/webfang-worktrees/tramo-d-cleaner-embedding-port`
Test runner: `cargo nextest run` (suite completa tras cada unidad); `cargo check --all-targets --all-features` antes de cada commit.

## Specs
- **S1** — "reescribir `SemanticCleanerImpl` sobre el `EmbeddingPort` existente (el seam existe; lo pendiente es el cleaner, aún genérico sobre `InferenceEngine` en `:215`) + traslado + batch/bisección + caché + relativo opt-in" (ADR:22). El cleaner ya tiene `embedding: Arc<dyn EmbeddingPort>` (`semantic_cleaner_impl.rs:321`) y `from_parts` ungated recibe `Arc<dyn EmbeddingPort>` (`:432-440`); `new` sigue bajo `#[cfg(feature = "ai")]` (`:392-421`).
- **S2** — `clean` deja de llamar `embed` unitario en `try_join_all` y usa `embed_batch` (`semantic_cleaner_impl.rs:472-569`, justificación actual en `:534-535`).
- **S3** — batch/bisección + caché: 400/413 degrada con troceado/bisección (ADR:34); caché de resultados de embeddings en el cleaner.
- **S4** — relativo opt-in: `matryoshka` opt-in desde perfiles (ADR:22, ADR:89).
- **S5** — versión de schema de perfiles fijada: `id → {schema_version, dim, batch_max, query_prefix, passage_prefix, context, chars_per_token, matryoshka: bool}` (ADR:89).
- **S6** — perfiles viven en el repo junto a `ProvidersConfig` (defaults) con overrides en el fichero de config del usuario; el mismo PR que toque el schema migra los perfiles existentes (ADR:89).
- **S7** — `--clean-ai` cambia de semántica: ONNX → endpoint remoto; knob `--embedding-provider` (env `WEBFANG_EMBEDDING_PROVIDER`, flag > env) (ADR:42).
- **S8** — gate `check_clean_ai_feature` (`cli/preflight.rs:1404`) y `:1410` (`check_clean_ai_feature_with`) desaparecen con la feature, con sus tests (ADR:102).
- **S9** — tests `crates/webfang_ai/tests/` (9 targets: `ai_integration`, `batched_inference_smoke`, `session_pool_sizing`, `semantic_cleaner_pipeline`, …) se eliminan con la feature; `crates/webfang_core/tests/adaptive_selectors_gate_test.rs` solo menciona el gate en un doc comment (`:12`) → se ajusta esa línea; snapshots insta de `--clean-ai` cambian y se revisan en el mismo PR (ADR:102).
- **S10** — `--offline` + remoto: exit 78 (guard de regresión, no criterio nuevo; `cli/error.rs:36`, `llm_wire.rs:141-147`) (ADR:116).

## Tasks
| # | Tarea | S# | Route | Commit |
|---|-------|----|-------|--------|
| T1 | `clean` usa `embed_batch` en vez de `try_join_all` de `embed` unitario | S2 | inline | — |
| T2 | Caché de embeddings en `SemanticCleanerImpl` (clave = hash del chunk, `DashMap`) | S3 | inline | — |
| T3 | Bisección/troceado ante 400/413 en el batch de embeddings | S3 | inline | — |
| T4 | Perfiles de modelo versionados (schema_version, matryoshka opt-in) junto a `ProvidersConfig` | S4, S5, S6 | inline | — |
| T5 | Docs: cleaner documenta que `--clean-ai` usará `--embedding-provider` (knob ya existe, uso es paso 6) | S7 | inline | — |

Des-gating de `check_clean_ai_feature`, eliminación de tests de `webfang_ai` y des-gateo de `--clean-ai`/`--offline` al build por defecto son **paso 6**, no tramo D (ver "Fuera de alcance" de este documento).

## Log
- **L1** (user, verbatim): "ok aramas el D"
- **L2** (decision): alcance del tramo D ajustado: des-gating de `check_clean_ai_feature`, eliminación de los 9 targets de tests de `webfang_ai` y des-gateo de `--clean-ai`/`--offline` al build por defecto pertenecen al paso 6 (feature doc, sección "Fuera de alcance"); se eliminan de las tareas T5/T6 originales y se dejan como T5 de docs solo.

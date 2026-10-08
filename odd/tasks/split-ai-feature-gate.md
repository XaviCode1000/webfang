# Feature: #1921 — Partir el gate `ai` al nivel de submódulo

**Issue:** #1921 — `refactor(ai): partir el gate \`ai\` al nivel de submódulo para poder testear el seam sin ONNX`
**Labels:** `type:refactor` (`status:approved` pendiente del owner)
**Branch:** `feat/split-ai-feature-gate`
**Worktree:** `~/Projects/Rust/webfang-worktrees/feat-split-ai-feature-gate`
**Base:** `main` @ `e57b68f5` (ADR-0004 re-scope mergeado; `feat/*` → `main` según `check-topology.sh`)

## Objective

Que el pipeline de texto (chunker, scorer, pruner) de `webfang_ai` compile y sea testeable
sin el feature `ai`, para que el seam `EmbeddingPort` que ADR-0004 paso 2 va a reescribir
tenga un hogar donde no se arrastre ORT.

## Contexto

`crates/webfang_ai/src/lib.rs:13` pone el árbol `infrastructure_ai` entero detrás de
`#[cfg(feature = "ai")]`, y los submódulos no tienen gate individual. Los colaboradores del
cleaner (`HtmlChunker`, `RelevanceScorer`, `LegibleContentPruner`) viven adentro: hoy el
seam no es compilable sin ORT.

Clasificación verificada por imports (`use crate::infrastructure_ai::{inference_engine,
tokenizer,…}`, `ort::`, `tokenizers::`, `hf_hub::`):

| Ungated en este slice | Sigue gated |
| --- | --- |
| `chunk_id`, `sentence`, `chunker`, `markdown_chunker`, `embedding_ops`, `relevance_scorer`, `threshold_config`, `content_pruner` | `inference_engine`, `tokenizer`, `semantic_cleaner_impl` (`hf_hub` `:61-62`), `embedding_adapter` (`:66-68`), `granite_dom_inspector` (`:21-22`), `cache_config`, `compat`, `ai_test_fixture` (`tokenizers` `:81`) |

Dependencias de los módulos de texto: sólo `wide` / `smallvec` / `unicode-segmentation`, que
ya son no-opcionales en `crates/webfang_ai/Cargo.toml`.

## Tasks

- [x] **T1 — `mod.rs`: gatear los 8 módulos ONNX** y sus `pub use` re-exports
      (`semantic_cleaner_impl`, `embedding_adapter`, `inference_engine`, `tokenizer`,
      `granite_dom_inspector`, `cache_config`, `compat`, `ai_test_fixture`).
      Actualizado el doc-comment que decía "This module is feature-gated behind the
      `ai` feature flag" y blindado el doc-example con `# #[cfg(feature = "ai")]`.
- [x] **T2 — `lib.rs`: quitar el gate del módulo padre** y partir el
      `pub use infrastructure_ai::{…}` en grupo texto (ungated) / grupo ONNX (gated).
- [x] **T3 — Verificación dual**: `cargo check`/`nextest` para `webfang_ai` con y sin
      `--features ai`, más `cargo check --workspace`.
- [x] **T4 — Gates completos**: clippy con los ratchets de #516, `cargo fmt --all -- --check`,
      rustdoc `-D warnings`, doc-tests con y sin el feature.
- [ ] **T5 — PR** con `Closes #1921` y `type:refactor` (bloqueado hasta `status:approved`).

### Hallazgo durante la implementación

La primera pasada de verificación falló en 4 de 9 checks con un solo defecto: gateé el
re-export `pub use granite_dom_inspector::GraniteDomInspector` pero **no** su declaración
`pub mod granite_dom_inspector;`, que quedó en la mitad de texto. El módulo importa
`tokenizer` e `inference_engine` (`granite_dom_inspector.rs:21-22`), así que rompía el
build sin `ai`. Corregido con el gate en `mod.rs:127` y anotado en el código, porque la
trampa es fácil de volver a caer: el módulo *parece* texto puro (cosine similarity) pero
no lo es.

## Non-goals

- Reescribir `SemanticCleanerImpl` sobre `EmbeddingPort` (Tramo D paso 2, slice B).
- Wiring de CLI/MCP (`main.rs:661-668`, `mcp_server/mod.rs:178-199`).
- Rename `--max-tokens → --max-chars`.
- Tocar `Cargo.toml`, la feature `ai` de los crates consumidores, o cualquier firma pública.

## Riesgos

| riesgo | mitigación |
| --- | --- |
| El doc-example de `mod.rs` usa `SemanticCleanerImpl` → `cargo test --doc` sin `ai` falla al compilar | Línea oculta `# #[cfg(feature = "ai")]` en el ejemplo (T1) |
| Un submódulo "limpio" que en realidad use un hermano gateado | T3 lo detecta: el build sin `ai` falla en el primer uso |
| Un submódulo ONNX que se(compile sin ort si su dependencia es sólo `hf_hub`, no `ort`) | Mantenerlo gated es conservador y correcto; no hay coste |
| Ratchets de clippy (#516) | T4 corre el comando exacto de CI, no uno laxo |

## Acceptance criteria

Idénticos a los de #1921 (check/nextest con y sin `--features ai`, clippy con ratchets,
fmt, rustdoc, doc-test, diff limitado a 2 archivos).

## Evidence

Verificación delegateada a `gentle-ai-verify`, worktree con target propio
(`seed: cold reason=no-seed key=v1-bb31eba4cf45011e`, sin seed compatible disponible).

| # | check | resultado |
| --- | --- | --- |
| 1 | `cargo check -p webfang_ai` (sin features) | PASS (0.63s) |
| 2 | `cargo check --workspace` (sin features) | PASS |
| 3 | `cargo test --doc -p webfang_ai` (sin features) | PASS (13 doctests) |
| 4 | `cargo nextest run -p webfang_ai` (sin features) | PASS (90/90) |
| 5 | `cargo check -p webfang_ai --features ai` | PASS |
| 6 | `cargo nextest run -p webfang_ai --features ai` | PASS (226/226, 9 skipped — idéntico a base) |
| 7 | `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | PASS (flags exactos de CI) |
| 8 | `cargo fmt --all -- --check` | PASS (verificación, no fixer) |
| 9 | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` | PASS |
| 10 | `cargo test --doc -p webfang_ai --features ai` | PASS (17 doctests) |

Cobertura de CI para el doc-example: job `doc-quality`, `ci.yml:872-873`
(`cargo test --doc --workspace --all-features`), required vía `gate` (`ci.yml:895`).

Nota sobre el paso 3: pasa *vacuamente*. El `# #[cfg(feature = "ai")]` está sobre el
`async fn example()` y el `use` está dentro del cuerpo, así que rustdoc elimina el ejemplo
completo antes de resolver nombres. Sin la línea, el doc-test sin el feature no compilaría.
El paso 10 es el que lo compila positivamente.

Scope del diff: `crates/webfang_ai/src/infrastructure_ai/mod.rs` (+43/−14) y
`crates/webfang_ai/src/lib.rs` (+19/−13). Sin `Cargo.toml`, sin crates consumidores.

Commits: `fabf2517` — `refactor(ai): split the `ai` feature gate down to submodule level`
(sobre `main` @ `e57b68f5`).
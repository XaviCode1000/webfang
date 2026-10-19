# ADR-0004 paso 6 — borrado total de ONNX (des-gating + eliminación)

## Objetivo
Ejecutar el paso 6 del ADR-0004: borrar el crate `webfang_ai`, la feature `ai`, des-gatear `--clean-ai`/`--offline` al build por defecto, y eliminar `check_clean_ai_feature` + sus tests + los 9 targets de tests del crate AI. Fija el marker de ruptura (`BREAKING CHANGE:`) en el commit del paso 6, no en el del paso 5.

**Rama:** `feat/adr0004-paso6-onnx-removal`
**Worktree:** `~/Projects/Rust/webfang-worktrees/feat-adr0004-paso6-onnx-removal`
**ADR:** `docs/adr/0004-onnx-local-removal.md` (secciones *Paso 6*, *Compatibilidad*, *Condición de revisión*, *Superficie afectada*)

## Specs
- **S1** — Borrar crate `webfang_ai` y su entrada en `Cargo.toml` workspace
- **S2** — Eliminar feature `ai` de `crates/webfang_cli/Cargo.toml` y `crates/webfang_mcp/Cargo.toml`; mover `webfang_ai` a dependencia opcional eliminada
- **S3** — Des-gatear `--clean-ai` y `--offline` al build por defecto (eliminar `#[cfg(feature = "ai")]` y `cfg!(feature = "ai")` que los guardan)
- **S4** — Eliminar `check_clean_ai_feature` / `check_clean_ai_feature_with` (`crates/webfang_core/src/cli/preflight.rs:1404-1450`) y sus tests
- **S5** — Eliminar 9 targets de tests en `crates/webfang_ai/tests/` (`ai_integration`, `batched_inference_smoke`, `chunker_regression`, `embedding_port_cleaner_test`, `erased_engine_ports_test`, `mock_inference_benchmark`, `p0_001_measure`, `semantic_cleaner_pipeline`, `session_pool_sizing`)
- **S6** — Ajustar doc comment en `crates/webfang_core/tests/adaptive_selectors_gate_test.rs:12` (elimina mención al gate `ai`)
- **S7** — Marker `BREAKING CHANGE:` en el commit del paso 6 (enmienda ADR 2026-10-16 #1998)
- **S8** — `--clean-ai` resuelve su `EmbeddingPort` vía `--embedding-provider` (env `WEBFANG_EMBEDDING_PROVIDER`, flag > env) — ya implementado en trunk, solo queda documentación
- **S9** — Actualizar docs/CI/AGENTS.md: matriz `webfang_ai`, sección `AI feature`, `hf-hub`, `ort`, `scripts/check_dependency_direction.sh`, `docs/test-inventory.md`, `COMPATIBILITY-MATRIX.md`, `README.md`, `docs/src/ai-onnx-migration.md`
- **S10** — Tests de regresión: `--clean-ai` con endpoint remoto funciona, `--offline` + remoto = exit 78, shims `--ai-model`/`--max-tokens`/`--max-chars` emiten aviso de migración útil

## Tasks
| # | Tarea | S# | Route | Commit |
|---|-------|----|-------|--------|
| T1 | Eliminar crate `webfang_ai` y workspace entry | S1 | inline | ✅ done |
| T2 | Eliminar feature `ai` de CLI/MCP Cargo.toml | S2 | inline | ✅ done |
| T3 | Des-gatear `--clean-ai`/`--offline` en `main.rs` + consumidores (eliminar 11+ cfg sites) | S3 | inline | ✅ done |
| T4 | Eliminar `check_clean_ai_feature` + tests en `preflight.rs` | S4 | inline | ✅ done |
| T5 | Eliminar 9 test targets de `webfang_ai/tests/` (con el crate) | S5 | inline | ✅ done |
| T6 | Ajustar doc comment en `adaptive_selectors_gate_test.rs` | S6 | inline | ✅ done |
| T7 | Actualizar docs/CI/AGENTS.md | S9 | inline | 🔲 pending |
| T8 | Verificar shims `--ai-model`/`--max-tokens`/`--max-chars` emiten aviso | S10 | inline | 🔲 pending |
| T9 | Commit con marker `BREAKING CHANGE:` | S7 | inline | 🔲 pending |
| T10 | Definición canónica única para `AdaptiveSelectorEngine` (placeholder ZST + módulo `real` gateado) | S3 | inline | ✅ done |

Fuera de alcance: re-indexado de vaults y su coste por usuario (ADR:107). Tests del `adaptive_engine` requieren actualización de mocks (follow-up).

## Verificación
```bash
bash scripts/ci_fast_gate.sh                 # lane-aware
cargo fmt --all -- --check                   # --check, nunca el fixer
cargo nextest run                            # full suite
env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps
# Manual: --clean-ai con endpoint remoto funciona
# Manual: --offline + remoto = exit 78
```

## Log
- **L1** (user, verbatim): "ok preparar el trabajo del paso 6 (des-gating + borrado)"
- **L2** (context): clock = 1 minor since step 5 (v2.12.0); need 2 minors per ADR before merge; proceed with preparation
- **L3** (2026-10-19): Core lib compila con y sin feature `adaptive-selectors`; workspace completo (`core`+`mcp`+`cli`) compila con `--all-features`; crate `webfang_ai` eliminado; feature `ai` eliminada de CLI/MCP; placeholder ZST `AdaptiveSelectorEngine` implementado en `adaptive_engine::mod.rs` con impl real en submódulo `real` gateado; 5 consumidores actualizados a import único sin cfg; copia muerta `EngineOptions` en CLI eliminada; tests del `adaptive_engine` requieren actualización de mocks (follow-up separado)
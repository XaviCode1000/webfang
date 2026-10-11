# Feature: docs C2–C3 superficie (ADR-0004, resto de #1923)

**Issue:** TBD — docs follow-up de #1923/#1927 (crear antes del PR; requiere `status:approved`)
**Branch:** `docs/c2c3-superficie` (base `main` @ `b31ba7a9`)
**Worktree:** `~/Projects/Rust/webfang-worktrees/docs-c2c3-superficie`
**ADR:** `docs/adr/0004-onnx-local-removal.md` — C1–C3 + Superficie afectada

## Objective

Cerrar el resto documental de C1–C3 tras #1927 (cleaner sobre `EmbeddingPort` +
`--max-tokens` → `--max-chars` ya mergeado en `52c378d6`): la referencia CLI
regenerada, la lista de flags en instalación, y la superficie afectada que el
ADR exige explícita. Solo docs, cero cambios de código.

## Estado medido (2026-10-11, `main` @ `b31ba7a9`)

- `docs/src/cli-reference.md:575-584` ya trae `--max-chars` + shim `--max-tokens`.
  Pendiente: correr `scripts/gen-cli-reference.sh --check` y regenerar si hay drift.
- `docs/src/installation.md:212` lista `--max-tokens` entre lo ausente en Intel macOS.
  Debe decir `--max-chars` (el shim deprecated no es la vía).
- Superficie ADR (verificar staleness uno por uno, editar solo lo obsoleto):
  `README.md`, `COMPATIBILITY-MATRIX.md`, `docs/test-inventory.md`,
  `AGENTS.md` (sección AI feature), `docs/src/cli-reference.md` (ya visto).
- Fuera de scope de este slice (de `cleaner-on-embedding-port.md`, no editar):
  `infrastructure_ai/mod.rs:46`, `error_class.rs:29`, warn site de
  `webfang_benchmark`, ungateo (ya hecho en slice C).

## Tareas

- [x] **Task 1 — cli-reference:** `scripts/gen-cli-reference.sh --check` verde
  en el primer run (2026-10-11, build `-p webfang_cli --features "ai mcp"`
  ~74 s vía direnv/CARGO_TARGET_DIR aislado): `ok: docs/src/cli-reference.md
  matches the binary help`, exit 0. Sin drift → sin regenerar.
- [x] **Task 2 — installation.md:** lista Intel macOS (`:209-214`) ahora con
  `--max-chars` (swap de una palabra sobre `--max-tokens`); el shim no se
  menciona — el contexto no lo exige y nombrarlo lo reviviría como vía.
- [x] **Task 3 — superficie:** verificado uno por uno el 2026-10-11; solo
  installation.md estaba obsoleto. `README.md` (filas feature `ai`/ONNX,
  nota Intel macOS), `COMPATIBILITY-MATRIX.md` (combos de features),
  `docs/test-inventory.md` (razones `#[ignore]` on-disk, con guard que las
  impone) y `AGENTS.md` (sección AI feature, describe el path local
  hf_hub/ONNX sin afirmar gating) — sin afirmaciones falsas sobre
  flags/modelos/feature `ai`; sin cambios (no reescrituras amplias).
- [x] **Task 4 — verificación + commit:** tres gates verdes (ver tabla) + un
  work-unit commit en la branch (docs, nada de código). Commit: `ad39994a`.

## Alcance autorizado (exacto, no ampliar)

- docs/src/cli-reference.md (solo vía script, bloque entre marcadores)
- docs/src/installation.md
- README.md
- COMPATIBILITY-MATRIX.md
- docs/test-inventory.md
- AGENTS.md (solo sección AI feature, si está obsoleta)

Nada fuera de estos paths. Nada de `crates/`, `scripts/` (solo ejecutar el
generador), ni `CHANGELOG.md` (lo escribe release-plz).

## Criterios de aceptación

- `scripts/gen-cli-reference.sh --check` verde.
- `bash scripts/ci_fast_gate.sh` verde (lane docs).
- `git diff --stat main...HEAD` solo toca los paths autorizados.
- PR con un solo label `type:*` + issue con `status:approved` + rama convencional
  (esta rama ya lo es: `docs/*` → `main`).

## Verificación

| Gate | Resultado |
| :--- | :--- |
| `scripts/gen-cli-reference.sh --check` | ✅ verde 2026-10-11 (`ok: … matches the binary help`, exit 0, sin drift) |
| `bash scripts/ci_fast_gate.sh` | ✅ verde 2026-10-11 (lane docs, PASS=4 FAIL=0 SKIP=1 — markdownlint warn-only ausente) |
| `cargo fmt --all -- --check` | ✅ verde 2026-10-11 (exit 0, sin output; cero cambios de código que formatear) |

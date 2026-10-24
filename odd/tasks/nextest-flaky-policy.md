# nextest-flaky-policy — política de flaky en nextest (#2305)

Issue: #2305 (`status:approved`, `type:chore`). Worktree:
`chore-nextest-flaky-policy`. Base: `cb691aa3` (main).

## Specs

- S1: "Centralizar la política de reintentos de nextest en `.config/nextest.toml`
  y activar el enforcement de flakiness, en cinco pasos **en este orden**."
- S2 (prerrequisito bloqueante): alinear `[profile.ci]` con `[profile.default]`
  en `retries`, `default-filter` y el override del trybuild
  (`illegal_page_transitions_fail_to_compile`, 600s / terminate-after 1) — sin
  adopción todavía.
- S3: verbosidad de `[profile.ci]`: `success-output = "never"`,
  `status-level = "fail"`, `final-status-level = "slow"` ("slow" ya incluye
  "flaky"; `"slow,flaky"` no es sintaxis válida).
- S4: `flaky-result = "fail"` en `[profile.default]` Y `[profile.ci]`, mismo
  commit. Gate de merge: conteo de la lane nocturna retries-0 report-only
  (#2286, #2264 cuarentenado): 0–2 flakes → adelante; >2 → issues de los
  recurrentes ANTES de este paso.
- S5: adoptar `--profile ci` en CI y quitar `--retries`. Verificación:
  correr la lane (no `show-config`).
- S6: decidir el lint #1784: extender `check_nextest_invocation_lint.sh` para
  rechazar flags de política (`--retries`, `--flaky-result`) con fixtures.
- S7 (done-when): ningún flag `--retries` en los workflows; política en
  `.config/nextest.toml`; conteo nocturno adjunto al PR; rollback escrito.

## Tasks

- [x] T1 · Paso 1: alinear `[profile.ci]` con `[profile.default]` (S2) + commit
- [x] T2 · Paso 2: verbosidad de `[profile.ci]` (S3) + commit
- [ ] T3 · Paso 3 (S4): **BLOQUEADO por el pin 0.9.114** — `flaky-result`
  (clave, flag y env) entró en nextest 0.9.131; en 0.9.114 la clave
  desconocida se ignora con warning (falso verde). Seguimiento: #2306.
  Gate de evidencia igual recolectado (L6): conteo 2 ≤ 2, habría cruzado.
- [x] T4 · Paso 4: `--profile ci` en test-core/test-full/test-crossplatform,
  lane nocturna con `[profile.flake-nightly]` (S5, S7) + commit
- [x] T5 · Paso 5: lint de flags de política + fixtures (S6) + commit
- [x] T6 · Gates locales livianos: harness del lint 24/24 (baseline real
  verde), structure harness 15 aserciones, fmt, actionlint, shellcheck.
  Compilación completa deferida a la lane del PR (diff sin código Rust,
  compilación en frío evitada por historial de OOM).
- [x] T7 · PR #2307 (`Closes part of #2305`) + evidencia de la lane adjunta

## Log

- L1: "vamos a atacar la issue 2305" (usuario, 2026-10-24).
- L2: Relevamiento: `--retries 2` en ci.yml (test-core:1084, test-full:1120,
  test-crossplatform:1193) y scripts/ci_fast_gate.sh (×4); lane nocturna
  `--retries 0` (flake-detection, #2264 cuarentenado con `-E`).
  `[profile.ci]` hoy: retries=0, sin default-filter, sin override trybuild,
  success-output=immediate, status-level=all (config muerta y verbosa).
- L3: La issue ya incorpora los tres hallazgos de la revisión previa (obs #2432)
  como "prerrequisito bloqueante" + orden de pasos.
- L4: La lane nocturna NO tenía corridas posteriores a su landing (22-oct,
  #2290); última scheduled 19-oct. Dispatch manual desde main:
  run 40116186564 (24-oct).
- L5: Precedente de adopción de perfil: bug-discovery.yml usa
  `--profile discovery` + `-E`. `poisoned-env` corre sin perfil ni flags.
- L6: Evidencia lane (run 40116186564, main cb691aa3): 4939 tests, 4937
  pasaron, 2 TIMEOUT, 10 skipped. Ambos en webfang_mcp al techo exacto de
  180s: `cli_mcp_export_jsonl_parity_test::cli_vs_mcp_export_jsonl_record_parity`
  (180.085s) y `sitemap_crawl_run_staleness_test::cli_vs_mcp_sitemap_run_record_shape_parity`
  (180.050s). Conteo 2 ≤ 2: el gate de S4 habría cruzado.
- L7: Blocker S4 verificado contra el binario pineado: `flaky-result` no
  existe en 0.9.114 (entró en 0.9.131, changelog); clave desconocida →
  warning "ignoring unknown configuration keys" (calibración con clave
  trampa). `final-status-level = "slow"` sí existe. Issue #2306 abierta para
  el bump; comentario con todo el detalle en #2305.
- L8: Entregado como PR #2307, commits 0c6e882e..164ae7c1 (registro de
  misión + pasos 1, 2, 4, 5). El paso 3 queda fuera por L7.

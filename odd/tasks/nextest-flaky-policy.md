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

- [ ] T1 · Paso 1: alinear `[profile.ci]` con `[profile.default]` (S2) + commit
- [ ] T2 · Paso 2: verbosidad de `[profile.ci]` (S3) + commit
- [ ] T3 · Paso 3 GATEADO (S4): `flaky-result = "fail"` en ambos perfiles + commit
- [ ] T4 · Paso 4: `--profile ci` en test-core/test-full/test-crossplatform y
  lane nocturna sin flag `--retries` (S5, S7) + commit
- [ ] T5 · Paso 5: lint de flags de política + fixtures (S6) + commit
- [ ] T6 · Gates locales: harness del lint, `ci_fast_gate.sh`, parse de la config
- [ ] T7 · PR con `Closes #2305` + evidencia de la lane nocturna adjunta

## Log

- L1: "vamos a atacar la issue 2305" (usuario, 2026-10-24).
- L2: Relevamiento: `--retries 2` en ci.yml (test-core:1084, test-full:1120,
  test-crossplatform:1193); lane nocturna `--retries 0` (flake-detection:1994,
  #2264 cuarentenado con `-E`). `[profile.ci]` hoy: retries=0, sin
  default-filter, sin override trybuild, success-output=immediate,
  status-level=all (config muerta y verbosa).
- L3: La issue ya incorpora los tres hallazgos de la revisión previa (obs #2432:
  flip silencioso a retries 0, no-op de flaky-result bajo retries 0, trybuild
  rojo garantizado) como "prerrequisito bloqueante" + orden de pasos.
- L4: Gate del paso 3: la lane nocturna NO tiene corridas posteriores a su
  landing (22-oct, #2290); última scheduled 19-oct. Dispatch manual desde main:
  run 40192353219-class → 40116186564 (24-oct). Evidencia a extraer de ahí.
- L5: Precedente de adopción de perfil: bug-discovery.yml usa
  `--profile discovery` + `-E`. `poisoned-env` corre sin perfil ni flags (usa
  `default`).

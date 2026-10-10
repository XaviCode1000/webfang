# Feature: #1947 — B2: `NetworkPolicy{mode, allow_cidrs, allow_hosts}` per provider

**Issue:** #1947 — `feat(domain): NetworkPolicy{allow_cidrs, allow_hosts} per provider (B2)`
**Labels:** `type:feature` + `status:approved` (owner instruyó implementar en la sesión 2026-10-09)
**Branch:** `feat/b2-network-policy` (base `origin/main` @ `85cf3e62`, incluye v2.8.0)
**Worktree:** `~/Projects/Rust/webfang-worktrees/feat-b2-network-policy`
**ADR:** `docs/adr/0004-onnx-local-removal.md` — B2, Q6 resuelto opción (b); bloqueante del paso 6

## Contexto

B1 (`AuthSource::None` + `allow_loopback`, #1937/#1915) ya merged. B2 es su generalización:
la allowlist CIDR por provider para endpoints LAN (el caso "vLLM en LAN" del registro
reproducible del paso 6). El tipo `NetworkPolicy` no existe (0 hits en `crates/` verificado
2026-10-09); hoy el guard solo parameteriza loopback exacto.

**Spike paralelo (viabilidad del registro, esta máquina):** vLLM 0.31.0 wheel CPU
(`+cpu-cp38-abi3-manylinux_2_39_x86_64`) + torch 2.13.0+cpu sirve
`ibm-granite/granite-embedding-97m-multilingual-r2` (ModernBertModel, fp32, `--runner
pooling --convert embed`, `VLLM_CPU_KVCACHE_SPACE=1`, `--max-model-len 512`) en un Ryzen
7 5700X AVX2-only sin GPU: **dim=384 confirmada**, servidor en `127.0.0.1:8101`. Sin
`libiomp5.so` preloaded no lo necesita esta build. Evidencia: `/tmp/vllm-spike-setup.log`.

## Slice explícito (por conflictos)

El worktree `adr0004-wiremock-suite` tiene modificado
`crates/webfang_core/src/infrastructure/llm/remote_embedding.rs` (la suite wiremock del
PR-8). El wiring del policy al cliente de embeddings toca ese mismo archivo → **B2 se
parte en dos slices secuenciales**, nunca stacked PR:

- **Slice 1 (este PR, domain-only):** tipo + validación + funciones de decisión en
  `domain/`, con tests de las invariantes. Cero conflicto con ninguna misión abierta.
- **Slice 2 (después del merge de la suite wiremock):** threading del policy por el
  camino de `allow_loopback` (`#1462`: parámetro por-cliente, nunca estado de registro)
  + E2E LAN estilo `ssrf_rfc1918_e2e_test.rs`.

## Invariantes no negociables (FIN-017, del ADR)

1. Link-local (`169.254.0.0/16`, `fe80::/10`), unspecified (`0.0.0.0`/`::`), multicast,
   broadcast y reserved **siempre bloqueadas aunque estén en la allowlist** — sin esto un
   allowlist ingenuo abre `169.254.169.254` (metadata cloud).
2. `100.64.0.0/10` (CGNAT) bloqueada por defecto; opt-in explícito por provider la habilita.
3. CIDR inválido en config → fail loud (patrón #1462: typo es error, nunca silent adopt).
4. Decisión en `domain/`; la allowlist es entrada de las funciones de decisión del
   ssrf_guard, no estado global del registro.

## Tareas

> **Degradación de routing registrada (2026-10-09):** el writer
> (`gentle-ai-worker`) falló 6 veces (3 modos: cuota /tmp agotada, 2 errores
> del modelo en el turno 1, 3 muertes sin reporte final entre turnos 5–9);
> `jd-fix-agent` exige activación formal de Judgment Day (correctamente
> cerrado). Sin `Agent` nativo en este runtime, la implementación la hizo el
> padre inline. El warm-up del target dir (2m10s) y los briefs
> auto-contenidos no alcanzaron: el provider del subagente estaba caído.

- [x] **Task 1 — Tipo `NetworkPolicy` + validación:** `domain/network_policy.rs`
  nuevo (CidrBlock std-only, modo enum, serde mirror fail-loud, 17 tests) +
  campo en `ProviderConfig` (4 tests serde). RED→GREEN observado: fallaban
  exactamente los 8 tests de reglas de validación.
- [x] **Task 2 — Función de decisión policy-aware en `ssrf_guard.rs`:**
  `is_forbidden_ip_with_policy` + `is_always_denied_ip` (9 tests de
  invariantes con policies hand-built para defense-in-depth). RED→GREEN
  observado con stub: fallaban los 4 casos "allowlist permite"; los tests
  además cazaron una inversión en la cola (`policy.allows_ip` →
  `!policy.allows_ip`) antes de que llegara a producción.
- [x] **Task 3 — Invariantes fijadas por tests:** `169.254.169.254` negada
  aunque esté allowlisted (también en forma mapeada `::ffff:`), Teredo y
  prefijos NAT64/6to4 nunca allowlisteables (validation + dial-time),
  CGNAT default-deny + opt-in explícito, RFC1918/ULA solo vía allowlist,
  loopback exclusivamente bajo `allow_loopback` (B1 sin regresión), NAT64
  embebido re-validado contra la policy del IPv4 efectivo.
- [x] **Task 4 — Gate + work-unit commit:** verificación abajo.

## Verificación (evidencia)

| Gate | Resultado |
| :--- | :--- |
| `cargo check --workspace` | ✓ |
| `cargo clippy --all-targets --all-features -- -D warnings -W cognitive_complexity -W too_many_lines` | ✓ |
| `cargo fmt --all -- --check` | ✓ (después de `cargo fmt --all`) |
| `env RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps` | ✓ (fix: shortcut link intra-doc en `//!`) |
| `cargo nextest run -p webfang_core` | ✓ **3699/3699** (21 skip por diseño) |
| `bash scripts/ci_fast_gate.sh` | ✓ **GREEN** (26 PASS / 0 FAIL / 1 SKIP) |

Notas de implementación:

- `allow_hosts` queda validado y normalizado (trim + lowercase), con su
  enforcement en el resolver como slice 2 (comentado en el módulo).
- 5 fixtures de test (llm_wire.rs, provider.rs, remote_embedding.rs ×1,
  auth_source_none_test.rs ×2) necesitaron `network_policy: …default()` por
  completitud de struct literal — todos en `#[cfg(test)]`, riesgo de
  conflicto con la misión wiremock mínimo (una línea por fixture).
- Sin dependencias nuevas (`ipnet` sigue transitivo); CIDR con `std`.
- RED→GREEN honesto en ambas capas: validación (8 tests) y guard (4 tests
  delta + inversión cazada).

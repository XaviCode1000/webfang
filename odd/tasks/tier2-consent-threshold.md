# Slice 3 — Tier 2 Consent + Threshold (ADR-0004 Paso 3)

**Parent issue:** #1977 → slice 1 cerró fuga, slice 2 abrió canal remoto con telemetría, slice 3 cierra UX y calibración.

---

## Objetivo

1. **Consentimiento explícito por corrida**: cuando `--tier2-embedding-provider` apunta a un endpoint remoto, loguear un hash `provider|endpoint` y requerir flag de confirmación (no interactivo: flag `--tier2-consent` o env var).
2. **Umbral por perfil**: consumir `ModelProfile` (ya existe completo) para derivar `semantic_threshold` efectivo en lugar del hardcoded 0.75.
3. **Recalibración del 0.75**: dos cambios separados:
   - Gate real: `AdaptiveSelectorOptions::semantic_threshold` (decide Repaired vs Degraded)
   - Denominador muerto: `ErrorHintConfig::semantic_threshold` (solo deforma drift factor del hint, no gatea nada)

---

## Tasks

### T1 — Consentimiento explícito por corrida
- [ ] Nuevo flag `--tier2-consent` (bool, default false) + env `WEBFANG_TIER2_CONSENT`
- [ ] En `resolve_tier2_remote_embedding`: si el provider resuelto NO es `local_onnx` Y `tier2_consent == false` → error de configuración con mensaje claro: "Tier 2 remoto requiere consentimiento explícito: use --tier2-consent o WEBFANG_TIER2_CONSENT=1"
- [ ] Log de consentimiento: al resolver remoto, emitir `tracing::info!(tier2_consent_hash = %hash, "Tier 2 remote embeddings consent granted")` donde `hash = blake3(provider_id + "|" + base_url)` (truncado a 16 chars)
- [ ] `local_onnx` no requiere consentimiento (no hay egress)
- **Verificación:** `cargo check` + test unitario que sin consentimiento falla, con consentimiento pasa

### T2 — ModelProfile como fuente de threshold
- [ ] `ModelProfile` ya tiene todos los campos necesarios (`dim`, `batch_max`, `query_prefix`, `passage_prefix`, `context`, `chars_per_token`, `matryoshka`, `verified`, `provenance`, `citation`)
- [ ] Nueva función `ModelProfile::semantic_threshold() -> f32` que derive el threshold apropiado:
  - Para modelos verificados (Granite 97M/311M): usar valor calibrado (ej. 0.72 para 384d Matryoshka)
  - Para modelos no verificados: fallback conservador 0.75
  - Documentar la derivación en el doc comment
- [ ] En `build_and_run` / `engine_options` construction: resolver el `ModelProfile` activo (vía `--ai-model` o default) y usar su `semantic_threshold()` en lugar del hardcoded
- [ ] El threshold usado se pasa a `ScrapeSummary.threshold_used` (ya existe desde slice 2)
- **Verificación:** test unitario que verifica threshold por modelo conocido

### T3 — Split del 0.75 (gate real vs denominador muerto)
- [ ] `AdaptiveSelectorOptions::semantic_threshold` → **gate real**: decide `Repaired` vs `Degraded` en el motor adaptativo. Este viene de `ModelProfile::semantic_threshold()`.
- [ ] `ErrorHintConfig::semantic_threshold` → **denominador muerto**: solo se usa en `hint_drift_factor = (threshold - score) / threshold` para escalar el hint. No gatea nada.
- [ ] Cambiar `ErrorHintConfig::semantic_threshold` a un valor neutro (ej. 1.0) o eliminarlo si no tiene sentido semántico como denominador, y documentar por qué.
- [ ] Asegurar que los dos thresholds son independientes: cambiar uno no afecta al otro.
- **Verificación:** test que demuestra que cambiar `ErrorHintConfig::semantic_threshold` no afecta decisión Repaired/Degraded

### T4 — Clippy + fmt + doc check
- [ ] `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`
- [ ] `cargo fmt --all -- --check`
- [ ] `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps`

---

## Archivos afectados

| Archivo | Tipo de cambio |
| :--- | :--- |
| `core/domain/options_spec/llm.rs` | Nuevo flag `--tier2-consent` |
| `core/cli/args/llm.rs` | Campo `tier2_consent` en `LlmArgs` + tests spec |
| `core/application/crawl_options.rs` | Propagación `tier2_consent` |
| `cli/main.rs` | Consent check en `resolve_tier2_remote_embedding` + log hash + threshold desde ModelProfile |
| `core/domain/model_profile.rs` | Método `semantic_threshold()` + doc |
| `core/application/adaptive_engine/mod.rs` | Verificar uso de `semantic_threshold` (gate real) |
| `core/domain/error_hint.rs` | Ajustar `ErrorHintConfig::semantic_threshold` (denominador muerto) |

---

## Criterios de aceptación

1. **Consentimiento**: `--tier2-embedding-provider ollama` sin `--tier2-consent` falla en startup con mensaje claro; con `--tier2-consent` pasa y loguea hash.
2. **Threshold por perfil**: `--ai-model granite-97m` usa threshold derivado del perfil (no 0.75 hardcoded); threshold aparece en `ScrapeSummary`.
3. **Split 0.75**: `AdaptiveSelectorOptions::semantic_threshold` gatea Repaired/Degraded; `ErrorHintConfig::semantic_threshold` solo escala hint drift factor.
4. **CI verde**: clippy, fmt, doc, tests pasan.

---

## Notas de implementación

- **No inventar schema**: `ModelProfile` ya existe completo. Slice 3 solo lo consume.
- **Blake3 para hash**: usar `blake3` crate (ya en deps via `webfang_core`?) o `sha256` truncado. El hash es solo para auditoría, no seguridad.
- **Threshold calibrado**: valores reales vendrán de benchmark slice 6 (ya archivado: 384d Ollama/vLLM verificado). Por ahora usar defaults conservadores documentados.
- **ErrorHintConfig**: está en `core/domain/error_hint.rs`. Revisar uso actual antes de cambiar.

---

## Commits planificados

1. `feat(tier2): add explicit consent flag for remote Tier 2 embeddings`
2. `feat(tier2): derive semantic_threshold from ModelProfile`
3. `feat(tier2): split 0.75 into gate threshold (AdaptiveSelectorOptions) and hint denominator (ErrorHintConfig)`
4. `chore(tier2): clippy + fmt + doc cleanup`
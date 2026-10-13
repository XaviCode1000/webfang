# Slice 2 — Tier 2 Remote Egress + Telemetría (ADR-0004 Paso 3)

**Parent issue:** #1977 (cerrada por slice 1) → este slice abre el canal remoto Tier 2 sin reabrir la fuga.

**Decisión de diseño heredada:** `LocalEmbeddingPort` cierra la fuga por tipo. El canal remoto Tier 2 es un **segundo camino con su propio tipo y su consentimiento**, no "quitar el newtype". Flag separado obligatorio.

---

## Objetivo

Conectar el inspector semántico Tier 2 (`EmbeddingSemanticInspector`) a un endpoint remoto de embeddings (Ollama/vLLM/OpenAI-compatible) cuando el usuario lo pida explícitamente, con:
1. **Flag separado** para Tier 2 remoto (no comparte `--embedding-provider` con vault search)
2. **Correlation ID** en todos los spans de embedding remoto
3. **Acumulador de usage** (prompt_tokens, total_tokens, request_count) por corrida
4. **ScrapeSummary vivo** con campos de embeddings + `threshold_used` impreso al final

---

## Tasks

### T1 — Flag separado para Tier 2 remoto (domain + CLI args)
- [ ] `crates/webfang_core/src/domain/options_spec/llm.rs`: agregar `TIER2_EMBEDDING_PROVIDER` (mirror de `EMBEDDING_PROVIDER`, heading "Tier 2 Embeddings")
- [ ] `crates/webfang_core/src/cli/args/llm.rs`: campo `tier2_embedding_provider: Option<String>` en `LlmArgs` + `FromArgMatches` + tests de paridad spec
- [ ] `crates/webfang_core/src/application/crawl_options.rs`: propagar `tier2_embedding_provider` en `CrawlOptions` + default `None`
- **Verificación:** `cargo check -p webfang_core` + tests `spec_parity_tests` en `llm.rs` pasan

### T2 — Wiring doble canal en main.rs (sin tocar path local)
- [ ] `crates/webfang_cli/src/main.rs`: nueva función `resolve_tier2_remote_embedding(opts)` que usa `opts.tier2_embedding_provider` (misma lógica que `resolve_remote_embedding` pero flag distinto)
- [ ] `build_ai_cleaner`: **inalterado** — sigue devolviendo `LocalEmbeddingPort` para cleaner + vault
- [ ] `resolve_remote_embedding`: **inalterado** — sigue resolviendo vault search con `embedding_provider`
- [ ] En `build_and_run`: llamar `resolve_tier2_remote_embedding` y pasar el port resultante (o `LocalEmbeddingPort` como fallback) a `semantic_inspector`
- [ ] `semantic_inspector` firma cambia: recibe `Option<Arc<dyn EmbeddingPort>>` explícito para Tier 2 (no `vault_ports.embedding_port`)
- **Verificación:** `cargo check -p webfang_cli` + `cargo check -p webfang_core`

### T3 — Correlation ID en spans remotos (remote_embedding.rs)
- [ ] Generar `correlation_id` (UUID v4 o `CorrelationId` existente) al entrar a `post_embeddings` / `embed_batch`
- [ ] Inyectar en spans `embed` (línea 709) y `embed_batch` (línea 734) como campo `correlation_id`
- [ ] Propagar en logs de error (`unauthorized_error`, `last_status_error`, `read_and_parse`)
- **Verificación:** `cargo check -p webfang_core` + inspección visual de spans en test E2E

### T4 — Acumulador EmbeddingUsage por corrida
- [ ] Nuevo tipo `EmbeddingRunMetrics { prompt_tokens: u64, total_tokens: u64, request_count: u64 }` en `remote_embedding.rs` o módulo telemetría
- [ ] Acumular en `post_embeddings` / `read_and_parse` tras parsear `EmbeddingUsage`
- [ ] Exponer via método `metrics(&self) -> EmbeddingRunMetrics` en `RemoteEmbeddingAdapter`
- [ ] Thread-safe (AtomicU64 o Mutex)
- **Verificación:** test unitario que verifica acumulación tras N llamadas mock

### T5 — ScrapeSummary vivo + threshold_used
- [ ] `crates/webfang_core/src/cli/summary.rs`: extender `ScrapeSummary` con:
  - `embedding_requests: usize`
  - `embedding_prompt_tokens: u64`
  - `embedding_total_tokens: u64`
  - `threshold_used: f32` (el umbral semántico efectivo de la corrida)
- [ ] `display_emoji` / `display_ascii`: incluir línea de embeddings si `embedding_requests > 0`
- [ ] Wirear en `build_and_run` / flujo de salida CLI: recolectar métricas del adapter remoto (si existe) + threshold de `CrawlOptions.ai_config.threshold` e instanciar `ScrapeSummary` real
- [ ] Llamar `summary.display(no_color)` al final de corrida exitosa
- **Verificación:** `cargo check` + test de integración que corrida con `--ai` imprime summary con campos de embeddings

### T6 — Tests de integración (E2E wiremock)
- [ ] Test: Tier 2 remoto activado vía flag separado → inspector usa endpoint remoto, vault search usa local (o viceversa)
- [ ] Test: correlation_id aparece en spans y logs
- [ ] Test: contadores de usage se acumulan e imprimen en summary
- [ ] Test: flag sin Tier 2 remoto → LocalEmbeddingPort usado (regresión #1977)
- **Verificación:** `cargo nextest run --test tier2_remote_e2e` (nuevo archivo test)

### T7 — Clippy + fmt + doc check
- [ ] `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`
- [ ] `cargo fmt --all -- --check`
- [ ] `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps`

---

## Archivos afectados (superficies de edición)

| Archivo | Tipo de cambio |
| :--- | :--- |
| `core/domain/options_spec/llm.rs` | Nuevo flag spec |
| `core/cli/args/llm.rs` | Nuevo campo + tests spec |
| `core/application/crawl_options.rs` | Propagación |
| `cli/main.rs` | Wiring doble canal (resolver + inspector) |
| `core/infrastructure/llm/remote_embedding.rs` | Correlation ID + acumulador usage |
| `core/cli/summary.rs` | Extender struct + display |
| `core/tests/tier2_remote_e2e_test.rs` | Nuevo test E2E (wiremock) |

---

## Criterios de aceptación (Definition of Done)

1. **Flag separado funciona:** `--tier2-embedding-provider ollama` abre canal remoto solo para Tier 2; `--embedding-provider ollama` solo para vault search; ambos independientes.
2. **Correlation ID visible:** `tracing` JSONL muestra `correlation_id` en spans `embed`/`embed_batch` de remote embedding.
3. **Usage acumulado:** Al final de corrida con Tier 2 remoto, summary imprime `embedding_requests`, `embedding_prompt_tokens`, `embedding_total_tokens`.
4. **Threshold usado:** Summary incluye `threshold_used` = valor efectivo de `--ai-threshold` (o default).
5. **Regresión #1977 bloqueada:** Sin flag Tier 2, inspector usa `LocalEmbeddingPort` local; vault search respeta su flag propio.
6. **CI verde:** `cargo check`, `clippy`, `fmt`, `doc`, `nextest` todo pasa.

---

## Notas de implementación

- **No inventar schema:** `ModelProfile` ya existe completo; slice 3 lo consumirá. Slice 2 solo abre el canal y telemetría.
- **No tocar `LocalEmbeddingPort`:** Es la garantía de tipo. El wiring nuevo debe respetar que el cleaner y vault ports siguen recibiendo el port local.
- **`ScrapeSummary` resucitar:** Pasar de código muerto a struct instanciado e impreso en cada corrida exitosa.
- **Wiremock:** Tests E2E usan `wiremock::MockServer` simulando `/embeddings` OpenAI-compatible. Ver `crates/webfang_core/tests/common/cli_harness.rs` para patrón.
- **Observabilidad:** Cada cambio en hot path (spans, contadores) debe tener `#[instrument]` o campos estructurados — no string soup.

---

## Commits planificados (work-unit)

1. `feat(tier2): add separate flag for Tier 2 remote embeddings (domain + cli args)`
2. `feat(tier2): wire dual-channel embedding resolver in main.rs`
3. `feat(tier2): add correlation_id to remote embedding spans`
4. `feat(tier2): add EmbeddingUsage accumulator per run`
5. `feat(tier2): revive ScrapeSummary with embedding metrics + threshold_used`
6. `test(tier2): add E2E wiremock tests for dual-channel + telemetry`
7. `chore(tier2): clippy + fmt + doc cleanup`
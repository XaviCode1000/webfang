# ADR 813: Backend óptimo para LLM extraction — OpenAI-compatible vs offline SLM (follow-up #789)

- **Status:** Proposed (benchmark acotado sin implementar SLM)
- **Date:** 2026-08-21
- **Issue:** #813 (follow-up #789 / PR #806 `ca602a1`, #702, research `REPORT.md` §2 open question #2, `findings/F2.md` [9][12])
- **Deciders:** Lead Rust Architect (bridge NotebookLM `Senior_Rust_Developer` 6b7f010b…)
- **Scope:** Decisión documentada con tabla comparativa A/B/C; **fuera de scope** implementar el SLM si el benchmark lo recomienda (follow-up SDD propio).

## 1. Contexto y problema (código real)

`#789` implementó el core LLM-First: trait `LlmPort: Send+Sync { send_completion(LlmRequest)->BoxFuture<Result<LlmResponse,ScraperError>> }` (`domain/llm_port.rs:53`), `OpenAiLlmClient` vía `wreq` Chrome145 POST `{base}/chat/completions` con `response_format=json_object` y `temperature=0.0` (`infrastructure/llm/client.rs:63`), `validation.rs` zero-dep (`type/required/properties/enum/items`, paths `$.a.b[0].c`, mensajes en español), y `LlmExtractionService` con pipeline **schema gate → SSRF gate → robots → fetch → cleaner.clean (HTML nunca al LLM) → chunk budget `max_tokens*8` → loop secuencial `send_completion` → merge dedupe `HashSet<String>` → `validate_record` → envelope** (`application/llm_extraction.rs:44-128`, `CHARS_PER_TOKEN=8`).

DI real: `Container { llm_port: OnceCell<Arc<dyn LlmPort>> }` (`application/container.rs:117`, `L282 get().cloned()`, `L364 with_llm_port` at-most-once, lazy wiring `#759` para no bloquear handshake MCP), pero `LlmExtractionService` guarda `Option<Arc<dyn LlmPort>>` para tests (`None` → `ScraperError::Config("no hay proveedor LLM configurado")` honesto en español). `core/Cargo.toml` feature `ai=[]` marker vacío; peso ONNX/`ort`/`hf_hub` vive solo en `webfang_ai` (cache `~/.cache/huggingface/hub`, lazy-download, sin `include_bytes!`). `cargo build` sin `ai` no crece; `--features ai` documenta Granite 97M ~390MB / 311M ~1.25GB.

La tool MCP `extract_structured` aún **no existe** en `webfang_mcp/src` en este checkout (`grep` 0 resultados, `cargo check --all-targets` verde) — el benchmark se valida a nivel `webfang_core`.

Tensión offline-first vs OpenAI-compatible: inferencia local es CPU-bound masiva (si se ejecuta en el executor Tokio → *thread starvation* → requiere `spawn_blocking` vía `CpuBridge`), mientras que `OpenAiLlmClient` es I/O puro `wreq`. La jerarquía de verdad es **compilador > código real > NotebookLM**; el mentor aporta teoría, `borrow checker` y `cargo check` deciden.

Research reporta: SmolLM2-360M quantizado ~90MB extra, 70-85% JSON válido en schemas de entidad única sin post-procesamiento, vs GPT-4 11.97% inválido en schemas complejos → PARSE 82.3%→98.7% con refinamiento (`F2.md` [9][12]). C híbrida `generate_schema() → CSS/XPath` reutilizable costo cero recurrente (`F2.md` [4]) cubriría >80% casos estables.

## 2. Benchmark acotado (sin implementar modelo)

**Objetivo:** medir las 3 opciones con los **mismos 3 fixtures** y decidir default/feature flags sin engordar binario base (`ai` o `llm-local` feature-gated, lazy `hf_hub`).

### 2.1 Fixtures (schemas que pasan `validate_schema`)

```json
// A) Entidad única flat (producto) — valid_json_rate 70-85% esperado en SLM
{
  "type": "object",
  "properties": {
    "name": { "type": "string" },
    "price": { "type": "number" },
    "currency": { "type": "string", "enum": ["USD","EUR","ARS"] }
  },
  "required": ["name","price","currency"]
}

// B) Lista homogénea — array de objetos
{
  "type": "array",
  "items": {
    "type": "object",
    "properties": {
      "id": { "type": "integer" },
      "title": { "type": "string" }
    },
    "required": ["id","title"]
  }
}

// C) Anidado complejo 3 niveles (company → departments → employees)
// Nota: validator zero-dep valida recursivamente `object.properties` pero en `array.items` solo valida `type` string simple (L `validation.rs`); profundidad 3 se valida superficialmente y el resto lo cubre PARSE-like refinement.
{
  "type": "object",
  "properties": {
    "company_name": { "type": "string" },
    "departments": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "department_name": { "type": "string" },
          "employees": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "employee_name": { "type": "string" },
                "role": { "type": "string" }
              },
              "required": ["employee_name","role"]
            }
          }
        },
        "required": ["department_name","employees"]
      }
    }
  },
  "required": ["company_name","departments"]
}
```

### 2.2 Harness (determinístico, sin red real ni API keys)

- **Ubicación:** `tests/behavioral/llm_benchmark.rs` (o `crates/webfang_core/benches/llm_bench.rs` si se mide `cargo bloat`); hereda `BehavioralTest` + `TempDir` + `wiremock`.
- **Opción A (API externa):** `wiremock::MockServer` con `POST /chat/completions` → respuestas pregrabadas + corpus corrupto (11.97% JSON inválido, `finish_reason=length`). `WEBFANG_DISABLE_SSRF=1` para loopback. Mide `input_tokens:u32`/`output_tokens:u32` reales del `Usage`.
- **Opción B (SLM local):** `StubLlm: LlmPort` determinístico con `spawn_blocking` (simula `ort` CPU) + `std::thread::sleep(120ms)` y `content.len()/8` como `output_tokens`; corpus 100 iteraciones validado contra `validate_record`.
- **Opción C (híbrida):** stub `generate_schema()` → CSS/XPath sintético reutilizable; mide `<5ms` parseo nativo + 0 costo LLM.
- **Métricas por fixture:** `valid_json_rate = valid / iterations *100`, `p50 latency ms` (sort latencies), `chunks`, `input/output_tokens`, `binario +MB` (`cargo build --features ai` vs base), `costo 1k req`.
- **Observabilidad:** `tracing::info!(backend=%backend, model=%model, chunks, valid_rate, p50_ms, "benchmark tick")` + `#[instrument(fields(backend, model))]` en `extract`; `correlation_id` interno `skip` en snapshots via `redact_nondeterministic()` (filtra `p50_ms`, `tokens`).
- **Invariantes Rust:** `!Send` `scraper::Html` nunca cruza `.await` — se limita a bloque síncrono o `spawn_blocking`; `LlmPort: Send+Sync` garantiza `BoxFuture +Send`.

## 3. Tabla comparativa y recomendación

| Dimensión | A: API externa (OpenAI/Ollama, ya existe) | B: SLM local SmolLM2-360M quant ~90MB | C: Híbrida `generate_schema() → CSS/XPath` |
|---|---:|---:|---:|
| **Costo 1k req** | variable pago por uso (≈ $0.001-0.05/extract) | USD 0 (cómputo local) | USD 0 |
| **Latencia p50** | 800-1500 ms (red) | 1200-3000 ms (CPU `spawn_blocking`) | **<5 ms** (parseo nativo) |
| **valid_json_rate** | ~88% (GPT-4 nested, 11.97% inválido sin PARSE → 98.7% con refinement) | **70-85% solo entidad única** (cae en B/C) | **100%** (garantizado por parser) |
| **Binario +MB** | +0 MB | +90MB modelo + `ort` (solo con `--features ai`) | +0 MB |
| **Feature flag** | default siempre disponible | `ai` (lazy `hf_hub`) | `adaptive-selectors` (ya existe, cascade trace) |
| **Offline** | no (requiere `LLM_API_URL`+`LLM_API_KEY`) | **sí** (offline-first) | **sí** |

**Recomendación default (respetando `cargo build` sin `ai` no crece):**

> **Default = C híbrida como primera capa** (`generate_schema()` one-time → CSS/XPath reutilizable, latencia <5ms, 100% válido, 0 costo). **Fallback dinámico → A** vía `OpenAiLlmClient` (OpenAI/Ollama) cuando drift de selectores o schema no cubierto, con `ssrf_gate` estricto y `response_format=json_object`. **B (SLM) estrictamente opt-in** bajo `--features ai` con lazy `hf_hub` a `~/.cache/huggingface/hub` (no `include_bytes!`), solo si usuario necesita offline total y acepta 70-85% sin refinement.

**Feature flags:** **Reusar `ai=[]` marker** (no crear `llm-local` separado). Justificación: `ai` ya aísla `ort`/`hf_hub`/`tokenizers` en `webfang_ai`; añadir `llm-local` duplicaría matriz y CI sin beneficio (90MB ya es el costo documentado de `ai`). El benchmark `cargo build --features ai` debe documentar `+MB` y `cargo build` base permanece ligero (DX). Si en follow-up se necesita Granite vs SmolLM2 separados, se introduce `ai-llm` como sub-feature, no ahora.

**Guía operativa (doc honesta):**
- Offline sin LLM → `extract` retorna `Config("no hay proveedor LLM configurado")` en español (preservado).
- Con API externa → configurar `LLM_API_URL` + `LLM_API_KEY` (o `OLLAMA_URL=http://localhost:11434/v1` con `LLM_API_KEY=dummy`).
- Con SLM offline → `cargo build --features ai` + `AI_MODEL_ID=HuggingFaceTB/SmolLM2-360M` (o Granite default) → descarga lazy `hf_hub`.

## 4. Mitigaciones y riesgos

- **SSRF:** `ssrf_gate(&Url)` allow-list `http/https` + `is_forbidden_literal_host` (loopback/private/link-local/CGNAT/ULA); tests `WEBFANG_DISABLE_SSRF=1`.
- **Token budget sin overlap:** `char_budget = max_tokens*8` validado antes del LLM (over-budget → `Validation`); sin `overlap_rate` hoy → riesgo de corte de registro en frontera de chunk (mitigar con PARSE-like refinement: re-inyectar `SchemaError` path `$.a.b[0].c` al LLM en retry acotado).
- **Dedupe:** `HashSet<String>` sobre `record.to_string()` → sensible a orden de claves/espacios; documentar como limitación y recomendar normalización canónica en follow-up.
- **Validation zero-dep:** sin `$ref`/`allOf`/`format`; fixture C profundo requiere post-validación Serde o refinement loop.
- **`!Send` Html:** nunca retener `scraper::Html` (Rc interno) a través de `.await`; aislar en `spawn_blocking` o bloque síncrono antes de `send_completion`.

## 5. Tests (ningún test contra red real ni keys en repo)

- `wiremock` para A (200/429/503/length), `StubLlm` con `spawn_blocking` para B, stub `generate_schema` para C.
- 3 fixtures × 100 iteraciones → `valid_json_rate` + `p50` + `insta::assert_snapshot!(redact_nondeterministic(...))`.
- Honest error: `LlmExtractionService::new(..., None)` → `extract` == `Config` español; `ssrf_gate("http://127.0.0.1")` bloqueado vs `WEBFANG_DISABLE_SSRF=1` permite wiremock.

## 6. Fuera de scope de esta issue

Implementar el SLM bundled si el benchmark lo recomienda (va en follow-up con SDD propia, incluyendo `cargo bloat` y `ort` session sharing `Arc<Mutex<Session>>` `#648`).

## 7. Bitácora de auditoría (3 rondas con NotebookLM `Senior_Rust_Developer`)

| Ronda | Propuesta NotebookLM | Realidad código local | Ajuste aplicado |
|---|---|---|---|
| R1 | Tensión IO vs CPU, `Container Option`, harness mock `usize` tokens + overlap | `container.rs:117 OnceCell` no Option; `LlmResponse u32`; `CHARS_PER_TOKEN=8` sin overlap; `cargo check` verde | Corregir tipos a `u32`, `OnceCell::new()`/`get().cloned()`, documentar sin overlap |
| R2 | Harness `spawn_blocking` + fixtures flat/array/nested + reusar `ai` vs `llm-local` | `LlmExtractionService Option` (no OnceCell), `ChatMessage` solo Serialize, `validation.rs` sin recursión array profunda, `adaptive-selectors` no es LLM | Distinguir Service Option vs Container OnceCell; schemas corregidos `Clone Serialize`; notar limitación validator; `ai=[]` marker reusado |
| R3 | `docs/llm-backend-decision.md` ADR completo + `OnceCell` vs `Option` invariantes + `!Send` Html + PARSE refinement | `extract_structured` MCP no existe aún (grep 0); `core ai=[]` vacío peso solo en `webfang_ai` | ADR con tabla, harness en `tests/behavioral`, observabilidad `redact_nondeterministic`, decisión default C→A fallback, B opt-in `ai` lazy `hf_hub` |

## 8. Criterio de aceptación verificado

- `cargo build` sin `ai` no crece (core marker vacío, ONNX solo en `webfang_ai`).
- `cargo build --features ai` documenta costo Granite/SmolLM ~90MB-1.25GB lazy.
- `extract` sin config → honest error español `Config("no hay proveedor LLM configurado")`.
- Doc presente en `docs/llm-backend-decision.md` con tabla, fixtures, valid_rate, p50, +MB, recomendación y flags.

---
*Generado vía bridge `OMAR-DEV` notebook `Senior_Rust_Developer` (6b7f010b-7122-4757-a96e-b4fade1043a0), conversaciones `ea00fcdf-2614-4e31-ab0c-bf03b80e621a` (R1-R3), notas `88e9c42d/65a15f7e/2d0aceba`, `gh issue 813`.*

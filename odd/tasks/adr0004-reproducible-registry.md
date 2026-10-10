# Registro reproducible — ADR-0004 paso 6 (M2) contra Ollama y vLLM reales

Fecha: 2026-10-10 · Binario: `feat/b2-network-policy` @ `4958c26b` (PR #1957, build `--features ai`,
target `~/.cache/cargo-target/feat-b2-network-policy/debug/webfang`) · Re-ejecutable: `bash odd/tasks/registry-paso6/run-registry.sh`

> **Dónde vive cada cosa.** Este documento, el script y la config están trackeados acá.
> Los artefactos primarios (traces JSONL, vectores, run logs) quedan en el directorio de
> auditoría `webfang-audit/AUDIT-AI-INFERENCE-FINAL/` — gitignored a propósito (`.gitignore:152-156`),
> por voluminosos y por ser regenerables: el script los reproduce entero desde cero.

## Resumen (lo que el ADR exige: comando + trace + summary backend/modelo/dim)

| Backend | Modelo | Dim (probe startup) | Dim (vector exportado) | Exit | Evidencia |
| :--- | :--- | :--- | :--- | :--- | :--- |
| vLLM 0.31.0 CPU (fp32) | `ibm-granite/granite-embedding-97m-multilingual-r2` | **384** | **384** | 0 | trace `vllm-trace.jsonl`: `remote embedding dim adopted from startup probe`, `remote embedding usage` |
| Ollama (rootless podman, CPU) | `granite-embedding:latest` | **384** | **384** | 0 | trace `ollama-trace.jsonl`: mismos eventos |

Ambas corridas: `--url https://example.com --clean-ai --embedding-provider <id> --trace-file … --output-vectors …`
→ `1 total, 1 succeeded, 0 failed` · `AI cleaning complete: 1 chunks from 1 pages`.

**Cross-check**: los primeros componentes del vector coinciden entre los dos backends
independientes (`-0.067156 / -0.029231 / 0.018178`), confirmando que el pipeline
RAG completo (`prune → chunk → guard → embed → score`) corrió sobre el endpoint real.

## Comando exacto (ambos backends)

```bash
export WEBFANG_CONFIG=odd/tasks/registry-paso6/registry-config.toml   # [[providers]] con auth={source="none"}, allow_loopback=true
webfang --url https://example.com --clean-ai --embedding-provider vllm-granite \
        --trace-file /tmp/registry/vllm-trace.jsonl --output /tmp/registry/out-vllm \
        --export-format jsonl --output-vectors /tmp/registry/vllm-vectors.jsonl
# idem con --embedding-provider ollama-granite para la segunda fila
```

Requisito: binario compilado con `--features ai` (hoy `--clean-ai` sigue gateado por esa feature;
su de-gating al build por defecto es trabajo pendiente del propio ADR).

## Entorno (verificado, no supuesto)

- **Host**: Ryzen 7 5700X (Zen 3, AVX2, **sin AVX512**), sin GPU utilizable (iGPU Renoir,
  sin ROCm) → todo el registro corre por CPU.
- **vLLM**: wheel oficial `vllm-0.31.0+cpu-cp38-abi3-manylinux_2_39_x86_64.whl` (151 MB)
  vía `uv venv --python 3.12`; **no** la imagen `vllm/vllm-openai` (9 GB comprimidos con
  libs CUDA inútiles aquí). Serving: `--runner pooling --convert embed --max-model-len 512
  --dtype float32`, `VLLM_CPU_KVCACHE_SPACE=1` (RAM compartida con 6 builds de cargo).
- **Ollama**: `podman run --name ollama-registry -p 11434:11434 ollama/ollama:latest`
  + `ollama pull granite-embedding` (62 MB).

## Notas que el registro deja fijadas (evitan perder tiempo en la repetición)

1. **El input del scrape debe ser público.** El guard SSRF rechaza targets loopback/RFC1918
   con exit 69 (`ssrf_guard.rs:592`). Lo que debe ser real es el *backend de embeddings*,
   que sí vive en loopback con el opt-in `allow_loopback` por provider (B1, #1462).
2. **`--clean-ai` exige build con `--features ai`** hoy. El de-gating del flag al build por
   defecto es trabajo pendiente del propio ADR ("Flags que sobreviven, des-gateados"), así
   que el build con `ai` es el estado esperado, no un workaround.
3. **La dim no se declara: se adopta del probe de startup** del backend
   (`embedding_dim` omitido → adopt; exit 78 en mismatch). El summary la reporta explícita.
4. El allowlist por CIDR (B2, slice 2) no interviene aquí porque ambos backends están en
   loopback; su caso de aceptación es "vLLM en LAN con `allow_cidrs`", que requiere el
   wiring del slice 2.

## Pendiente para cerrar el gate técnico del paso 6

- Este registro cubre la mitad "registro reproducible" de la condición 1 del owner.
- Falta **B2 slice 2** (wiring `network_policy` al cliente de embeddings + E2E LAN),
  bloqueado por el merge de la suite wiremock (toca `remote_embedding.rs`).
- LM Studio: best effort, sin garantía (no requerido).
- `usage` de Ollama/vLLM: sigue sin verificar y es **opcional, nunca bloqueante** (ADR-0004:30).

#!/usr/bin/env bash
# Registro reproducible del paso 6 del ADR-0004 (M2) contra backends REALES.
# Re-ejecutable de punta a punta en una máquina x86 sin GPU.
# Uso: bash run-registry.sh   (idempotente; deja vLLM y Ollama sirviendo)
set -uo pipefail
REG=/tmp/registry
WEBFANG_BIN=${WEBFANG_BIN:-$HOME/.cache/cargo-target/feat-b2-network-policy/debug/webfang}
PORT_FIXTURE=8791   # no se usa como input (el guard SSRF bloquea loopback); queda como referencia
PORT_VLLM=8101
mkdir -p "$REG" && cd "$REG" || exit 1
say() { echo "[$(date -Is)] $*"; }

# ---------------------------------------------------------------- vLLM (CPU)
# Wheel CPU oficial (no la imagen CUDA de Docker: 9 GB y sin GPU que la use).
say "vLLM: preparando venv CPU en ~/.cache/vllm-spike"
if [ ! -x "$HOME/.cache/vllm-spike/venv/bin/vllm" ]; then
  uv venv --python 3.12 --seed --managed-python "$HOME/.cache/vllm-spike/venv"
  V=$(curl -s https://api.github.com/repos/vllm-project/vllm/releases/latest \
      | python3 -c "import sys,json;print(json.load(sys.stdin)['tag_name'])")
  ASSET=$(curl -s "https://api.github.com/repos/vllm-project/vllm/releases/tags/$V" \
      | python3 -c "import sys,json;d=json.load(sys.stdin);print(next((a['name'] for a in d['assets'] if a['name'].startswith('vllm-') and '+cpu' in a['name'] and 'x86_64' in a['name'] and a['name'].endswith('.whl')),''))")
  uv pip install --python "$HOME/.cache/vllm-spike/venv/bin/python" \
    "https://github.com/vllm-project/vllm/releases/download/$V/$ASSET" --torch-backend=cpu
fi
curl -s -m 2 "http://127.0.0.1:$PORT_VLLM/v1/models" | grep -q data || {
  say "vLLM: arrancando granite-embedding-97m-r2 en fp32 (CPU)"
  IOMP=$(find "$HOME/.cache/vllm-spike/venv" -name libiomp5.so 2>/dev/null | head -1)
  LD_PRELOAD="${IOMP:-}" VLLM_CPU_KVCACHE_SPACE=1 \
    nohup "$HOME/.cache/vllm-spike/venv/bin/vllm" serve \
      ibm-granite/granite-embedding-97m-multilingual-r2 \
      --runner pooling --convert embed --port $PORT_VLLM --max-model-len 512 --dtype float32 \
      > "$REG/vllm-serve.log" 2>&1 &
  disown; for _ in $(seq 1 180); do sleep 5
    curl -s -m 2 "http://127.0.0.1:$PORT_VLLM/v1/models" | grep -q data && break; done
}

# ---------------------------------------------------------------- Ollama
say "Ollama: contenedor rootless en :11434"
podman run -d --name ollama-registry -p 11434:11434 docker.io/ollama/ollama:latest >/dev/null 2>&1 || true
for _ in $(seq 1 60); do sleep 2; curl -s -m 2 http://127.0.0.1:11434/api/tags >/dev/null 2>&1 && break; done
podman exec ollama-registry ollama pull granite-embedding >/dev/null 2>&1 || true

# ---------------------------------------------------------------- config
# allow_loopback habilita el opt-in B1 por provider. El allowlist por CIDR (B2)
# es slice 2 y solo aplica a endpoints fuera de loopback (caso "vLLM en LAN").
cat > "$REG/config.toml" <<'EOF'
[[providers]]
id = "vllm-granite"
display_name = "vLLM CPU (granite-embedding 97m, fp32)"
kind = "open_ai_compatible"
base_url = "http://127.0.0.1:8101/v1"
auth = { source = "none" }
capabilities = ["embedding"]
model = "ibm-granite/granite-embedding-97m-multilingual-r2"
allow_loopback = true

[[providers]]
id = "ollama-granite"
display_name = "Ollama (granite-embedding, CPU)"
kind = "open_ai_compatible"
base_url = "http://127.0.0.1:11434/v1"
auth = { source = "none" }
capabilities = ["embedding"]
model = "granite-embedding:latest"
allow_loopback = true
EOF

# ---------------------------------------------------------------- corridas
# El INPUT debe ser público: el guard SSRF bloquea targets loopback/RFC1918
# (exit 69). Lo que debe ser real es el backend de embeddings, que va en loopback
# con el opt-in allow_loopback por provider.
run() { # $1=provider $2=slug
  say "corrida contra $1"
  WEBFANG_CONFIG="$REG/config.toml" timeout 240 "$WEBFANG_BIN" \
    --url https://example.com --clean-ai --embedding-provider "$1" \
    --trace-file "$REG/$2-trace.jsonl" --output "$REG/out-$2" \
    --export-format jsonl --output-vectors "$REG/$2-vectors.jsonl" \
    > "$REG/$2-run.log" 2>&1
  echo "  exit=$? -> $REG/$2-run.log"
}
run vllm-granite vllm
run ollama-granite ollama

# ---------------------------------------------------------------- summary
python3 - <<'PY'
import json, pathlib
reg = pathlib.Path("/tmp/registry")
for slug in ("vllm", "ollama"):
    vec = reg / f"{slug}-vectors.jsonl"
    if not vec.exists(): print(f"{slug}: SIN vectores"); continue
    d = json.loads(vec.read_text().splitlines()[0])
    probe = [json.loads(l) for l in (reg / f"{slug}-trace.jsonl").read_text().splitlines()
             if "dim adopted from startup probe" in l]
    print(f"{slug}: dim={len(d['embedding'])} probe={probe[0].get('fields',{}).get('dim') if probe else '?'} "
          f"url={d['url']} primeros3={[round(x,6) for x in d['embedding'][:3]]}")
PY
say "registro completo en $REG"

# Migración desde ONNX local

ONNX local **ya no existe** en el producto (ADR-0004, paso 6). Esta página dice **qué cambió, a quién
le afectó y qué hacer ahora**, en el orden de los pasos. La decisión y su secuencia están en
[ADR 0004](https://github.com/XaviCode1000/webfang/blob/main/docs/adr/0004-onnx-local-removal.md).

## Qué cambió y cuándo

| Momento | Qué pasó |
| :--- | :--- |
| Aviso (2 minors antes del corte) | Se **anunció**. No se borró nada: los shims seguían funcionando y `--clean-ai` seguía usando ONNX local |
| Ventana de migración | El `--embedding-provider` resolvió el slot de vault search mientras el ONNX seguía vivo |
| Corte (paso 6 del ADR-0004) | ONNX local, el crate `webfang_ai` y la feature `ai` **se eliminaron**. Este es el estado actual |
| Retiro de los shims | Los shims de migración **se eliminaron**: ya no parsean. Un flag retirado responde con el error estándar de clap (`unexpected argument`, exit 64) |

`cargo build --features ai` falla ahora en la resolución de features: la feature desapareció, y no
deja un shim detrás. Un shim detrás de la feature que se borra no existe, en ningún lenguaje.

## A quién le afecta

| Tu setup | Qué te pasa ahora |
| :--- | :--- |
| `--clean-ai` con el modelo local (Granite 97M / 311M) | **Te afecta.** El cleaner resuelve su endpoint por `--embedding-provider`. Perdiste el offline total |
| `--clean-ai` + `--embedding-provider` ya configurado | Sirve hoy: el flag mueve el slot de vault search **y** alimenta el cleaner vía el `EmbeddingPort` des-gateado |
| `--ai-model`, `AI_MODEL_ID`, `WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE` | **Retirados.** Ya no son flags: `--ai-model` responde con el error estándar de clap (`unexpected argument`, exit 64) y las env se ignoran en silencio. Usá `--embedding-provider` / `WEBFANG_EMBEDDING_PROVIDER` |
| `--max-tokens`, `WEBFANG_MAX_TOKENS` | **Retirados.** `--max-tokens` ya no parsea (error estándar de clap, exit 64) y `WEBFANG_MAX_TOKENS` se ignora. Usá `--max-chars` / `WEBFANG_MAX_CHARS` |
| Vaults con vectores de Granite | Hay que **re-indexar**: el endpoint nuevo no produce vectores comparables. El coste (y el BYO-key) lo pagás vos |
| No usás la superficie AI | Nada. Los avisos no aparecen en corridas que no la tocan |

## Cómo migrar

1. **Configurá el endpoint remoto** — detecta y valida, y escribe la config:

   ```bash
   webfang connect
   ```

   Comprobá que quedó bien, con el detalle de cada valor:

   ```bash
   webfang doctor
   webfang show
   ```

2. **Pasá `--clean-ai` al endpoint remoto — disponible desde el corte.**

   El cleaner resuelve su `EmbeddingPort` por `--embedding-provider`
   (o su env `WEBFANG_EMBEDDING_PROVIDER`; el flag gana al env):

   ```bash
   webfang --url https://example.com --clean-ai --embedding-provider ollama
   ```

   En `--offline` no hay provider de embeddings que sirva: el guard rechaza cualquier endpoint —uno
   remoto público o uno en `127.0.0.1`— y sale con exit 78. El "loopback permitido" del aviso del
   paso 5 se refería al modelo local ONNX, que este paso eliminó; sin ese camino, `--offline` solo
   sirve para corridas sin limpieza semántica. Un endpoint LAN sigue necesitando la allowlist por
   CIDR (`allow_cidrs`), que **no** incluye `100.64.0.0/10` salvo opt-in explícito.

3. **Re-indexá los vaults** que tengan vectores de Granite. No hay forma de detectar la mezcla: el
   fail-closed por dimensión no puede dispararse sobre los vaults que nunca tuvieron `model_id`, que
   son justamente los afectados. Vas a ver un aviso de "legado"; re-indexar es lo que lo cierra.

4. **Renombrá `--max-tokens` → `--max-chars`.** El shim se retiró: `--max-tokens` ya no parsea y
   `WEBFANG_MAX_TOKENS` se ignora en silencio.

## Qué sigue funcionando

`--clean-ai` y `--offline` **empezaron como flags activos siempre** en el corte: se des-gatearon,
quedaron en el build por defecto, sin feature. `--clean-ai` cambió de semántica (ONNX → endpoint
remoto); no fue un shim, es el mismo flag haciendo el trabajo con otro backend.

## El umbral 0.3

El `0.3` por defecto estuvo calibrado para Granite. Retirado el único modelo calibrado, un endpoint
remoto retiene distinto con el mismo número, así que la corrida avisa una vez mientras el umbral sea
el default. El aviso no es decoración: es la señal de que el número ya no significa lo que decía.

Elegí un umbral según tu modelo:

```bash
webfang --url https://example.com --clean-ai --embedding-provider ollama --threshold 0.45
```

El valor efectivo de cada corrida aparece como `threshold_used` en el summary.

## Si recién necesitás ONNX local

El borrado quedó reversible **desde una etiqueta git de recuperación creada antes del borrado**:
tag `onnx-tree/pre-step5` (commit `38ab6a31`). Recuperar ese árbol es el único camino oficial hacia
el ONNX de origen; desde ahí, un resurgimiento requiere un ADR nuevo — no es un rollback ni queda
plementado en la línea actual.

Referencias: [CLI reference](cli-reference.md) ·
[ADR 0004](https://github.com/XaviCode1000/webfang/blob/main/docs/adr/0004-onnx-local-removal.md)

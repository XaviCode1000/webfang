# Migración desde ONNX local

ONNX local se retira del producto. Esta página dice **qué cambia, a quién le afecta y qué hacer**,
con el orden de los pasos. La decisión y su secuencia están en
[ADR 0004](https://github.com/XaviCode1000/webfang/blob/main/docs/adr/0004-onnx-local-removal.md).

## Qué cambia y cuándo

| Momento | Qué pasa |
| :--- | :--- |
| Este release | Se **anuncia**. Nada se borra: los shims siguen funcionando y `--clean-ai` sigue usando ONNX local. La corrida imprime un aviso cuando toca la superficie AI |
| Este release + 1 minor | Ventana de migración |
| Este release + 2 minors | ONNX local y la feature `ai` se eliminan. Es el corte |

`cargo build --features ai` pasa a fallar en la resolución de features: la feature desaparece, y no
deja un shim detrás. Un shim detrás de la feature que se borra no existe, en ningún lenguaje.

## A quién le afecta

| Tu setup | Qué te pasa |
| :--- | :--- |
| `--clean-ai` con el modelo local (Granite 97M / 311M) | **Te afecta.** El cleaner pasará a resolver su endpoint por `--embedding-provider`. Perdés el offline sin red |
| `--clean-ai` + `--embedding-provider` ya configurado | Hoy el flag **no** toca el cleaner: sólo mueve el slot de vault search. Sirve cuando el des-gating llegue (ver el paso 2) |
| `--ai-model`, `AI_MODEL_ID`, `WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE` | Shims. Siguen funcionando ≥2 minors, luego desaparecen |
| `--max-tokens`, `WEBFANG_MAX_TOKENS` | Shim de un shim: ya renombrado a `--max-chars`. Migralo ahora |
| Vaults con vectores de Granite | Hay que **re-indexar**: el modelo nuevo no produce vectores comparables. El coste (y el BYO-key) lo pagas vos |
| No usás la superficie AI | Nada. El aviso no aparece en corridas que no la tocan |

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

2. **Pasá `--clean-ai` al endpoint remoto — cuando este camino esté disponible.**

   > ⚠️ **Hoy este paso no funciona.** `build_ai_cleaner` arma siempre el stack ONNX local
   > (`crates/webfang_cli/src/main.rs`, la llamada a `build_onnx_embedding_port`). El
   > `--embedding-provider` de hoy resuelve el slot de vault search y Tier 2, no el cleaner.
   > El cableado es trabajo del **paso 6** del ADR-0004. El comando de abajo es el destino,
   > no el presente.

   Cuando exista, el cleaner resolverá su `EmbeddingPort` por `--embedding-provider`
   (o su env `WEBFANG_EMBEDDING_PROVIDER`; el flag gana al env):

   ```bash
   # no disponible todavía — no lo pruebes esperando que el cleaner cambie
   webfang --url https://example.com --clean-ai --embedding-provider ollama
   ```

   `--offline` conserva su semántica: loopback permitido, remoto público → exit 78. Un Ollama en
   `127.0.0.1` sigue funcionando sin red; un endpoint LAN necesita la allowlist por CIDR
   (`allow_cidrs`), que **no** incluye `100.64.0.0/10` salvo opt-in explícito.

3. **Re-indexá los vaults** que tengan vectores de Granite. No hay forma de detectar la mezcla: el
   fail-closed por dimensión no puede dispararse sobre los vaults que nunca tuvieron `model_id`, que
   son justamente los afectados. Vas a ver un aviso de "legado"; re-indexar es lo que lo cierra.

4. **Renombrá `--max-tokens` → `--max-chars`.** El shim funciona pero avisa en cada corrida.

## Qué sigue funcionando y hasta cuándo

`--clean-ai` y `--offline` **sobreviven des-gateados**: pasan al build por defecto, sin feature.
`--clean-ai` cambia de semántica (ONNX → endpoint remoto); no es un shim, es el mismo flag haciendo
el trabajo con otro backend.

## El umbral 0.3

El `0.3` por defecto está calibrado para Granite. Retirado el único modelo calibrado, un endpoint
remoto retiene distinto con el mismo número, así que la corrida avisa una vez mientras el umbral sea
el default. El aviso no es decoración: es la señal de que el número ya no significa lo que decía.

Elegí un umbral según tu modelo (mismo camino aún no disponible del paso 2):

```bash
# no disponible todavía — no lo pruebes esperando que el cleaner cambie
webfang --url https://example.com --clean-ai --embedding-provider ollama --threshold 0.45
```

El valor efectivo de cada corrida aparece como `threshold_used` en el summary.

## Si dependés de ONNX local

Comentá en [#1997](https://github.com/XaviCode1000/webfang/issues/1997) diciendo qué comando usás y en
qué versión. Esa issue es el canal que decide el desenlace: sin reclamos registrados, el crate se
borra; con reclamos, pasa a refugio externo "sin soporte".

Una limitación honesta: la ausencia de reclamos **no prueba** ausencia de uso. Hay quien no lee los
avisos. Por eso el borrado queda reversible desde una etiqueta git de recuperación creada antes de
este release.

## Estado real hoy, sin adornos

`--clean-ai` **todavía** exige un binario compilado con la feature `ai`; sin ella sale con exit 78
(`--clean-ai requiere un binario compilado con la feature ai`). El des-gating al build por defecto
es parte del corte, no de este anuncio. El aviso describe el estado al que se llega, no el de esta
versión: por eso la corrida que usa `--clean-ai` sin la feature ve primero el aviso y después ese
error.

Referencias: [CLI reference](cli-reference.md) ·
[ADR 0004](https://github.com/XaviCode1000/webfang/blob/main/docs/adr/0004-onnx-local-removal.md)
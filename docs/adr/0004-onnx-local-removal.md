# ADR 0004: Retirada de ONNX local; embeddings remotos como única vía AI

- **Status:** Proposed
- **Date:** 2026-10-08
- **Deciders:** Project owner
- **Supersedes:** ADR 0003 §5 ("Local ONNX goes dormant, not deleted")
- **Alcance de la supersesión:** solo el §5. Verificado: el resto (§1–4, 6, 7, trade-offs, alternativas) no depende de ONNX y sigue vigente.
- **Related issues:** #1915
- **Sources:** `AUDIT-AI-INFERENCE-FINAL/` (10, 11, 12), FIN-001…047

## Context

ADR 0003 dejó ONNX dormido tras `--features ai` como fallback offline. La auditoría AI-inference (Pasadas 0–5, HEAD `3ac5599`) demuestra que la dormición es la peor de las tres opciones: se paga el árbol en CI (6 lanes `--all-features`; virgen `check -p webfang_ai --features ai` = 1m44s + 1.3 GB en máquina local, sin tests (techo, no cifra: con caché incremental es menor y los tests `ort` cuestan más; "offline OK" puede ser artefacto local), `hf-hub` arrastra `reqwest` (política no-negociable violada bajo `ai`), 390 MB de modelos, y código que se pudre sin dueño. webfang es pipeline; `texto→vector` lo pone el usuario (M1/D3, convergencia real de las tres auditorías).

## Decision

**Q1(a): borrar ONNX del workspace**, con refugio **Q1(c)**: si sobrevive algo, es un crate externo etiquetado "sin soporte", fuera del camino crítico y sin mantenimiento. Secuencia obligatoria (cada paso deja `main` mejor; el borrado es el último):

1. MVP PRs 1–8 + **B2 (`NetworkPolicy` + allowlist, tras responder Q6)** + **C1–C3** (`--max-chars`, este ADR, docs regenerados — exigidos por la condición 4).
2. **Tramo D del plan (08)**: seam `EmbeddingPort` sin borrar + traslado + batch/bisección + caché + relativo opt-in (D6a).
3. Tier 2 con destino explícito (verificado: reimplementable sobre endpoint; umbral 0.75 a recalibrar por modelo).
4. Tramo propio **configuración fácil**: perfiles auto + `connect`/`doctor`/`show` + ajuste manual (o ADR hermano si crece). "Mantener funcionalidades" incluye el flujo, no solo la técnica.
5. **Release de deprecación** (ver Compatibilidad), con la etiqueta git de recuperación creada antes de publicarlo.
6. Borrado total o crate externo sin soporte.

## Condiciones del owner (bloqueantes del paso 6)

1. **M2 funciona de verdad**: Ollama/vLLM por endpoint OpenAI-compatible (`AuthSource::None`, loopback permitido por B2). Criterio: además de wiremock, **prueba manual o de integración contra Ollama y vLLM reales** (su `usage` sigue sin verificar) antes del borrado. LM Studio: best effort, sin garantía (documentación más débil, sin verificar).
2. **Tier 2 explícito**: migrar sobre endpoint o eliminar documentado. Nunca en silencio (FIN-024).
3. **Se asume la pérdida del zero-config offline-total**: M3 cubre sin-inferencia; `--clean-ai` exigirá endpoint del usuario. Se declara en el README.
4. **Reescritura**: `SemanticCleanerImpl` (1096 LOC) sobre `EmbeddingPort`; `--max-tokens → --max-chars` (C2) con `chars_per_token` por perfil (default 3: mejor que 4, insuficiente para CJK —el respaldo real es el degradado por 400/413, nunca la estimación); 400/413 degrada con troceado/bisección.
5. **Deprecación publicada**: el release del paso 5 salió al menos una minor antes del borrado.

## Compatibilidad (ruptura anunciada)

Desaparecen la feature `ai`, `--ai-model`, `AI_MODEL_ID`/`WEBFANG_AI_MODEL_ID` y `WEBFANG_AI_ENGINE`. Un release previo lo anuncia como deprecado (aviso en ejecución y en el CHANGELOG); el borrado llega en el siguiente. Tras el borrado, los flags retirados se conservan como *shims* que devuelven un error útil que explica la migración (`--ai-model` → perfil + endpoint; `ai` feature → M2/M3) durante al menos 2 releases minor; nunca `ConfigError` genérico ni silencio.

## Umbral por defecto huérfano

El 0.3 está calibrado para Granite; retirado el único modelo calibrado, cualquier remoto retiene distinto con el mismo número (además del 0.75 de Tier 2). Este ADR lo declara riesgo aceptado a corto plazo, con una mitigación obligatoria: mientras el umbral sea el default y el modelo no esté calibrado, la ejecución emite un aviso una vez por corrida y registra `threshold_used` en el summary (PR-5). Lo enlaza a su remedio: relativo opt-in (D6a) + golden set (Q4) + flip anunciado (Q7/D6b). Sin ese enlace, el cambio de default sería una regresión silenciosa.

## Condición de revisión

Si las entrevistas (pregunta 3) muestran que los clientes NO correrían el modelo en su VPC, se reevalúa el refugio (c) antes del paso 6: M2-endpoint dejaría de ser camino principal y se decidiría si el crate ONNX externo pasa a tener dueño (excepción explícita al "sin mantenimiento"). Si al llegar al paso 5 no hay al menos 3 entrevistas hechas, el owner decide con la información disponible y lo registra aquí.

## Perfiles de modelo (sustituyen a models.dev como fuente)

models.dev no basta (sin dim/batch/prefijos; `limit.output` ≠ dimensión —mistral 3072 vs 1024 real—; sin Granite-embedding; sin runtimes locales). Perfiles propios versionados: `id → {dim, batch_max, query_prefix, passage_prefix, context, chars_per_token, matryoshka: bool}`. models.dev solo refresco de precio (licencia MIT, con aviso). Reglas defensivas: `dimensions` **omitido por defecto** (vLLM no-Matryoshka lo rechaza con 400), `usage` siempre opcional.

## Egress de Tier 2 (ruta nueva, explícita)

El inspector pasa de local a enviar fragmentos del DOM a un tercero: entra en el consentimiento por hash provider+endpoint (M4/FIN-014) y en el contador de gasto (PR-4/PR-6). Ningún documento anterior lo mencionaba; este ADR lo incluye como requisito, no como nota.

## Migración de vaults

Vectores Granite 384 existentes: si el perfil nuevo también da 384, la mezcla es indetectable (FIN-006). El ADR exige aviso + procedimiento de reindexado (extender el fail-closed por dimensión a `model_id`, conservando el mensaje actual) antes del borrado. Los vaults sin `model_id` se tratan como "legado": se permite abrirlos con aviso explícito y se recomienda reindexar; no se asume que coinciden con el modelo configurado.

## Consecuencias

- Positivas: se cierra el árbol ONNX en CI, `wreq`-only real, M2 como camino principal si las entrevistas (pregunta 3: ¿modelo en su VPC?) lo confirman.
- Negativas: se pierde offline-total con inferencia; coste por uso pasa al usuario (BYO-key); latencia de red en el hot path.
- Neutras: M3 existe y es sólido; el borrado, si nadie usa el crate externo, es trivial.

## Criterios de aceptación de M2 (condición 1, verificables)

- Ollama `127.0.0.1:11434` sin key: `--clean-ai` funciona, 0 `Authorization` inventada.
- vLLM en LAN con `allow_cidrs`: funciona; `100.64.0.0/10` rechazado por defecto.
- `--offline` + loopback: funciona; + remoto público: exit 78.
- Prueba contra Ollama y vLLM reales en verde antes del paso 6.

## Reversibilidad y trazabilidad

Si el borrado falla, el árbol ONNX se recupera desde la etiqueta git del último release que lo contiene (creada antes del paso 5, el release de deprecación). Evidencia primaria: `AUDIT-AI-INFERENCE-FINAL/` (10 decisión, 11 tanda A, 12 tanda B, FIN-001…047) —fuera del repo—; al mover este ADR por PR, su cuerpo ya resume los números clave y el PR adjunta las tablas de paridad (11) y servidores (12).

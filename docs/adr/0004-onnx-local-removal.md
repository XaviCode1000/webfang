# ADR 0004: Retirada de ONNX local; embeddings remotos como única vía AI

- **Status:** Accepted
- **Date:** 2026-10-08
- **Deciders:** Project owner
- **Supersedes:** ADR 0003 §5 ("Local ONNX goes dormant, not deleted")
- **Alcance de la supersesión:** solo el §5. Verificado: el resto (§1–4, 6, 7, trade-offs, alternativas) no depende de ONNX y sigue vigente.
- **Related issues:** #1915, #1917
- **Working papers:** auditoría AI-inference fuera del repo (5 pasadas, HEAD `3ac5599`, 47 hallazgos FIN); los números clave van inline abajo para que este ADR se sostenga solo.

## Context

ADR 0003 dejó ONNX dormido tras `--features ai` como fallback offline. La dormición es la peor de las tres opciones (borrar, dormir, aislar): se paga el árbol en CI (7 invocaciones `--all-features` en 5 jobs —clippy:464, build+test:568,569, doc:870,874, mcp:1159, coverage:1496—; virgen `check -p webfang_ai --features ai` = 1m44s + 1.3 GB en máquina local, sin tests: techo, no cifra —con caché incremental es menor, los tests `ort` cuestan más y "offline OK" puede ser artefacto local), `hf-hub` está fuera de `[features] ai` (`crates/webfang_ai/Cargo.toml:25` vs `:55`) así que **todo** build que incluya `webfang_ai` arrastra `reqwest` 0.12.28 (verificado: `cargo tree -p webfang_ai` sin features) —violación incondicional de la política `wreq`-only, no "bajo ai"—, 390 MB de modelos, y código que se pudre sin dueño. webfang es pipeline; `texto→vector` lo pone el usuario. Nota de base: este ADR se redactó sobre el HEAD auditado `3ac5599`, con una commit de desfase (#1914) a su merge.

## Decision

**Borrar ONNX del workspace**, con refugio: si sobrevive algo, es un crate externo etiquetado "sin soporte", fuera del camino crítico y sin mantenimiento. Secuencia obligatoria (cada paso deja `main` mejor; el borrado es el último):

1. MVP PRs 1–8 (clasificación de errores, validación de vectores, techo+parada, auth sin credencial + offline-por-host, trazabilidad, contadores, prefix+perfiles, suite) + **config de endpoint real** (las env vars de ADR 0003 §1 —`WEBFANG_AI_ENDPOINT/API_KEY`— no existen en código, 0 hits fuera del propio ADR; la config real hoy es `ProvidersConfig` + `AuthSource`: hay que hacer existir el camino documentado antes de simplificarlo) + **B2** (allowlist CIDR para LAN, tras responder Q6) + **C1–C3** (`--max-chars`, este ADR, docs regenerados).
2. **Tramo D**: el seam ya existe (`EmbeddingPort`, `domain/embedding_port.rs:40`, con `RemoteEmbeddingAdapter` implementándolo) —lo pendiente es **reescribir `SemanticCleanerImpl`**, aún genérico sobre `InferenceEngine` (`semantic_cleaner_impl.rs:215)— + traslado + batch/bisección + caché + relativo opt-in.
3. Tier 2 con destino explícito (verificado: reimplementable sobre endpoint, solo consume texto→vectores; umbral 0.75 a recalibrar por modelo).
4. Tramo propio **configuración fácil**: perfiles auto + `connect`/`doctor`/`show` + ajuste manual (o ADR hermano si crece). "Mantener funcionalidades" incluye el flujo, no solo la técnica.
5. **Release de deprecación** (ver Compatibilidad), con la etiqueta git de recuperación creada antes de publicarlo.
6. Borrado total o crate externo sin soporte.

## Condiciones del owner (bloqueantes del paso 6)

1. **M2 funciona de verdad**: añadir la variante `AuthSource::None` (hoy solo existen Keyring/EncryptedFile/Env, `auth_source.rs:55-70`) y permitir loopback/offline-por-host —el loopback ya está permitido (`is_permitted_loopback`, `ssrf_guard.rs:231`, opt-in por provider); B2 agrega la allowlist CIDR, no el loopback—. Criterio: además de wiremock, **prueba manual o de integración contra Ollama y vLLM reales** (su `usage` sigue sin verificar) antes del borrado. LM Studio: best effort, sin garantía.
2. **Tier 2 explícito**: migrar sobre endpoint o eliminar documentado. Nunca en silencio.
3. **Se asume la pérdida del zero-config offline-total**: M3 cubre sin-inferencia; `--clean-ai` exigirá endpoint del usuario. Se declara en el README.
4. **Reescritura**: `SemanticCleanerImpl` (1096 LOC) sobre `EmbeddingPort`; `--max-tokens → --max-chars` con `chars_per_token` por perfil (default 3: mejor que 4, insuficiente para CJK —el respaldo real es el degradado por 400/413, nunca la estimación); 400/413 degrada con troceado/bisección.
5. **Deprecación publicada**: el release del paso 5 salió al menos una minor antes del borrado.

## Compatibilidad (ruptura anunciada)

Desaparecen la feature `ai`, `--ai-model`, `AI_MODEL_ID`/`WEBFANG_AI_MODEL_ID` y `WEBFANG_AI_ENGINE` (flags verificados bajo gate `ai`). Un release previo lo anuncia como deprecado (aviso en ejecución y en el CHANGELOG); el borrado llega en el siguiente. Tras el borrado, los flags retirados se conservan como *shims* que devuelven un error útil que explica la migración (`--ai-model` → perfil + endpoint; `ai` feature → M2/M3) durante al menos 2 releases minor; nunca `ConfigError` genérico ni silencio.

## Umbral por defecto huérfano

El 0.3 está calibrado para Granite y vive en tres sitios (`options_spec/ai.rs:37`, `threshold_config.rs:49`, `semantic_cleaner_impl.rs:124`); retirado el único modelo calibrado, cualquier remoto retiene distinto con el mismo número (además del 0.75 de Tier 2, que no es de similitud sino `ErrorHintConfig::semantic_threshold`, `extraction_quality.rs:11-15`, junto a `lexical_escalation_lower_bound: 0.70`). Este ADR lo declara riesgo aceptado a corto plazo, con una mitigación obligatoria: mientras el umbral sea el default y el modelo no esté calibrado, la ejecución emite un aviso una vez por corrida y registra `threshold_used` en el summary (nombrando los tres sitios). Lo enlaza a su remedio: relativo opt-in + golden set + flip anunciado. Sin ese enlace, el cambio de default sería una regresión silenciosa.

## Condición de revisión

Si las entrevistas (pregunta 3: ¿modelo en su VPC?) muestran que los clientes NO correrían el modelo en su VPC, se reevalúa el refugio antes del paso 6: M2-endpoint dejaría de ser camino principal y se decidiría si el crate ONNX externo pasa a tener dueño (excepción explícita al "sin mantenimiento"). Si al llegar al paso 5 no hay al menos 3 entrevistas hechas, el owner decide con la información disponible y lo registra aquí.

## Perfiles de modelo (sustituyen a models.dev como fuente)

models.dev no basta (sin dim/batch/prefijos; `limit.output` ≠ dimensión —mistral 3072 vs 1024 real—; sin Granite-embedding; sin runtimes locales). Perfiles propios versionados: `id → {dim, batch_max, query_prefix, passage_prefix, context, chars_per_token, matryoshka: bool}`. models.dev solo refresco de precio (licencia MIT, con aviso). Reglas defensivas: `dimensions` **omitido por defecto** (vLLM no-Matryoshka lo rechaza con 400), `usage` siempre opcional. `NetworkPolicy`/`allow_cidrs` no existen como tipo (0 hits): B2 los crea.

## Egress de Tier 2 (ruta nueva, explícita)

El inspector pasa de local a enviar fragmentos del DOM a un tercero: entra en el consentimiento por hash provider+endpoint y en el contador de gasto. Ningún documento anterior lo mencionaba; este ADR lo incluye como requisito, no como nota.

## Migración de vaults

Confirmado: no hay `model_id` en schema ni DTOs (0 hits en infraestructura+aplicación); si el perfil nuevo también da 384, la mezcla con vectores Granite es indetectable. El ADR exige aviso + procedimiento de reindexado (extender el fail-closed por dimensión de `vault_search.rs:130-148` a `model_id`, conservando su mensaje actual) antes del borrado. Los vaults sin `model_id` se tratan como "legado": se permite abrirlos con aviso explícito y se recomienda reindexar; no se asume que coinciden con el modelo configurado.

## Consecuencias

- Positivas: se cierra el árbol ONNX en CI, `wreq`-only real e incondicional, M2 como camino principal si las entrevistas lo confirman.
- Negativas: se pierde offline-total con inferencia; coste por uso pasa al usuario (BYO-key); latencia de red en el hot path.
- Neutras: M3 existe y es sólido; el borrado, si nadie usa el crate externo, es trivial.

## Criterios de aceptación de M2 (condición 1, verificables)

- Ollama `127.0.0.1:11434` sin key: `--clean-ai` funciona, 0 `Authorization` inventada.
- vLLM en LAN con `allow_cidrs` (B2): funciona; `100.64.0.0/10` rechazado por defecto.
- `--offline` + remoto: exit 78 —ya implementado (`EXIT_CONFIG`, `cli/error.rs:36`; `llm_wire.rs:141-147`): es guarda de regresión, no criterio nuevo.
- Prueba contra Ollama y vLLM reales en verde antes del paso 6.

## Reversibilidad y trazabilidad

Si el borrado falla, el árbol ONNX se recupera desde la etiqueta git del último release que lo contiene (creada antes del paso 5, el release de deprecación).

## Alternatives rejected

1. **Mantener dormido (ADR 0003 §5).** La dormición cobra CI, arrastra `reqwest` incondicional y pudre código sin dueño a cambio de un fallback que nadie testea. Rechazada.
2. **Aislar sin borrar (crate en el workspace).** Mantiene el coste de mantenimiento y la matriz de features a cambio de nada una vez que M2 funciona. Solo aceptable como refugio externo sin soporte. Rechazada como estado final.
3. **Reescribir `webfang_ai`.** El crate es correcto; está mal posicionado como default. Rechazada (precedente ADR 0003).

## References

- `crates/webfang_core/src/domain/embedding_port.rs:40` — `EmbeddingPort` (el seam existe)
- `crates/webfang_ai/src/infrastructure_ai/semantic_cleaner_impl.rs:215` — aún genérico sobre `InferenceEngine`
- `crates/webfang_ai/Cargo.toml:25,55` — `hf-hub` fuera de `[features] ai`
- `crates/webfang_core/src/domain/auth_source.rs:55-70` — sin variante `None`
- `crates/webfang_core/src/domain/ssrf_guard.rs:231` — loopback ya permitido
- `crates/webfang_core/src/cli/error.rs:36`, `cli/llm_wire.rs:141-147` — `--offline` ya implementado
- `crates/webfang_core/src/domain/extraction_quality.rs:11-15` — 0.75 es hint, no similitud
- `crates/webfang_core/src/application/vault_search.rs:130-148` — fail-closed por dimensión
- `docs/adr/0003-remote-inference-adapter.md:35` — env vars declaradas, nunca implementadas
- Issues: #1915 (decisión), #1917 (correcciones de esta revisión)

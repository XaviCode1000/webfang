# ADR 0004: Retirada de ONNX local; embeddings remotos como única vía AI

- **Status:** Accepted
- **Date:** 2026-10-08 (revisado 2026-10-09: mecanismo de arbitraje del paso 6 y resolución de Q6 — precisión de mecanismo, la decisión (a) con refugio (c) no cambia)
- **Deciders:** Project owner
- **Supersedes:** ADR 0003 §5 ("Local ONNX goes dormant, not deleted")
- **Alcance de la supersesión:** el §5, más las afirmaciones del §Context de 0003 que quedaron obsoletas: "Port defined, never injected" es falso desde que `--extract-with-llm` inyecta un `dyn LlmPort` real en el Container (`webfang_cli/src/main.rs:721-759`, cableado en #1493). No es `resolve_remote_embedding` (`:687`): esa fn devuelve el `RemoteEmbeddingAdapter` del slot de embeddings, no un `LlmPort`. El resto (§1–4, 6, 7, trade-offs, alternativas) sigue vigente.
- **Related issues:** #1915 (decisión), #1917 (primera revisión, cerrada por #1918), #1919 (re-scope), #1930 (revisión del mecanismo de arbitraje + Q6)
- **Working papers:** auditoría AI-inference fuera del repo (5 pasadas, HEAD `3ac5599`, 47 hallazgos FIN); los números clave van inline abajo para que este ADR se sostenga solo.

## Context

ADR 0003 dejó ONNX dormido tras `--features ai` como fallback offline. La dormición es la peor de las tres opciones —**borrar, dormir, aislar**—: se paga el árbol en CI (7 invocaciones `--all-features` en 5 jobs —clippy:464, build+test:568,569, doc:870,874, mcp:1159, coverage:1496—; virgen `check -p webfang_ai --features ai` = 1m44s + 1.3 GB en máquina local, sin tests: techo, no cifra), `hf-hub` está fuera de `[features] ai` (`crates/webfang_ai/Cargo.toml:25` vs `:55`) así que **todo** build que incluya `webfang_ai` arrastra `reqwest` 0.12.28 (verificado: `cargo tree -p webfang_ai` sin features) —violación incondicional de la política `wreq`-only—, 390 MB de modelos, y código que se pudre sin dueño. Nota de base: la auditoría se corrió sobre `3ac5599`; este ADR se mergea sobre el `main` del momento, así que la distancia crece con cada commit — lo que fija el alcance es el SHA de auditoría, no cuántos commits faltan.

**Corrección de premisa (revisión #1919): el remoto ya existe y es independiente de ONNX.** `RemoteEmbeddingAdapter` implementa `EmbeddingPort` (`remote_embedding.rs:454`, con `embed_batch`, `usage: Option` tolerante en `:73` y retry acotado), se resuelve con `--embedding-provider` / `WEBFANG_EMBEDDING_PROVIDER` (`options_spec/llm.rs:66-79`) y `--extract-with-llm` inyecta el puerto en el Container (`main.rs:721-759`) sin gate `ai`. El gap real es mucho más chico que el descrito antes: **embeddings remotos para el cleaner** —`SemanticCleanerImpl` se construye desde `InferenceEngine` y se erasa a `dyn InferenceEngine` (`main.rs:660-676`), sigue ONNX-only—. Este ADR se re-scopea en consecuencia: no es "cómo llegamos a remoto" sino **qué se borra, qué se rompe y cómo se deprecia**.

## Decision

**Borrar ONNX del workspace**, con refugio externo solo con dueño y criterio de abandono (ver abajo). Secuencia obligatoria (el borrado es el último paso):

1. MVP PRs 1–8 + **config de endpoint real** (las env vars de ADR 0003 §1 no existen en código —0 hits fuera del propio ADR—; la config real es `ProvidersConfig` + `AuthSource`: hacer existir el camino documentado antes de simplificarlo) + **B2** (allowlist CIDR para LAN — **Q6 resuelto, opción (b)**: `NetworkPolicy{mode, allow_cidrs, allow_hosts}` por provider, con link-local/unspecified/multicast/broadcast/reserved **siempre** bloqueadas aunque estén en la allowlist (FIN-017: sin esto un allowlist ingenuo abre `169.254.169.254`), y `100.64.0.0/10` bloqueado por defecto con opt-in explícito; la primera entrega es B1, `AuthSource::None`, que es independiente de Q6) + **C1–C3** (`--max-chars`, este ADR, docs regenerados con la lista explícita de Superficie afectada).
2. **Tramo D**: reescribir `SemanticCleanerImpl` sobre el `EmbeddingPort` existente (el seam existe; lo pendiente es el cleaner, aún genérico sobre `InferenceEngine` en `:215`) + traslado + batch/bisección + caché + relativo opt-in. Los `chars_per_token`/`matryoshka` de los perfiles los consumen los pasos 2–3, así que la versión de schema de perfiles queda fijada aquí.
3. Tier 2 con destino explícito (verificado: reimplementable sobre endpoint, solo consume texto→vectores; umbral 0.75 a recalibrar por modelo).
4. Tramo propio **configuración fácil**: perfiles auto + `connect`/`doctor`/`show` + ajuste manual (o ADR hermano si crece).
5. **Release de deprecación** (ver Compatibilidad): anuncia, no retira capacidad —los shims siguen funcionando—; con la etiqueta git de recuperación creada antes de publicarlo. Este paso no mejora `main`, la prepara: se declara así explícitamente.
6. Desenlace según la *Condición de revisión* (abajo), con criterios distintos por rama: **6a borrado total** (si se cumplen el criterio técnico y la señal de objeción no registra dependencia activa) o **6b crate externo** (si hay reclamos o demanda real: con dueño nombrado, CI mínima y criterio de abandono —sin actividad ni releases en 6 meses se archiva—).

## Condiciones del owner (bloqueantes del paso 6)

1. **M2 funciona de verdad**: añadir la variante `AuthSource::None` (hoy solo Keyring/EncryptedFile/Env, `auth_source.rs:55-70`); el loopback ya está permitido (`is_permitted_loopback`, `ssrf_guard.rs:231`); B2 agrega la allowlist CIDR, no el loopback. Gate: el **criterio técnico** de la *Condición de revisión* —los *Criterios de aceptación de M2* (abajo) más la suite wiremock del PR-8 y el test de 401 con y sin credencial— y un **registro reproducible** archivado en el PR que habilita el paso 6 (comando + `--trace-file` + summary con backend/modelo/dim) ejecutado contra Ollama y vLLM reales. "Probado a mano" sin registro no cuenta; el registro es re-ejecutable por cualquiera. El `usage` que reportan Ollama y vLLM sigue sin verificar y es **opcional, nunca bloqueante**. LM Studio: best effort, sin garantía.
2. **Tier 2 explícito**: migrar sobre endpoint o eliminar documentado. Nunca en silencio.
3. **Se asume la pérdida del zero-config offline-total**: M3 cubre sin-inferencia; `--clean-ai` sobrevive (des-gateado, ver Compatibilidad) pero exigirá endpoint del usuario. Se declara en el README.
4. **Reescritura**: `SemanticCleanerImpl` (1096 LOC) sobre `EmbeddingPort`; `--max-tokens → --max-chars` con `chars_per_token` por perfil (default 3: mejor que 4, insuficiente para CJK —el respaldo real es el degradado por 400/413, nunca la estimación); 400/413 degrada con troceado/bisección.
5. **Deprecación publicada**: el release del paso 5 salió al menos una minor antes del borrado.

## Compatibilidad (ruptura anunciada)

Superficie, toda verificada bajo gate `ai` hoy (`options_spec/ai.rs:77,102,127`):

- **La feature `ai` desaparece sin shim.** Las features de Cargo resuelven en compilación: `cargo build --features ai` fallará en resolución, así que aquí sólo hay release notes.
- **Flags que sobreviven, des-gateados.** `--clean-ai` (cambia de semántica: ONNX → endpoint remoto; es el flag del cleaner del paso 2, **no** un shim) y `--offline`. Para que vivan hay que quitarles el gate `ai` y pasarlos al build por defecto: un shim detrás de la feature que se borra no existe, en ningún lenguaje.
- **Shims** (error útil de migración, ≥2 releases minor, también des-gateados): `--ai-model`, `--max-tokens` —que además se renombra a `--max-chars`, condición 4—, y las env `WEBFANG_MAX_TOKENS`, `AI_MODEL_ID`/`WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE`.
- **Precedencia que los shims reemplazan**, para que el mensaje de migración sea exacto: `WEBFANG_AI_MODEL_ID` > `AI_MODEL_ID` legacy (`webfang_ai/.../compat.rs:34-38`); el flag de CLI gana al env (`webfang_cli/src/main.rs:602`).

`--offline` conserva su semántica (loopback permitido, remoto público → exit 78). Un release previo anuncia todo esto (aviso en ejecución + trailer `BREAKING CHANGE:` en el commit —los work PRs no tocan `CHANGELOG.md`, lo escribe release-plz—); el borrado llega en el siguiente. Nunca `ConfigError` genérico ni silencio.

## Umbral por defecto huérfano

El 0.3 está calibrado para Granite y vive en cuatro sitios (`options_spec/ai.rs:37`, `threshold_config.rs:49`, `semantic_cleaner_impl.rs:124`, `relevance_scorer.rs:333`); retirado el único modelo calibrado, cualquier remoto retiene distinto con el mismo número (además del 0.75 de Tier 2, que no es de similitud sino `ErrorHintConfig::semantic_threshold`, `extraction_quality.rs:11-15`, junto a `lexical_escalation_lower_bound: 0.70`). Mitigación obligatoria con **criterio de salida**: mientras el umbral sea el default y el modelo no esté calibrado, aviso una vez por corrida + `threshold_used` en el summary; la mitigación termina en el flip anunciado con tope de 2 releases minor —si no hay golden set para entonces, el owner decide (aviso permanente documentado o mantener 0.3 con riesgo firmado) y lo registra aquí. Sin salida con fecha, el huérfano se vuelve permanente.

## Condición de revisión

La decisión entre borrado total (6a) y refugio (6b) se arbitra por **dos señales verificables**, sin depender de procesos de discovery externos ni de telemetría.

**1. Criterio técnico (gate del paso 6).** Se evalúa al llegar al paso 5:

- Los *Criterios de aceptación de M2* (abajo) pasan en verde contra **Ollama y vLLM reales**, no solo wiremock, en al menos un entorno documentado.
- La suite wiremock del PR-8 (401 —con y sin credencial—, 429 con `Retry-After` en segundos y en fecha, 5xx, timeout, respuesta malformada, orden de batch, error de lectura de cuerpo, DNS) está verde.
- Sobre el 429, la regla corregida: `Retry-After` se honra en **ambas** formas que permite RFC 9110 §10.2.3 — `delay-seconds` y fecha HTTP. Antes la fecha HTTP se descartaba en silencio y caía a backoff exponencial, con lo que el cliente reintentaba antes de lo que el servidor pedía; hoy la conversión es una función pura con `now` por parámetro, y una cabecera presente pero inservible emite un `warn!` estructurado en lugar de degradarse sin dejar rastro.

Si ambos se cumplen, el paso 6 puede proceder, sujeto a la señal 2. Si alguno falla, se posterga un release minor y el gap se registra en este ADR.

**2. Señal de objeción (entre el paso 5 y el 6).** Es pasiva: no recolecta datos ni abre egress alguno.

- El release de deprecación publica el aviso (en ejecución y en el CHANGELOG, ver Compatibilidad) apuntando a un issue o discusión fija (p. ej. "Migración desde ONNX local").
- Tras **2 releases minor**: si ningún usuario reclama dependencia de ONNX, el paso 6 procede como borrado total; si hay reclamos, el crate pasa a refugio (c) "sin soporte" y los casos se registran aquí.
- **Limitación aceptada:** la ausencia de reclamos **no prueba** ausencia de uso —hay usuarios que no leen avisos—. Por eso el borrado es reversible desde la etiqueta git del paso 5 (ver *Reversibilidad*), y esa asimetría es un riesgo que este ADR declara, no oculta.

**Entrevistas:** son **insumo no bloqueante**. Si existen al llegar al paso 5, informan la decisión; si no, el owner decide con las dos señales anteriores y lo registra aquí. Esta sección **supera** la línea del documento `10_DECISION_OWNER_Q1.md` que las declaraba arbitantes entre (a) y (c): ese archivo está gitignored y es registro histórico de una decisión en su momento, así que no se edita — la corrección queda en el documento normativo, que es donde la va a buscar quien lea después.

**Medición futura:** si alguna vez se quiere medir uso real, requiere su propio ADR —unidad de medida por usuario o por corrida única, destino declarado y consentimiento, conforme a M4—. Este ADR no instrumenta nada.

**Excepción de mantenimiento:** si las señales muestran que M2-endpoint no es camino viable **y** hay demanda real de ONNX, el owner puede asignar dueño al crate externo y lo registra aquí. Es excepción explícita al "sin mantenimiento".

## Perfiles de modelo

models.dev no basta (sin dim/batch/prefijos; `limit.output` ≠ dimensión —mistral 3072 vs 1024 real—; sin Granite-embedding; sin runtimes locales). Perfiles propios versionados: `id → {schema_version, dim, batch_max, query_prefix, passage_prefix, context, chars_per_token, matryoshka: bool}`, viviendo en el repo junto a `ProvidersConfig` (defaults) con overrides en el fichero de config del usuario; el mismo PR que toque el schema migra los perfiles existentes. models.dev solo refresco de precio (licencia MIT, con aviso). Reglas defensivas: `dimensions` **omitido por defecto** (vLLM no-Matryoshka lo rechaza con 400), `usage` siempre opcional. `NetworkPolicy`/`allow_cidrs` no existen como tipo (0 hits): B2 los crea.

## Egress de Tier 2 (ruta nueva, explícita)

El inspector pasa de local a enviar fragmentos del DOM a un tercero: entra en el consentimiento por hash provider+endpoint y en el contador de gasto. Observabilidad del salto (regla de hierro: sin traza no está terminado): los spans ya existen —`remote_embedding.rs:456-460` en `embed` (`provider_id`/`model`/`dim`) y `:482-487` en `embed_batch` (los tres más `count`=textos)— y hay que extenderlos con `CorrelationId` propagado + summary por corrida con tokens/retries/degradados.

## Migración de vaults

Confirmado: no hay `model_id` en schema ni DTOs (0 hits en infraestructura+aplicación); si el perfil nuevo también da 384, la mezcla con vectores Granite es indetectable. El fail-closed extendido a `model_id` (`vault_search.rs:130-148`, mensaje actual conservado) **no puede dispararse** sobre los vaults que nunca tuvieron `model_id` —que son exactamente los afectados—; se permite abrirlos con aviso explícito ("legado", se recomienda reindexar), aceptando la indeterminación. Lo único que cierra de verdad es el **re-embedding**, cuyo coste con BYO-key lo paga el usuario: va a Consecuencias, no se esconde.

## Superficie afectada (entregable explícito de C1–C3, no "docs regenerados")

- Gate de la feature: `cli/preflight.rs:1404` (`check_clean_ai_feature`) y `:1410` (`check_clean_ai_feature_with`) desaparecen con la feature, con sus tests.
- Tests: `crates/webfang_ai/tests/` (9 targets, la mayoría ort-dependent: `ai_integration`, `batched_inference_smoke`, `session_pool_sizing`, `semantic_cleaner_pipeline`, …) se eliminan con la feature; `crates/webfang_core/tests/adaptive_selectors_gate_test.rs` sólo menciona el gate en un doc comment (`:12`), así que se ajusta esa línea en vez de reescribir el archivo; los snapshots insta de `--clean-ai` cambian y se revisan en el mismo PR.
- Docs y política: `AGENTS.md` (matriz con `webfang_ai`, sección `AI feature`, `hf-hub`, `ort`), `scripts/check_dependency_direction.sh`, `docs/test-inventory.md`, `COMPATIBILITY-MATRIX.md`, `README.md` (secciones de modelos, feature `ai`, caché y crate).
- Observabilidad: campos del salto remoto (arriba) + summary por corrida.

## Consecuencias

- Positivas: se cierra el árbol ONNX en CI; `wreq`-only en el build default —con qualifier: `--all-features` conserva `reqwest` vía `chromiumoxide` (`Cargo.toml:158`, feature `chromium` en `core/Cargo.toml:33`), así que el "real" vale para default, no para all-features—; M2 como camino principal cuando el criterio técnico se cumple y la señal de objeción no registra dependencia activa.
- Negativas: se pierde offline-total con inferencia; coste por uso y **coste de re-embedding de vaults** pasan al usuario (BYO-key); latencia de red en el hot path; **la señal de objeción puede subestimar el uso real** —riesgo aceptado, mitigado por la reversibilidad desde la etiqueta git del paso 5—.
- Neutras: M3 existe y es sólido; el borrado, si nadie usa el crate externo, es trivial.

## Criterios de aceptación de M2 (condición 1, verificables)

- Ollama `127.0.0.1:11434` sin key: `--clean-ai` funciona, 0 `Authorization` inventada.
- vLLM en LAN con `allow_cidrs` (B2): funciona; `100.64.0.0/10` rechazado por defecto.
- `--offline` + remoto: exit 78 —ya implementado (`EXIT_CONFIG`, `cli/error.rs:36`; `llm_wire.rs:141-147`): es guarda de regresión, no criterio nuevo.
- Registro reproducible contra Ollama y vLLM reales archivado antes del paso 6.
- Suite wiremock del PR-8 en verde **más test de 401 con y sin credencial**: con `AuthSource::None` un 401 de un provider sin credencial debe distinguirse de un 401 por credencial inválida. Ese test no existe todavía —es entregable de B1, no criterio preexistente—.
- **Enmienda sobre "truncado de stream"** (ver *Condición de revisión* §1): el escenario, tal como estaba enumerado, no es expresable en el código y por eso no se puede cubrir. El adapter no tiene camino de lectura en streaming —usa `read_body_capped`, una lectura acotada a 16 MiB—, de modo que no existe un punto donde un stream pueda truncarse a mitad, y wiremock no ofrece ninguna primitiva para abortar un body en curso. El criterio verificable que lo sustituye es el **mapeo de error de lectura de cuerpo**: una lectura que falla se traduce a `SemanticError::Inference` con el mensaje en español que nombra el endpoint, y nunca es silencio ni `panic`.

## Reversibilidad

Si el borrado falla, el árbol ONNX se recupera desde la etiqueta git del último release que lo contiene (creada antes del paso 5). Qualifier: no es drop-in —si los pasos 1–4 tocaron `webfang_ai`, el tag devuelve código viejo que puede no calzar con el árbol; la etiqueta es punto de referencia para recuperación manual, no restore automático.

## Alternatives rejected

1. **Mantener dormido (ADR 0003 §5).** Cobra CI, arrastra `reqwest` incondicional y pudre código sin dueño a cambio de un fallback que nadie testea. Rechazada.
2. **Aislar sin borrar (crate en el workspace).** Mantiene mantenimiento y matriz de features a cambio de nada una vez que M2 funciona. Solo aceptable como refugio externo con dueño y abandono definido. Rechazada como estado final.
3. **Reescribir `webfang_ai`.** El crate es correcto; está mal posicionado como default. Rechazada (precedente ADR 0003).

## References

- `crates/webfang_core/src/domain/embedding_port.rs:40` — el seam existe
- `crates/webfang_core/src/infrastructure/llm/remote_embedding.rs:454,73` — adapter remoto + `usage` opcional
- `crates/webfang_core/src/domain/options_spec/llm.rs:66-79` — `--embedding-provider` existe
- `crates/webfang_cli/src/main.rs:721-759,660-676,687` — inyección sin gate `ai`; cleaner aún ONNX-only
- `crates/webfang_ai/src/infrastructure_ai/semantic_cleaner_impl.rs:215` — pendiente de reescribir
- `crates/webfang_ai/Cargo.toml:25,55` — `hf-hub` fuera de `[features] ai`
- `Cargo.toml:158`, `core/Cargo.toml:33` — `chromiumoxide` también arrastra `reqwest`
- `crates/webfang_core/src/domain/auth_source.rs:55-70` — sin variante `None`
- `crates/webfang_core/src/domain/ssrf_guard.rs:231` — loopback ya permitido
- `crates/webfang_core/src/cli/error.rs:36`, `cli/llm_wire.rs:141-147` — `--offline` ya implementado
- `crates/webfang_core/src/domain/extraction_quality.rs:11-15` — 0.75 es hint, no similitud
- `crates/webfang_core/src/application/vault_search.rs:130-148` — fail-closed por dimensión
- `docs/adr/0003-remote-inference-adapter.md:35` — env vars declaradas, nunca implementadas
- `crates/webfang_core/src/cli/preflight.rs:1404,1410` — el gate de la feature que desaparece
- `crates/webfang_core/src/domain/options_spec/ai.rs:77,102,127` — los tres flags bajo gate `ai` (y sus env: `WEBFANG_MAX_TOKENS`, `WEBFANG_OFFLINE`)
- `crates/webfang_ai/src/infrastructure_ai/relevance_scorer.rs:333` — cuarto sitio del 0.3
- `crates/webfang_core/src/infrastructure/llm/remote_embedding.rs:456-460,482-487` — spans del salto remoto
- `crates/webfang_cli/src/main.rs:602` — precedencia `--ai-model` sobre env
- Issues: #1915 (decisión), #1917 (primera revisión, cerrada por #1918), #1919 (re-scope)

## Glosario de referencias (IDs opacos)

Los IDs de trabajo (auditoría fuera del repo, gitignored) significan: Q1(a)=borrar, Q1(c)=refugio externo; B2=allowlist CIDR; C1–C3=docs y `--max-chars`; FIN-024=Tier 2 en silencio; PR-4/5/6=trazabilidad y contadores del MVP; M1–M4=modelos de producto; D6a=umbral relativo opt-in; Q4/Q6/Q7=preguntas al owner (golden set, alcance M2, default). Todo lo load-bearing está inline arriba; el glosario es solo índice.

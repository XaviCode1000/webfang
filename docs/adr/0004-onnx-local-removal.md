# ADR 0004: Retirada de ONNX local; embeddings remotos como única vía AI

- **Status:** Accepted — implemented and shipped. La retirada se mergeó en #2072 (`e9b7217a`) y se publicó como **v3.0.0** (tag sobre `63d912ae`, 2026-10-20). El *Addendum 2026-10-19* al final registra el desenlace 6a, la decisión del umbral y el follow-up de shims.
- **Date:** 2026-10-08 (revisado 2026-10-09: mecanismo de arbitraje del paso 6 y resolución de Q6 — precisión de mecanismo, la decisión (a) con refugio (c) no cambia; revisado 2026-10-11: corrección factual — el 0.75 de Tier 2 son dos números con roles opuestos — y registro de la condición 2 resuelta como migrar, #1975; revisado 2026-10-16: el marker de ruptura se mueve del paso 5 al paso 6 — leer *Enmienda 2026-10-16* en *Compatibilidad* —, `--clean-ai` queda atado a `--embedding-provider`, la etiqueta de recuperación y el canal de objeción pasan a ser concretas, #1998)
- **Deciders:** Project owner
- **Supersedes:** ADR 0003 §5 ("Local ONNX goes dormant, not deleted")
- **Alcance de la supersesión:** el §5, más las afirmaciones del §Context de 0003 que quedaron obsoletas: "Port defined, never injected" es falso desde que `--extract-with-llm` inyecta un `dyn LlmPort` real en el Container (`webfang_cli/src/main.rs:721-759`, cableado en #1493). No es `resolve_remote_embedding` (`:687`): esa fn devuelve el `RemoteEmbeddingAdapter` del slot de embeddings, no un `LlmPort`. El resto (§1–4, 6, 7, trade-offs, alternativas) sigue vigente.
- **Related issues:** #1915 (decisión), #1917 (primera revisión, cerrada por #1918), #1919 (re-scope), #1930 (revisión del mecanismo de arbitraje + Q6), #1975 (corrección 0.75 Tier 2 + condición 2 resuelta), #1997 (canal de objeción del paso 5, abierto), #1998 (enmienda del marker de ruptura), #1999 (aviso en ejecución + aviso de umbral + guía de migración)
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
5. **Release de deprecación** (ver Compatibilidad): anuncia, no retira capacidad —los shims siguen funcionando—; con la etiqueta git de recuperación creada antes de publicarlo, ya hecha: **`onnx-tree/pre-step5`** (commit `38ab6a31`, ver *Reversibilidad*). Es un **minor sin marker de ruptura**: el anuncio lo hacen el aviso en ejecución y las release notes, no el semver (*Enmienda 2026-10-16*). El canal de objeción que el aviso nombra es **#1997**. Este paso no mejora `main`, la prepara: se declara así explícitamente.
6. Desenlace según la *Condición de revisión* (abajo), con criterios distintos por rama: **6a borrado total** (si se cumplen el criterio técnico y la señal de objeción no registra dependencia activa) o **6b crate externo** (si hay reclamos o demanda real: con dueño nombrado, CI mínima y criterio de abandono —sin actividad ni releases en 6 meses se archiva—).

## Condiciones del owner (bloqueantes del paso 6)

1. **M2 funciona de verdad**: añadir la variante `AuthSource::None` (hoy solo Keyring/EncryptedFile/Env, `auth_source.rs:55-70`); el loopback ya está permitido (`is_permitted_loopback`, `ssrf_guard.rs:231`); B2 agrega la allowlist CIDR, no el loopback. Gate: el **criterio técnico** de la *Condición de revisión* —los *Criterios de aceptación de M2* (abajo) más la suite wiremock del PR-8 y el test de 401 con y sin credencial— y un **registro reproducible** archivado en el PR que habilita el paso 6 (comando + `--trace-file` + summary con backend/modelo/dim) ejecutado contra Ollama y vLLM reales. "Probado a mano" sin registro no cuenta; el registro es re-ejecutable por cualquiera. El `usage` que reportan Ollama y vLLM sigue sin verificar y es **opcional, nunca bloqueante**. LM Studio: best effort, sin garantía.
2. **Tier 2 explícito**: migrar sobre endpoint o eliminar documentado. Nunca en silencio. **Resuelto 2026-10-11: migrar sobre endpoint**, con el paquete de egress completo (§Egress de Tier 2) — la eliminación documentada queda descartada porque degrada `--adaptive-selectors` a Tier 1 en silencio, exactamente lo que esta condición prohíbe.
3. **Se asume la pérdida del zero-config offline-total**: M3 cubre sin-inferencia; `--clean-ai` sobrevive (des-gateado, ver Compatibilidad) pero exigirá endpoint del usuario. Se declara en el README.
4. **Reescritura**: `SemanticCleanerImpl` (1096 LOC) sobre `EmbeddingPort`; `--max-tokens → --max-chars` con `chars_per_token` por perfil (default 3: mejor que 4, insuficiente para CJK —el respaldo real es el degradado por 400/413, nunca la estimación); 400/413 degrada con troceado/bisección.
5. **Deprecación publicada**: el release del paso 5 salió al menos una minor antes del borrado.

## Compatibilidad (ruptura anunciada)

Superficie, toda verificada bajo gate `ai` hoy (`options_spec/ai.rs:77,102,127`):

- **La feature `ai` desaparece sin shim.** Las features de Cargo resuelven en compilación: `cargo build --features ai` fallará en resolución, así que aquí sólo hay release notes.
- **Flags que sobreviven, des-gateados.** `--clean-ai` (cambia de semántica: ONNX → endpoint remoto; es el flag del cleaner del paso 2, **no** un shim) y `--offline`. Para que vivan hay que quitarles el gate `ai` y pasarlos al build por defecto: un shim detrás de la feature que se borra no existe, en ningún lenguaje.
- **Qué knob resuelve `--clean-ai` después del borrado: `--embedding-provider`** (env `WEBFANG_EMBEDDING_PROVIDER`, flag > env). Decidido 2026-10-16. Es el mismo knob que ya llena el slot de vault y que **hoy resuelve sin la feature `ai`** (`main.rs:980-987`), así que el camino ya existe y no hay que inventar uno. No es `--tier2-embedding-provider`: el §Egress de Tier 2 prohíbe que un slot de embeddings herede la resolución de otro, y el del inspector es explícito por diseño y exige su propio `--tier2-consent`. No es un flag nuevo: sería un shim más que deprecar dentro de la misma ventana en la que ya se retiran cinco. **Gap hoy:** `build_ai_cleaner` (`main.rs:873-978`) vive bajo `#[cfg(feature = "ai")]` y resuelve solo el puerto ONNX local, así que este knob es la especificación del paso 6, no una capacidad existente.
- **Shims** (error útil de migración, ≥2 releases minor, también des-gateados): `--ai-model`, `--max-tokens` —que además se renombra a `--max-chars`, condición 4—, y las env `WEBFANG_MAX_TOKENS`, `AI_MODEL_ID`/`WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE`.
- **Precedencia que los shims reemplazan**, para que el mensaje de migración sea exacto: `WEBFANG_AI_MODEL_ID` > `AI_MODEL_ID` legacy (`webfang_ai/.../compat.rs:34-38`); el flag de CLI gana al env (`webfang_cli/src/main.rs:602`).

`--offline` conserva su semántica (loopback permitido, remoto público → exit 78). Un release **minor** previo anuncia todo esto (aviso en ejecución + release notes —los work PRs no tocan `CHANGELOG.md`, lo escribe release-plz—); el borrado llega dos minors más adelante, y es **ese** commit el que lleva el marker de ruptura. Nunca `ConfigError` genérico ni silencio.

### Enmienda 2026-10-16 — el marker de ruptura pertenece al paso 6 (#1998)

La redacción anterior de esta sección pedía el trailer `BREAKING CHANGE:` en el commit del release previo, "para que both el version bump y la sección ⚠️ se generen". En este repo el marker de ruptura —título `!` o footer `BREAKING CHANGE:`— **dispara el bump**, así que leído al pie de la letra el paso 5 salía como MAJOR (3.0.0) y no como minor. Eso contradecía la condición 5 del owner ("el release del paso 5 salió al menos una minor antes del borrado") y la *Reversibilidad*, que asume shims con ventana de ≥2 releases minor.

**Decisión:** el paso 5 es un minor **sin** marker; el marker viaja en el commit del paso 6, que es donde el cambio es real y donde corresponde la sección ⚠️. El costo es que la sección ⚠️ del CHANGELOG aparece un release después de la primera mención, no en él.

**Salvedad, y no es cosmética:** la regla del bump viene de la política documentada en `AGENTS.md`, no de una medición en este repo — desde `v2.0.0` no existe ningún commit de ruptura en el historial (`git log v2.0.0..HEAD --grep='!:' -E` no devuelve nada), así que la regla nunca se ejecutó acá. Verificarla es un paso del release de deprecación: si al cortar el minor resultara un MAJOR, esta enmienda cae y el aviso se reescribe como announcement de 3.0.0.

## Umbral por defecto huérfano

El 0.3 está calibrado para Granite y vive en cuatro sitios (`options_spec/ai.rs:37`, `threshold_config.rs:49`, `semantic_cleaner_impl.rs:124`, `relevance_scorer.rs:333`); retirado el único modelo calibrado, cualquier remoto retiene distinto con el mismo número. Mitigación obligatoria con **criterio de salida**: mientras el umbral sea el default y el modelo no esté calibrado, aviso una vez por corrida + `threshold_used` en el summary; la mitigación termina en el flip anunciado con tope de 2 releases minor —si no hay golden set para entonces, el owner decide (aviso permanente documentado o mantener 0.3 con riesgo firmado) y lo registra aquí. Sin salida con fecha, el huérfano se vuelve permanente.

**Corrección 2026-10-21 (#2167): el 0.3 ya no vive en cuatro sitios.** El párrafo anterior los enumera (`options_spec/ai.rs:37`, `threshold_config.rs:49`, `semantic_cleaner_impl.rs:124`, `relevance_scorer.rs:333`): tres de esos ficheros都属于 al crate `webfang_ai`, que este mismo ADR borró en el paso 6, así que la lista describe un árbol que ya no existe — y la línea que sobrevive (`options_spec/ai.rs`) ya no es la del default. Los **cuatro sitios reales** del default son `domain/options_spec/ai.rs:47` (`DEFAULT_THRESHOLD`, la fuente única del valor), `domain/options_spec/ai.rs:76` (el default del spec, que es una cadena), `config.rs:59` y `application/crawl_options.rs:37` (los defaults de `Config` y `AiConfig`).

El razonamiento de esta sección no cambia por eso, y es lo que la sostiene: lo que importa es que el default **no está calibrado** para ningún provider vivo, y eso sigue siendo cierto. Lo que cambia es dónde vive el número. La decisión original **no** se reescribe: el párrafo de arriba queda como el registro de lo que se escribió en su momento.

**Corrección 2026-10-11 (#1975): el 0.75 de Tier 2 son dos números, no uno.** El texto anterior decía que "no es de similitud sino `ErrorHintConfig::semantic_threshold`" — incompleto. (1) `ErrorHintConfig::semantic_threshold = 0.75` (`extraction_quality.rs:29`, junto a `lexical_escalation_lower_bound: 0.70`) no gatea nada: su único consumidor es `semantic_drift_factor` (`structural_score.rs:56-64`), que lo usa como denominador de normalización `1 − (0.75 − t)/0.75` y devuelve 1.0 neutral cuando Tier 2 no corrió. (2) `AdaptiveSelectorOptions::semantic_threshold = 0.75` (`adaptive_engine/mod.rs:99`) sí es la puerta de similitud: decide Repaired/Degraded (`:532`) y se pasa como threshold del inspector (`webfang_cli/src/main.rs:588`), que además filtra por él. El "umbral 0.75 a recalibrar por modelo" del paso 3 refiere a (2); recalibrar (1) es desacoplar un denominador del hint — cambio distinto.

## Condición de revisión

La decisión entre borrado total (6a) y refugio (6b) se arbitra por **dos señales verificables**, sin depender de procesos de discovery externos ni de telemetría.

**1. Criterio técnico (gate del paso 6).** Se evalúa al llegar al paso 5:

- Los *Criterios de aceptación de M2* (abajo) pasan en verde contra **Ollama y vLLM reales**, no solo wiremock, en al menos un entorno documentado.
- La suite wiremock del PR-8 (401 —con y sin credencial—, 429 con `Retry-After` en segundos y en fecha, 5xx, timeout, respuesta malformada, orden de batch, error de lectura de cuerpo, DNS) está verde.
- Sobre el 429, la regla corregida: `Retry-After` se honra en **ambas** formas que permite RFC 9110 §10.2.3 — `delay-seconds` y fecha HTTP. Antes la fecha HTTP se descartaba en silencio y caía a backoff exponencial, con lo que el cliente reintentaba antes de lo que el servidor pedía; hoy la conversión es una función pura con `now` por parámetro, y una cabecera presente pero inservible emite un `warn!` estructurado en lugar de degradarse sin dejar rastro.
- La costura de B2 entre la consulta de nombres y el veredicto de política está cubierta **con un resolver determinista**: `ValidatingResolver` recibe el lookup por un seam (`NameSource`, producción = `GaiNameSource`, el mismo `getaddrinfo` que antes se llamaba inline), de modo que `Resolve::resolve` se ejercita de punta a punta con un answer set fijo —admite un host declarado sobre red allowlisted, rechaza el no declarado con la clase `allow_hosts`, envenena el conjunto mixto con la clase SSRF, salta el scan con la validación desarmada y propaga un fallo de lookup sin cambiarlo—. Antes de esto ambas mitades se probaban por separado (`fail_closed_scan` con answer sets sintéticos, y `resolve` real solo con literales o con `localhost` denegado), así que un fallo en cómo `gai_lookup` las unía no habría sido observable. Una resolución de LAN real no es reproducible en CI (depende del resolver y el routing de la máquina), por eso el seam en vez de un E2E de red.

Si ambos se cumplen, el paso 6 puede proceder, sujeto a la señal 2. Si alguno falla, se posterga un release minor y el gap se registra en este ADR.

**2. Señal de objeción (entre el paso 5 y el 6).** Es pasiva: no recolecta datos ni abre egress alguno.

- El release de deprecación publica el aviso (en ejecución y en el CHANGELOG, ver Compatibilidad) apuntando a un issue o discusión fija. **Fijado 2026-10-16: #1997** (`chore: canal de objeción — migración desde ONNX local`). Es una issue y no una discusión porque el repo tiene Discussions deshabilitadas (`has_discussions: false`), y **no** puede ser la issue que cierre el PR del aviso: `pr-validation.yml` exige que todo PR cierre una issue, y GitHub cerraría también el canal de objeción al mergear. El PR del aviso referencia #1997 con `Refs`, y cierra su propia issue de trabajo (#1999).
- Tras **2 releases minor**: si ningún usuario reclama dependencia de ONNX, el paso 6 procede como borrado total; si hay reclamos, el crate pasa a refugio (c) "sin soporte" y los casos se registran aquí.
- **Limitación aceptada:** la ausencia de reclamos **no prueba** ausencia de uso —hay usuarios que no leen avisos—. Por eso el borrado es reversible desde la etiqueta git del paso 5 (ver *Reversibilidad*), y esa asimetría es un riesgo que este ADR declara, no oculta.

**Entrevistas:** son **insumo no bloqueante**. Si existen al llegar al paso 5, informan la decisión; si no, el owner decide con las dos señales anteriores y lo registra aquí. Esta sección **supera** la línea del documento `10_DECISION_OWNER_Q1.md` que las declaraba arbitantes entre (a) y (c): ese archivo está gitignored y es registro histórico de una decisión en su momento, así que no se edita — la corrección queda en el documento normativo, que es donde la va a buscar quien lea después.

**Medición futura:** si alguna vez se quiere medir uso real, requiere su propio ADR —unidad de medida por usuario o por corrida única, destino declarado y consentimiento, conforme a M4—. Este ADR no instrumenta nada.

**Excepción de mantenimiento:** si las señales muestran que M2-endpoint no es camino viable **y** hay demanda real de ONNX, el owner puede asignar dueño al crate externo y lo registra aquí. Es excepción explícita al "sin mantenimiento".

## Perfiles de modelo

models.dev no basta (sin dim/batch/prefijos; `limit.output` ≠ dimensión —mistral 3072 vs 1024 real—; sin Granite-embedding; sin runtimes locales). Perfiles propios versionados: `id → {schema_version, dim, batch_max, query_prefix, passage_prefix, context, chars_per_token, matryoshka: bool}`, viviendo en el repo junto a `ProvidersConfig` (defaults) con overrides en el fichero de config del usuario; el mismo PR que toque el schema migra los perfiles existentes. models.dev solo refresco de precio (licencia MIT, con aviso). Reglas defensivas: `dimensions` **omitido por defecto** (vLLM no-Matryoshka lo rechaza con 400), `usage` siempre opcional. `NetworkPolicy`/`allow_cidrs` no existen como tipo (0 hits): B2 los crea.

## Egress de Tier 2 (ruta nueva, explícita)

El inspector pasa de local a enviar fragmentos del DOM a un tercero: entra en el consentimiento por hash provider+endpoint y en el contador de gasto. El slot de Tier 2 es explícito por diseño: sin `--tier2-embedding-provider` sirve el pool local, y **nunca** adopta por defecto el provider de embeddings del vault — la resolución por defecto del slot de vault no se hereda aquí (#1981, #1977). Observabilidad del salto (regla de hierro: sin traza no está terminado): los spans ya existen —`remote_embedding.rs:456-460` en `embed` (`provider_id`/`model`/`dim`) y `:482-487` en `embed_batch` (los tres más `count`=textos)— y hay que extenderlos con `CorrelationId` propagado + summary por corrida con tokens/retries/degradados.

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

Si el borrado falla, el árbol ONNX se recupera desde `onnx-tree/pre-step5` — etiqueta **no-`v*`**, creada el 2026-10-16 antes de publicar el release de deprecación, apuntando al commit `38ab6a31` (un commit después de `v2.11.0`), que es el último estado de `main` con la implementación ONNX completa. El nombre no-`v*` es deliberado: `release.yml:34-35` dispara con `tags: v*` y `RELEASE_TAG_NAME_RE` (`scripts/release-tag-trust.sh:50`) es `^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$`, así que esa etiqueta es inerte para el pipeline de release. El paso 5 no toca código ONNX, así que apuntar al commit previo al release no pierde nada. Qualifier: no es drop-in —si los pasos 1–4 tocaron `webfang_ai`, el tag devuelve código viejo que puede no calzar con el árbol; la etiqueta es punto de referencia para recuperación manual, no restore automático.

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
- `crates/webfang_core/src/domain/extraction_quality.rs:29` — 0.75 del hint: denominador del drift factor, no gatea
- `crates/webfang_core/src/application/adaptive_engine/mod.rs:99,532` + `crates/webfang_cli/src/main.rs:588` — 0.75 gate real de similitud Tier 2 (Repaired/Degraded + threshold del inspector)
- `crates/webfang_core/src/application/structural_score.rs:56-64` — único consumidor del 0.75 del hint (normalización)
- `crates/webfang_core/src/application/vault_search.rs:130-148` — fail-closed por dimensión
- `docs/adr/0003-remote-inference-adapter.md:35` — env vars declaradas, nunca implementadas
- `crates/webfang_core/src/cli/preflight.rs:1404,1410` — el gate de la feature que desaparece
- `crates/webfang_core/src/domain/options_spec/ai.rs:77,102,127` — los tres flags bajo gate `ai` (y sus env: `WEBFANG_MAX_TOKENS`, `WEBFANG_OFFLINE`)
- `crates/webfang_ai/src/infrastructure_ai/relevance_scorer.rs:333` — cuarto sitio del 0.3
- `crates/webfang_core/src/infrastructure/llm/remote_embedding.rs:456-460,482-487` — spans del salto remoto
- `crates/webfang_cli/src/main.rs:602` — precedencia `--ai-model` sobre env
- Issues: #1915 (decisión), #1917 (primera revisión, cerrada por #1918), #1919 (re-scope)

**Corrección 2026-10-21 (#2167, #2168): algunas referencias ya no resuelven.** Esta lista es el inventario de la evidencia sobre la que se decidió, y queda como estaba; lo que se corrige es su estado actual. Los punteros al crate `webfang_ai` (`semantic_cleaner_impl.rs`, `relevance_scorer.rs`, `Cargo.toml`) apuntan a un árbol que este ADR borró en el paso 6: ya no resuelven. Dos de las referencias vivas se movieron por los slices que cerraron este ADR —las líneas de `options_spec/ai.rs` y de `cli/llm_wire.rs`— y sus citas por línea quedaron desfasadas; las ubicaciones actuales de los sitios del 0.3 están en la corrección del §Umbral por defecto huérfano, y la semántica vigente de `--offline` está en el §Compatibilidad y en su corrección fechada (#2166).

## Addendum 2026-10-19 — cierre del paso 6

### Desenlace: 6a, borrado total

La *Condición de revisión* arbitra 6a (borrado total) frente a 6b (refugio externo) por la señal 2. #1997 quedó abierta desde 2026-10-16 y al ejecutar el borrado tenía **0 comentarios**: no hay reclamo ni dependencia declarada de ONNX. Se ejecutó **6a** en #2072 (`e9b7217a`).

La limitación que este ADR ya declaraba sigue vigente y no se reescribe: la ausencia de reclamos **no prueba** ausencia de uso. La mitigación es la misma que el ADR fijó, la etiqueta `onnx-tree/pre-step5`, y no una garantía.

### Umbral por defecto huérfano: aviso permanente documentado

El flip anunciado ocurrió y no existe golden set. De las dos salidas que esta sección deja al owner, se toma la primera: **aviso permanente documentado**.

- El aviso por corrida y `threshold_used` en el summary siguen emitiéndose; el valor no se mueve (0.3).
- El riesgo —un endpoint remoto retiene distinto con el mismo número— queda declarado en la guía de migración, no sólo en este ADR.
- **Salida con fecha:** al cortar **3.2.0**, el mismo release que retira los shims, el owner revisa la decisión. Si para entonces hay golden set, se recalibra; si no, renueva el aviso por otra ventana o firma el riesgo de mantener 0.3. Un solo punto de revisión para las dos ventanas abiertas, en vez de dos.

**Corrección 2026-10-21 (#2167, #2168): el ancla 3.2.0 quedó inalcanzable y la revisión pasa a ser por condición.** `.github/support.json` declara 4.0 STABLE · 3.0 MAINTENANCE · 2.13 EOL, así que ninguna release futura puede cortar un 3.2.0: la fecha de revisión que se acaba de escribir no puede llegar, y con ella se cae la "salida con fecha" que esta sección exigía — el huérfano se volvería permanente por el mecanismo mismo que el ADR prohíbe.

La decisión de fondo **no cambia**: sigue siendo **aviso permanente documentado**. El aviso por corrida y `threshold_used` en el summary se emiten, el valor sigue en 0.3, y su riesgo —un endpoint remoto retiene distinto con el mismo número— queda declarado en la guía de migración, no sólo en este ADR. Lo que se corrige es el **disparador** de la revisión: pasa a ser **por condición**, con la condición que el propio ADR ya fijaba —**cuando exista un golden set** para el provider remoto en uso, se recalibra el umbral; mientras no exista, el aviso permanente y el 0.3 firmado se mantienen—, y el owner puede re-abrirla antes si aparece un motivo. La decisión original **no** se reescribe: el párrafo de arriba queda como el registro de lo que se anunció en su momento, y esta corrección es la que empieza a decir la verdad.

Sin esa revisión el huérfano se vuelve permanente, que es exactamente lo que este ADR prohíbe.

### Follow-up: la ventana de shims no cierra con este ADR

Este ADR cierra el **borrado**. Los shims (`--ai-model`, `--max-tokens`, `WEBFANG_MAX_TOKENS`, `AI_MODEL_ID` / `WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE`) siguen vivos y su retiro es un slice propio, con su marker de ruptura y su issue: **#2119** (2026-10-20).

El ancla es **3.2.0**. La ventana del §Compatibilidad pedía "≥2 releases minor" desde el anuncio de v2.12.0: pasó 2.13.0 (minor) y 3.0.0 (major, el borrado). Contarlas como las dos releases de la ventana es la lectura que mantiene la promesa hecha al usuario sin estirarla a un tercer release.

**Corrección 2026-10-21 (#2119, #2168): los shims se retiran y su ancla quedó inalcanzable.** El párrafo anterior dice que los shims "siguen vivos": su retiro es el slice **#2119** (`refactor(cli)!:`) — `--ai-model` y `--max-tokens` dejan de parsear (error estándar de clap, exit 64), sus envs se ignoran en silencio, y el aviso `onnx_deprecation` que los anunciaba se va con ellos. El ancla era **3.2.0** y **nunca llegó**: 4.0.0 se publicó antes de que ese minor existiera, de modo que la fecha de retiro era inalcanzable y la ventana se cierra **de forma anticipada**.

Lo que se cumple es la promesa del §Compatibilidad, que era "≥2 releases minor" desde el anuncio de v2.12.0: pasaron 2.13.0 (minor) y 3.0.0 (major, el borrado). El cierre anticipado no rompe esa promesa — la cuenta de releases que la sostenía ya estaba cerrada, y lo que se adelanta es la fecha de un ancla que el proceso de releases nunca iba a alcanzar. Por eso el ancla no se re-ancla a otra fecha: una fecha futura repetiría el mismo fallo cuando esa release se adelante, y lo que hace falta es un disparador que no dependa del calendario de releases.

La decisión original **no** se reescribe: el párrafo de arriba queda como el registro de lo que se anunció en su momento, y esta corrección es la que empieza a decir la verdad.

### Residuo del borrado (2026-10-20)

El paso 6 cerró cuando el motor dejó de existir, no cuando el árbol quedó prolijo. La auditoría local posterior al release encontró cuatro unidades de residuo; una ya se cerró y tres siguen abiertas:

- **#2104 (cerrada, #2108)** — `SemanticError` conservaba seis variantes sin productor (`ModelLoad`, `Download`, `CacheValidation`, `OfflineMode`, `InvalidThreshold`, `InvalidCharsPerToken`) e `Inference` se renderizaba como "Error ejecutando inferencia ONNX". Retiradas.
- **#2105 (abierta)** — `LocalEmbeddingPort` y `Tier2EmbeddingPort::Local` no se construyen en producción.
- **#2106 (abierta)** — `show` y `doctor` todavía imprimen `granite-97m`, un modelo local retirado.
- **#2107 (abierta)** — comentarios que afirman que la feature Cargo `ai` existe.

**Corrección 2026-10-21 (#2168): el estado de esa lista cambió.** #2105 y #2106 se cerraron (sus slices aterrizaron: `Tier2EmbeddingPort` colapsado a un newtype, y `show`/`doctor` reportando el provider de embeddings efectivamente resuelto). #2107 se dividió: la mitad de comentarios se corrigió en #2122, y la parte que retiraba el flag `--enable-ai` de los dos binarios MCP es el slice #2123. La lista de arriba queda como el registro del 2026-10-20, que es lo que era; esta corrección es la que la actualiza.

Ninguna es criterio de aceptación de este ADR, y por eso no bloquean su cierre. Se registran acá para que "ADR-0004 cerrado" no se lea como "no queda rastro de ONNX en el árbol".

### Nota de proceso: la revisión nativa no cubrió el paso 6

La revisión nativa quedó *exhausted* sobre este rango: `lens_context_budget_exceeded` con 20.706 y 15.927 líneas contra un diff de 14.534 — borrar un crate de ese tamaño desborda el presupuesto del lens. El veredicto vino de un verificador independiente (registro en L8 del feature doc de la unidad). Se anota para que una unidad de este tamaño no asuma cobertura del reviewer nativo: la ausencia de hallazgos ahí no es evidencia de ausencia de defectos.

### Corrección de la Enmienda 2026-10-16

La salvedad de esa enmienda pedía verificar la regla del bump "al cortar el minor", porque desde `v2.0.0` ningún commit de ruptura la había ejecutado. Se verificó, y el resultado no es el que la enmienda preveía:

- El commit del paso 6 usó el título `feat!(ai):`, que **no** es Conventional Commits: el `!` va después del scope, `feat(ai)!:`.
- Con esa forma release-plz no reconoce ni el tipo ni la ruptura y calcula un patch (2.13.1). Medido con `release-plz update` sobre el mismo árbol: `feat(ai):` → 2.14.0; `fix(cli)!:` o un footer `BREAKING CHANGE:` → 3.0.0.
- **La enmienda no cae.** La regla del repo (un marker de ruptura produce major) se cumple; lo que estaba mal era la forma del marker, no la regla.

Corolario operativo: este repo squash-mergea, así que un footer `BREAKING CHANGE:` escrito en un commit de rama queda enterrado en el cuerpo del commit squash. El marker que sobrevive es el del **título del PR**.

### Entrega

- **Implementación:** PR #2072 → `e9b7217a` (squash), 2026-10-19.
- **Release:** **v3.0.0**, tag sobre `63d912ae` (2026-10-20). Es 3.0.0 y no 2.14.0 porque la retirada es una ruptura; el número llegó ahí por un commit de declaración explícito (#2077, `chore(release)!:` mergeado con `--merge`), no por el commit del borrado: el squash descarta el `!` del título del PR y release-plz ignora el footer `BREAKING CHANGE:` para el cálculo de versión, aunque git-cliff sí lo lea para agrupar el CHANGELOG. La clase de fallo quedó registrada en #2081 y cerrada en #2085.
- **Ventana de soporte:** rotada en #2082 — 3.0 STABLE, 2.13 MAINTENANCE, 2.12 EOL.

### Desviación: el major salió sin `release/3.0` y sin RC

Este addendum cuenta la entrega; también tiene que contar lo que **no** se hizo, porque el registro limpio de arriba sugiere un camino que no fue el que se recorrió.

`AGENTS.md` pide dos cosas al cortar un MAJOR, y son de naturaleza distinta:

- **`release/X.Y` es regla dura, sin condición:** "MAJOR → always cuts `release/X.Y`". **No se cortó.** Es una desviación.
- **El RC es discrecional, pero de este caso:** "`vX.Y.0-rc.N` ... only when the maintainer declares a stabilization window (state migrations, export format, major AI (embedding-provider) changes)". La retirada del proveedor de embeddings es exactamente uno de los casos listados, y nadie declaró ventana. No es una regla incumplida: es una decisión que no se tomó.

Por qué ocurrió, en orden:

1. `release-plz` estaba roto desde #2072 —`release-plz.toml` seguía nombrando el crate borrado—, así que no había ningún tag posible hasta #2077.
2. El número 3.0.0 no salió del commit del borrado sino de un commit de declaración mergeado con `--merge` (#2077): el squash descarta el `!` y release-plz ignora el footer `BREAKING CHANGE:` para calcular versión (#2081).
3. La rotación de `support.json` debe ocurrir inmediatamente después del tag: cualquier otra PR entre el tag y la rotación falla `Validate PR metadata`. Cortar `release/3.0` como paso previo exigía meter una PR exactamente en la ventana que no la admite.

**Consecuencia real:** el major no tuvo *soak*. La primera build publicada de 3.0.0 fue la definitiva. Lo que sí se verificó antes de anunciarla: `Release-plz release` en verde y los 14 assets del GitHub Release presentes — evidencia de que la release se publicó completa, no de que el major fuera estable.

**Qué es reversible y qué no.** `release/3.0` todavía se puede cortar desde el tag si en algún momento hace falta una línea de mantenimiento para 3.0. Un `v3.0.0-rc.N` **no** se puede retrofitear: el soak sólo sirve antes de publicar.

**Causa raíz del punto 2, cerrada:** #2085 hace que `merge-when-green.sh` se niegue a squash-ear cuando el título o un commit declara ruptura. La próxima declaración de major no depende de que quien mergea se acuerde de pasar `--merge`.

## Glosario de referencias (IDs opacos)

Los IDs de trabajo (auditoría fuera del repo, gitignored) significan: Q1(a)=borrar, Q1(c)=refugio externo; B2=allowlist CIDR; C1–C3=docs y `--max-chars`; FIN-024=Tier 2 en silencio; PR-4/5/6=trazabilidad y contadores del MVP; M1–M4=modelos de producto; D6a=umbral relativo opt-in; Q4/Q6/Q7=preguntas al owner (golden set, alcance M2, default). Todo lo load-bearing está inline arriba; el glosario es solo índice.

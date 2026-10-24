# L14 — Comentarios y triage para el lado issues (redactados por el worker, revisados por el padre)

Este archivo es la **entrada del lado issues** del lote L14: el worker redactó los textos y el
padre los revisó. El **owner del plan** (sesión `01a1607b`) publica los comentarios y las
issues, según el reparto: el owner del plan hace las specs, el libro mayor y el lado issues; el
padre hace toda la implementación.

Publicar (REST, la GraphQL está floja):
`curl -sS -X POST https://api.github.com/repos/XaviCode1000/webfang/issues/<N>/comments -H "Authorization: Bearer $GH_TOKEN" -d '{"body":"..."}'`

Ninguna de estas cuatro issues se cierra: dos quedan comentadas con su estado real (#1826,
#1710) y #1609 se cierra con veredicto por fila más ≤2 slices. #2061 se responde con el
rechazo del auto-PR de rotación.

---

## 1) #2061 — rechazo del auto-PR de rotación (responder, no cerrar)

> **Rechazamos la parte de rotación de esta issue (y la de detección, por el mismo motivo).**
> El gate de scope `published` **ya corre dentro del job requerido "Validate PR metadata"**:
> `.github/workflows/pr-validation.yml` ejecuta `scripts/check_support_drift.sh --scope=published`
> como paso *enforcing* de ese job, que corre en cada PR y en cada `merge_group`, y es
> fail-closed (el script rechaza un clon shallow en vez de aprobar lo que no puede ver). El
> drift que esta issue quiere "detectar" ya se detecta **en el punto donde todavía puede
> bloquear un merge**, y sin estado: el gate responde la misma pregunta en cada PR.
>
> Un bot que abre issues de drift (o los actualiza) sería un **escritor nuevo con su propia
> máquina de estados**: qué issue es "el issue de drift actual", cómo se actualiza sin duplicar,
> cuándo se cierra, qué pasa si el maintainer rechaza la rotación. Eso es una segunda fuente de
> verdad que puede discrepar del gate que ya falla cerrado — y sin ganancia: la señal que
> importa ya es un check requerido.
>
> Y la rotación tampoco es automatizable sin una persona: `Validate PR metadata` exige que
> **toda issue vinculada** tenga el label protegido `status:approved`, que solo el mantenedor
> puede aplicar. Un PR de rotación abierto por un bot quedaría sin mergear hasta que un humano
> apruebe una issue — o sea, el paso "proponer, no mergear" que esta issue quiere ya es un paso
> humano en el pipeline actual. El bot agregaría estado, no un gate.
>
> Además, la rotación en sí misma estaba rota hasta el fix de collation de #2054: automatizar
> un comando que hasta hace poco abortaba es automatizar el bug.
>
> Lo que queda en pie es el gate: `check_support_drift.sh --scope=published` dentro de
> `Validate PR metadata` (PR) y `support-drift.yml` sobre `push: main` (drift post-merge). Si en
> el futuro queremos *notificar* el drift post-merge, que sea una notificación **sin escritura
> de estado** (un comentario en un run, no un issue que el bot debe mantener) — la decisión de
> governance ("nada que un agente corra crea, extiende ni EOLs una línea") sigue siendo del
> mantenedor, con issue aprobada detrás.

## 2) #1826 — estado real de las cuatro filas (comentar, NO cerrar)

> **Estado real de las cuatro filas (verificado contra `main`):**
>
> 1. y 2. **El crate `webfang_ai` ya no existe** (ADR-0004 paso 6: el runtime ONNX/Granite
>    local se retiró y la limpieza semántica es un `EmbeddingPort` remoto).
>    `crates/webfang_ai/src/infrastructure_ai/inference_engine.rs` no está en el árbol y el
>    workspace tiene 5 miembros. No queda "path batched muerto" que cablear ni clones de tensores
>    que quitar: esas filas quedaron **obsoletas**, no abiertas.
> 3. **Parcialmente hecha.** El límite en bytes existe: `ElasticConfig.ram_budget_bytes` está
>    "expresado directamente en bytes (un permiso == un byte)" y el resource downloader pide
>    `acquire_many(chunk_len)` contra `Semaphore::new(budget)`
>    (`infrastructure/crawler/resource_downloader.rs`); `--ram-budget` es el knob del operador.
>    Lo que sigue siendo **contado, no pesado en bytes**, es el fan-out exterior de
>    `elastic_ingestion.rs::ingest_batch` (`.buffer_unordered(self.config.cpu_cores)`), que acota
>    **cuántas URLs** están en vuelo, no sus bytes.
> 4. **Sigue abierta.** El crate de benchmarks no tiene ningún barrido 1/2/4/8 de concurrencia
>    de páginas con CPU y RSS; eso exige una corrida viva contra sitios reales
>    (`BENCHMARKING_PLAN.md` de la auditoría), no un cambio de código.
>
> Nota lateral de este mismo camino: la base SQLite/WAL que escribe la ingesta elástica ya tiene
> **runbook de recuperación** (`docs/src/state-recovery-runbook.md`, en el libro), con el gate de
> versión de esquema (`PRAGMA user_version`) documentado.
>
> **No cerramos la issue** (queda la mitad que espera un barrido real). Cuando la fila 3 se
> mida, la 4 se ejecute y las filas 1-2 se den por obsoletas con la evidencia de arriba, se puede
> cerrar por partes.

## 3) #1710 — estado real de los AC (comentar, NO cerrar)

> **Estado real de los AC:**
>
> - **AC del piso: hecho, con el número movido.** El piso vigente es **0.2.5**
>   (`MINIMUM_OBSCURA_VERSION = (0, 2, 5)`, `MINIMUM_OBSCURA_VERSION_STR = "0.2.5"` en
>   `crates/webfang_core/src/cli/preflight.rs`): el `0.2.3` que pedía esta issue quedó viejo — la
>   release estable upstream ya es 0.2.5. Tests: `assess_obscura_version_classifies_meets_too_old_unknown`,
>   `hybrid_version_025_passes`, `hybrid_version_below_minimum_errors` (exit 78 nombrando versión
>   encontrada y requerida).
> - **AC del veredicto `Unknown`: en curso.** El veredicto ya **degrada a warning y no
>   bloquea** (`hybrid_version_garbage_degrades_to_warning`,
>   `hybrid_version_probe_failure_degrades_to_warning`). Falta la parte de que el warning
>   **nombre el mínimo esperado** — eso entra con el lote L11 del plan ODD
>   (`preflight.rs::unknown_verdict_names_the_expected_minimum`).
> - **AC de los contadores por capa: superseded por decisión.** No hay contadores nuevos ni modo
>   estricto: el **hit-rate de L2 se deriva de los logs por capa** que el router hybrid ya emite.
>   La receta está en `docs/src/cli-reference.md` (sección "Layer 2 (Obscura)"), con los `jq`
>   sobre el trace file: "Layer 2 (Obscura): attempting fetch" (intentos), "Layer 2 returned N
>   bytes" (hits), "Layer 3 (Chromiumoxide): attempting fetch" (escaladas). El archivo de trace
>   corre en TRACE para el target `webfang` con independencia de `-v`, así que los eventos están
>   ahí sin `-vv`.
> - **AC de la variante de build: decidido y documentado.** **L2 se acepta como "solo render
>   barato"**: webfang **no** exige `-stealth` y L2 no carga ninguna promesa anti-WAF. El
>   fundamento: exigir `-stealth` agrega un verbo nuevo al contrato del binario y una clase
>   nueva de falla, a cambio de sostener una afirmación de marketing; la alternativa no agrega
>   ninguna promesa, la documentación dice lo que L2 da. Corregido en `docs/src/cli-reference.md`
>   y en el comentario del módulo de `obscura_downloader.rs` (que además tenía el piso viejo,
>   0.2.0).
> - **AC de docs: hecho** con lo anterior.
>
> **No cerramos la issue**: queda la mitad del veredicto `Unknown` (L11) y el hilo necesita
> seguir abierto para que ese lote no quede sin contraparte. Cuando L11 aterrice, esta issue se
> puede cerrar por partes con la evidencia de los tests.

## 4) #1609 — veredicto por fila, cierre y ≤2 slices

**Veredicto por fila** (evidencia citada; `FR-08b` y `F14` incluidos):

| Fila | Veredicto | Evidencia |
|---|---|---|
| FR-01 `cargo install` en las release notes | Resuelta | `release.yml:727` dice "binaries only … `cargo install webfang` does not work"; slice #1622 cerrada (2026-09-27) |
| FR-02 README sin `cmake` | Resuelta | `README.md:59-67` nombra `cmake` obligatorio y la cadena wreq→btls→btls-sys |
| FR-03 piso de glibc sin documentar | Resuelta | `docs/src/installation.md` §"Platform requirements": glibc 2.39, sin musl, sección Intel macOS |
| FR-04 fallos de AI-init sin remedio | Resuelta (supersedida) | `webfang doctor` / `connect` / `show`; el mensaje de `--offline` nombra los dos Exodus reales (`cli/llm_wire.rs`) |
| FR-05 caché de modelo corrupta → 78 en vez de 69 | Obsoleta | no existe variante `CacheValidation`; el runtime ONNX local se eliminó (ADR-0004 paso 6) |
| DF-D1 sin documento de data-flow/privacidad | **ABIERTA** | no existe ninguno en `docs/` |
| FR-06 `HF_HUB_OFFLINE` ignorado | Obsoleta | `hf-hub` ya no es dependencia de ningún crate |
| FR-07 proxy incidental | Obsoleta como está escrita (su evidencia era hf-hub). Residuo distinto: el producto **no** tiene soporte de proxy para el endpoint remoto y no lo dice | sin `proxy` en `infrastructure/`, sin proxy en `docs/` |
| FR-09 sin página de instalación/desinstalación | Resuelta | `docs/src/installation.md` ("Installation & Uninstall" + §"Uninstall"); la sub-issue #1647 está **cerrada** (2026-09-29) aunque la tabla de #1609 la marque "open" |
| FR-10 `config.toml` malformado cae a defaults en silencio | Resuelta por decisión | `cli/config.rs::ConfigDefaults::load` loguea `error!` ruidoso (#1659); un `WEBFANG_CONFIG` explícito falla cerrado con 78 (`load_explicit` + sus cuatro tests) |
| FR-11 `--js-strategy full` en release | Resuelta como scope aceptado y documentado | `installation.md` §"What is *not* in the release binary" + `troubleshooting.md` |
| FR-12 sin subcomando `doctor` | Resuelta | `webfang doctor` (config, credencial, red, presupuesto) + `show` + `connect` |
| FR-13 sin guía de PATH/chmod | Resuelta | `installation.md` + `troubleshooting.md` (`chmod +x webfang`, `sudo install -m 0755`) |
| FR-14 CRT dinámico de Windows | Resuelta | `installation.md` §"Platform requirements" (VC++ 2015-2022 x64) + `troubleshooting.md` (`VCRUNTIME140.dll`) |
| FR-15 checksum solo unix | Resuelta | `installation.md` documenta `sha256sum`, `shasum` y `Get-FileHash` |
| FR-16 `--ai-model` no lista ids válidos | Obsoleta | `--ai-model` se retiró (#2119); `the_retired_shims_are_not_flags_anymore` fija que clap responde "unexpected argument" |
| FR-08b README lista `tract-onnx` | Resuelta | la tabla de crates del README ya no lista tract/onnx |
| FR-17 limpieza del temp-dir de Chrome sin verificar | Resuelta | `test_chrome_profile_dir_cleans_up_on_drop` |
| DF-E3 descarga de modelo HF por defecto, host de egreso sin documentar | Obsoleta | no hay descarga local de modelo; el camino AI es un endpoint remoto (`--embedding-provider`) |
| DF-D4 contrato de payload LLM/embedding sin documentar | **ABIERTA** | "cleaned text, never raw HTML" solo existe en el código (`application/llm_extraction.rs` + su test); ninguna página del libro lo dice |
| DF-D5 MCP sin modo offline, cold cache de ~390 MB | Obsoleta | `mcp_server/ai_wiring.rs` ya no existe; las tools AI de MCP usan provider remoto |
| M1/G1 `ExportState` muerto + `AGENTS.md` con `filter_processed_urls` | **MEDIA ABIERTA** | la podredumbre de doc está arreglada; `StateStore::load_or_default` sigue con **cero llamadores de producción** (sus 11 callers son de test) |
| F14 `AGENTS.md` omite `webfang_benchmark` | Resuelta | `AGENTS.md` lo lista en el mapa del workspace y en la tabla de dirección de dependencias |
| Carry-over: README apunta al wiki | **ABIERTA** | `README.md:447` enlaza `github.com/XaviCode1000/webfang/wiki` ("Architecture, API reference, guides") — el wiki no tiene páginas |
| Carry-over: tabla "Build Features" del README | **ABIERTA** | `README.md:328` anuncia `default` (`images` + `documents`) como "✅ Yes", pero `default` hoy es un set **vacío** |

**Las ≤2 slices vivas** (a abrir):

- **Slice A — "docs: data-flow y contrato de payload"** (DF-D1 + DF-D4 + DF-E3 como residuo
  documentado). Entregable: `docs/src/data-flow.md` en el libro (con entrada en `SUMMARY.md`): qué
  datos salen de la máquina y hacia dónde (endpoint de embeddings, endpoint LLM, consumidor
  MCP), el contrato de payload ("texto limpio, nunca HTML crudo", pineado por el test de
  `llm_extraction.rs`), hosts de egreso y el comportamiento offline de MCP. Es la remediación
  que la propia auditoría de dataflow propuso; no toca código.
- **Slice B — "docs rot: el contrato ExportState muerto y el README"** (residual de M1/G1 + los
  dos carry-over). Decidir (una de dos) el contrato `StateStore::load_or_default` muerto:
  cablearlo al camino de `--resume` **o** borrarlo (con su port y sus tests) — la decisión es de
  producto, no un follow-up automático. En el mismo slice: quitar el enlace al wiki vacío
  (`README.md:447`) y corregir la tabla "Build Features" (`README.md:328`) contra el
  `Cargo.toml` real. F14 y FR-08b salen de la slice: ya están resueltas.

**Al cerrar #1609**: corregir su tabla de sub-issues (#1647 y #1622 están cerradas) y adjuntar
el veredicto por fila de arriba.

**Nota deResidual (no es de esta issue, queda registrado)**: el doc-comment de
`crates/webfang_core/src/infrastructure/export/state_store.rs::load_or_default` sigue nombrando
`filter_processed_urls`, que ya no existe → es parte de la Slice B.

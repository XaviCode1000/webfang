# 🕵️ Reporte de Auditoría MCP v2.0.0 — hallazgos menores (issue #696)

> ⚠️ **PROVENIENCIA — RECONSTRUCCIÓN, NO ORIGINAL**
>
> El reporte original de la auditoría MCP v2.0.0 (2026-08-11) nunca fue
> persistido en la bóveda: no existe en el repo, en ningún worktree, ni en
> rutas temporales (búsqueda exhaustiva de filesystem, 2026-08-17). Este
> archivo es una **reconstrucción** escrita el 2026-08-17 a partir del
> contenido de la issue #696 y de la verificación de código realizada ese
> mismo día. No debe citarse como el artefacto original de la auditoría;
> se commitea únicamente para que el historial de Git sea coherente con la
> referencia de evidencia de la issue #696.

**Rol:** Senior Rust Architect & MCP Security Reviewer
**Fecha de reconstrucción:** 2026-08-17 · **Binario auditado (original):** MCP server v2.0.0
**Metodología:** Caja blanca (lectura de `mcp_server/`) + probe de runtime
**Issue vinculada:** #696 · **Fix:** commit `6829bd6` (`fix/696-mcp-export-roots`)

---

## 0. Veredicto Ejecutivo

| ID | Hallazgo | Severidad | Estado original | Resolución |
|:---|:---------|:---------:|:---------------:|:-----------|
| RIESGO-MCP-EXPORT-001 | `output_dir` absoluto = write-anywhere (F17) | Riesgo | Abierto (por diseño #600) | **FIJADO** — root-of-trust `allowed_export_roots` |
| OBS-MCP-BATCH-001 | métricas de `scrape_batch` bajo un solo dominio (F15) | Observación | Abierto | **FIJADO** — atribución por dominio real |

---

## 1. RIESGO-MCP-EXPORT-001 — `output_dir` absoluto = write-anywhere

### Evidencia original (issue #696)

`export_file` acepta `output_dir` absoluto (issue #600, por diseño). Probe
`output_dir=/etc filename=passwd` → error honesto
`isError:true: write error: /etc/passwd.jsonl.lock: Permission denied`
(sin panic, sin escritura; el filename está saneado por `SanitizedFilename`,
issue #601). El riesgo real: si el server corre con privilegios, el caller
puede escribir en cualquier ruta del filesystem.

### Verificación de código (2026-08-17)

Confirmado. En `mcp_server/params.rs` la validación usaba
`require_safe_path_allow_absolute("output_dir", …)` — el comentario en línea
cita explícitamente la issue #600: los paths absolutos DEBEN aceptarse. Las
protecciones existentes funcionan contra *path traversal en el filename*
(`SanitizedFilename`), pero no restringen **dónde** puede escribir el
`output_dir` absoluto. El riesgo residual es un *write-anywhere primitive*
condicionado solo a los privilegios del proceso servidor.

### Resolución (commit `6829bd6`)

Se añadió un **root-of-trust** en `McpState`:

- Campo `allowed_export_roots: Arc<Vec<PathBuf>>` — vacío por defecto =
  **fail-closed** (los `output_dir` absolutos se rechazan).
- `McpState::validate_export_dir(&Path)` — los paths relativos pasan sin
  cambio (comportamiento existente); los absolutos deben estar bajo uno de
  los roots tras normalización léxica (`normalize_lexical` resuelve `.`/`..`
  sin tocar el filesystem), de modo que ni `..` ni un hermano por prefijo de
  string (`/tmp/rootX` vs root `/tmp/root`) escapen del root.
- Cableado en `export_file`, `export_jsonl`, `export_vector` y
  `download_assets`.
- Ambos binarios (`mcp_server_http.rs`, `mcp_server_stdio.rs`) exponen
  `--export-roots` / `WEBFANG_MCP_EXPORT_ROOTS` (repetible o comma-separated).

Tests: relativo siempre permitido; absoluto rechazado sin roots; absoluto
permitido bajo root; rechazado fuera del root; hermano por prefijo rechazado;
`..` no escapa del root.

---

## 2. OBS-MCP-BATCH-001 — métricas de `scrape_batch` bajo un solo dominio

### Evidencia original (issue #696)

`scrape_batch` agrega sus métricas bajo `urls.first().host` aunque procese N
dominios: `tool=scrape_batch domain=example.com pages=2` para un batch con
`example.com` + `books.toscrape.com`.

### Verificación de código (2026-08-17)

Confirmado. En `mcp_server/handlers/scraping.rs` el dominio se derivaba de
`urls.first().and_then(|u| u.host_str())` y `record_scrape_identity` recibía
ese único dominio con el `count` total. En `mcp_server/metrics.rs`,
`DomainStats` acumulaba `pages` bajo esa sola clave: un batch multi-dominio
inflaba un dominio y dejaba el resto en cero.

### Resolución (commit `6829bd6`)

Helper `record_batch_metrics_by_domain` en `scraping.rs`: agrupa `results` y
`failed` por su host **real** (`result.url.host_str()` / parse de
`failed.url`) y emite un `ScrapeEvent` por grupo de dominio. Todos los eventos
comparten el `root_correlation` de la operación (#501/#698): una identidad de
operación, un evento por dominio. `BTreeMap` mantiene el orden de emisión
determinístico (DD-6). El outcome por dominio es `Success` (todo ok), `Error`
(todo falló) o `Partial` (mixto).

Tests: multi-dominio atribuye un evento por host; single-dominio como
regression guard; outcomes mixtos por dominio (`Partial` + `Error`).

---

## 3. Nota sobre la evidencia perdida

El archivo referenciado originalmente por la issue #696 no estaba commiteado.
Esta reconstrucción cierra esa brecha de trazabilidad. Cualquier cita futura
debe referirse a este archivo como **reconstrucción**, no como el reporte
original de la auditoría.

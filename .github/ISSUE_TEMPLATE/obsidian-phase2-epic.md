# Epic: Obsidian Phase 2 — Duplicate Detection, Local Images & Semantic Dedup

**Type:** Epic  
**Labels:** enhancement, obsidian, roadmap  
**Priority:** High — top user demands from research

---

## Context

Obsidian integration v1.1.0 ya implementa:
- ✅ Vault auto-detect (4-tier)
- ✅ Wiki-links conversion
- ✅ Relative asset paths
- ✅ Rich metadata frontmatter (`readingTime`, `language`, `wordCount`, `contentType`, `status`)
- ✅ Quick-save mode
- ✅ Obsidian URI

La investigación de usuarios ([`docs/research/obsidian-user-research.md`](docs/research/obsidian-user-research.md)) y el spec técnico ([`docs/research/obsidian-markdown-spec.md`](docs/research/obsidian-markdown-spec.md)) identificaron las features más demandadas que aún faltan.

---

## Phase 1: Duplicate Detection + Local Images (High Demand)

> **Prioridad:** Alta — demanda #1 de la comunidad Obsidian

### 1.1 Duplicate URL Detection

**Demanda:** 🔥🔥🔥🔥🔥 (top feature request across all tools)

**Fuentes:**
- [obsidian-clipper #112](https://github.com/obsidianmd/obsidian-clipper/issues/112) — "Indicate if current URL is referenced" (10+ reactions)
- [obsidian-clipper #323](https://github.com/obsidianmd/obsidian-clipper/issues/323) — "Hint for already-added pages"
- [obsidian-clipper #433](https://github.com/obsidianmd/obsidian-clipper/issues/433) — "Prompt on plugin icon that URL has been bookmarked"

**Approach técnico:**
Ya tenemos `StateStore` (`src/infrastructure/export/state_store.rs`) que trackea URLs procesadas por dominio. Para duplicate detection a nivel vault:
1. Al detectar vault con `detect_vault()`, escanear archivos `.md` existentes
2. Parsear frontmatter YAML → extraer campo `source` / `url`
3. Antes de guardar: comparar URL del scrapeo con URLs existentes
4. Si match: warning → skip/overwrite prompt

**Nuevos archivos:**
- `src/infrastructure/obsidian/dedup.rs` — vault URL index + duplicate check

**Nuevos flags CLI:**
- `--obsidian-dedup` — enable duplicate detection (default: true when `--obsidian-*` is used)

### 1.2 Download Images Locally

**Demanda:** 🔥🔥🔥🔥

**Fuente:** [obsidian-markdown-spec.md §P1](docs/research/obsidian-markdown-spec.md)

**Approach técnico:**
Ya tenemos asset download (`src/extractor/mod.rs:extract_images()`, `src/adapters/downloader/mod.rs`). Para Obsidian:
1. Download images a `{vault}/_attachments/` (configurable)
2. Rewrite markdown image refs: `![alt](https://...)` → `![](../_attachments/hash.png)` o `![[hash.png]]`
3. Usar `pathdiff` (ya es dependencia) para paths relativos

**Archivos a modificar:**
- `src/infrastructure/converter/obsidian.rs` — image rewrite logic
- `src/infrastructure/obsidian/` — image download coordinator

**Nuevos flags CLI:**
- `--obsidian-local-images` — download images to vault (default: true with Obsidian mode)
- `--obsidian-image-folder <PATH>` — custom image folder (default: `_attachments`)

### 1.3 Incremental Clipping

**Demanda:** 🔥🔥🔥🔥 (#4 user request)

**Fuente:** [obsidian-user-research.md §4](docs/research/obsidian-user-research.md)

**Approach técnico:**
Similar a `--resume` pero a nivel archivo de vault:
1. Escanear vault para URLs ya guardadas
2. Skip URLs que ya tienen nota en el vault
3. Opcional: append new content to existing note

**Nuevos flags CLI:**
- `--obsidian-incremental` — skip URLs already saved in vault

### Phase 1 Acceptance Criteria

- [ ] `--obsidian-dedup`: detecta URLs duplicadas en vault antes de guardar
- [ ] Si duplicado: warning con path del archivo existente + opción skip/overwrite
- [ ] `--obsidian-local-images`: descarga imágenes a `_attachments/` del vault
- [ ] Image refs rewritten a paths relativos o wiki-links
- [ ] `--obsidian-incremental`: skip URLs ya guardadas en vault
- [ ] Tests: dedup detection, image download, incremental save
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `cargo nextest run --test-threads 2` passing

---

## Phase 2: Semantic Duplicate Detection (Unique Differentiator)

> **Prioridad:** Media — differentiator que ningún competidor tiene

### 2.1 Semantic Duplicate Detection

**Demanda:** 🔥🔥🔥

**Fuente:** [obsidian-user-research.md §Differentiator #3](docs/research/obsidian-user-research.md)

**Approach técnico:**

Esto es un **unique differentiator** — ningún competidor (MarkDownload, Obsidian Clipper, Firecrawl) lo hace. Ya tenemos toda la infraestructura:

| Componente | Ubicación | Estado |
|------------|-----------|--------|
| `cosine_similarity()` | `src/infrastructure/ai/embedding_ops.rs:71` | ✅ Implementada |
| `RelevanceScorer` | `src/infrastructure/ai/relevance_scorer.rs` | ✅ Implementado |
| `SemanticCleanerImpl` | `src/infrastructure/ai/semantic_cleaner_impl.rs` | ✅ Implementado |
| `DocumentChunk` con embeddings | `src/domain/entities.rs` | ✅ Implementado |
| ONNX inference (tract-onnx) | `Cargo.toml` feature `ai` | ✅ Disponible |

**Flujo propuesto:**
1. Al scrapear con `--clean-ai`, generar embedding del contenido (ya soportado)
2. Antes de guardar en vault, generar embeddings de notas existentes
3. Comparar via `cosine_similarity()` con threshold configurable
4. Si `similarity > threshold` (default 0.85): "Contenido similar encontrado: `{archivo}`"
5. Opciones: skip, append, overwrite

**Nuevos archivos:**
- `src/infrastructure/obsidian/semantic_dedup.rs` — vault embedding index + similarity check

**Nuevos flags CLI:**
- `--obsidian-semantic-dedup` — enable semantic duplicate detection (requires `--features ai`)
- `--obsidian-semantic-threshold <FLOAT>` — similarity threshold (default: 0.85, range: 0.0-1.0)

### Phase 2 Acceptance Criteria

- [ ] `--obsidian-semantic-dedup`: genera embedding del contenido scrapeado
- [ ] Escanea vault y compara embeddings existentes
- [ ] Si similarity > threshold: warning con path del archivo similar
- [ ] Threshold configurable (default: 0.85)
- [ ] False positive rate < 5% en tests
- [ ] Feature-gated: requiere `--features ai`
- [ ] `cargo nextest run --features ai --test-threads 2` passing

---

## Phase 3: Content Detection + MOCs (Future)

> **Prioridad:** Baja — exploratorio, definir después de Phase 1 y 2

### 3.1 Content Type Auto-Detection

**Demanda:** 🔥🔥🔥

**Fuente:** [obsidian-user-research.md §Differentiator #8](docs/research/obsidian-user-research.md)

Ya tenemos `ContentType` enum en `src/infrastructure/obsidian/metadata.rs`. Expandir para detectar automáticamente:
- Artículos, productos, recetas, papers, documentación
- Usar schema.org meta tags, contenido, estructura HTML
- Aplicar template apropiado según tipo detectado

### 3.2 Auto-Generated MOCs (Maps of Content)

**Demanda:** — (differentiator)

**Fuente:** [obsidian-user-research.md §Differentiator #6](docs/research/obsidian-user-research.md)

Después de scrapear múltiples páginas sobre un tema, generar automáticamente una nota índice (MOC) con links a todas las notas relacionadas.

### 3.3 Git-Aware Vault

**Demanda:** — (differentiator)

**Fuente:** [obsidian-user-research.md §Differentiator #7](docs/research/obsidian-user-research.md)

Detectar si el vault usa Git y crear commits descriptivos por cada nota guardada.

### 3.4 Template System

**Demanda:** 🔥🔥

**Fuente:** [obsidian-markdown-spec.md §P2](docs/research/obsidian-markdown-spec.md)

Sistema de templates configurables para personalizar el output de cada nota (similar a MarkDownload y Obsidian Clipper v1.0).

---

## Dependencies

| Dependency | Status | Notes |
|------------|--------|-------|
| `pathdiff` 0.2 | ✅ Cargo.toml | Paths relativos |
| `whatlang` 0.18 | ✅ Cargo.toml | Detección de idioma |
| `slug` 0.1 | ✅ Cargo.toml | Filenames |
| `urlencoding` 2.1 | ✅ Cargo.toml | URI encoding |
| Asset download | ✅ `src/extractor/mod.rs` | `extract_images()` |
| StateStore | ✅ `src/infrastructure/export/state_store.rs` | URL tracking |
| `cosine_similarity` | ✅ `src/infrastructure/ai/embedding_ops.rs` | Phase 2 solo |
| AI infrastructure | ✅ `--features ai` | Phase 2 solo |

**Phase 1 no necesita nuevas dependencias.** Phase 2 reusa infraestructura AI existente.

---

## Architecture Impact

GitNexus confirma que los módulos Obsidian actuales tienen bajo acoplamiento:

| Módulo | Archivos | Líneas | Impacto Phase 1 |
|--------|----------|--------|-----------------|
| `src/infrastructure/obsidian/` | 3 | ~350 | +1 archivo (`dedup.rs`) |
| `src/infrastructure/converter/obsidian.rs` | 1 | 677 | Modificar (image rewrite) |
| `src/infrastructure/ai/` | 12 | ~3,800 | Sin cambios (Phase 2 reusa) |

Los cambios serán **aditivos** — nuevos módulos sin modificar los existentes.

---

## Research Sources

- [`docs/research/obsidian-user-research.md`](docs/research/obsidian-user-research.md) — Top 15 features demandadas
- [`docs/research/obsidian-markdown-spec.md`](docs/research/obsidian-markdown-spec.md) — Spec completa + matriz competidores
- [docs/OBSIDIAN.md](docs/OBSIDIAN.md) — Documentación de usuario actual

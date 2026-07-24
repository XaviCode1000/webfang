# Auditoría missing_docs

## Resumen

- webfang_core: 206 warnings
- webfang_ai: 213 warnings
- Total: 419 warnings

## Por módulo

| Módulo | Warnings |
|---|---:|
| cli | 74 |
| infrastructure/crawler | 44 |
| cli/args | 40 |
| application/crawler | 36 |
| adapters | 36 |
| domain | 34 |
| domain/error | 30 |
| application | 30 |
| infrastructure/downloader | 22 |
| domain/entities | 16 |
| infrastructure | 12 |
| application/batch | 12 |
| infrastructure_ai | 7 |
| application/pipeline/stages | 6 |
| adapters/downloader | 6 |
| adapters/detector | 6 |
| (raíz del crate) | 6 |
| domain/site | 2 |

## Top archivos

| Archivo | Warnings |
|---|---:|
| adapters/url_path.rs | 36 |
| domain/error/crawl_error.rs | 30 |
| cli/error.rs | 30 |
| cli/args/mod.rs | 30 |
| application/crawler/crawl_task_ctx.rs | 30 |
| cli/export_flow.rs | 26 |
| infrastructure/crawler/sitemap_parser.rs | 24 |
| domain/mod.rs | 24 |
| application/progress_types.rs | 24 |
| domain/entities/content.rs | 16 |
| infrastructure/downloader/cookie_bridge.rs | 12 |
| infrastructure/autotuning.rs | 10 |
| cli/mod.rs | 8 |
| application/batch/manager.rs | 8 |
| infrastructure/crawler/compression_handler.rs | 6 |
| config.rs | 6 |
| cli/commands.rs | 6 |
| application/pipeline/stages/output.rs | 6 |
| application/crawler/collector.rs | 6 |
| adapters/downloader/mod.rs | 6 |

## Recomendación de implementación

Esfuerzo estimado: L

Justificación basada en el total de warnings:
- Total 100-500 → M (2-3 PRs por módulo)

Plan sugerido por fases:
1. Dominio (traits, entidades, value objects, errores, invariantes)
2. Application (servicios, casos de uso, orquestación)
3. Ports/config/errors (interfaces de adaptadores, repositorios, tipos públicos de config)
4. webfang_ai (providers, prompts, schemas, errores de integración)
5. CI + deny final

## Criterios para activar `deny`

- Total < 100 warnings → activar deny en el próximo PR de documentación.
- Total 100-500 → activar deny por fases, deny solo en módulos ya documentados.
- Total > 500 → no activar deny hasta tener un plan de documentación por fases aprobado.

## Notas técnicas

- `missing_docs` default es `allow`. Se cambió a `warn` para la auditoría.
- No se activó `deny` en esta fase.
- No se modificó ningún ítem de configuración de lints en Cargo.toml.

## Corrección de clasificación

La clasificación original fue **M**, pero el total de 419 warnings supera el umbral de 200 warnings definido en el brief.
Por lo tanto, el esfuerzo correcto es **L** y corresponde implementar la documentación por fases,
no en un único PR.

## Addendum: Reconciliación crudo → deduplicado (2026-07-24)

La auditoría original reporta **419 warnings crudos**:

- `webfang_core`: 206
- `webfang_ai`: 213
- Total crudo: 419

Sin embargo, `webfang_ai` depende de `webfang_core`. Cuando se activa `#![warn(missing_docs)]`
en `webfang_ai`, los 206 warnings de `webfang_core` se propagan. Solo **7 warnings** son
realmente de `webfang_ai` (en `infrastructure_ai/`).

El script `audit-missing-docs.sh` fue corregido para deduplicar por `file:line`,
eliminando el doble-conteo. Estado efectivo:

| Métrica | Crudo | Deduplicado |
|:--------|------:|:-----------:|
| webfang_core | 206 | 192 |
| webfang_ai (propios) | 213 | 7 |
| **Total** | **419** | **199** |

### Breakdown por PR (Fase 0)

| PR | Módulos | ΣWarnings | ΣE (pub(crate)) | ΣR (docs/allow) |
|:--:|:--------|:---------:|:----------------:|:----------------:|
| 0A | (sin warnings — tooling + lint swap) | 0 | 0 | 0 |
| 0B | infrastructure/*, adapters/* | 62 | ~50 | ~12 |
| 0C | cli/*, cli/args/* | 50 | ~35 | ~15 |
| 0D | domain/*, application/*, config.rs | 80 | ~42 | ~38 |
| F5 | webfang_ai/infrastructure_ai | 7 | 0 | 7 |
| **Total** | | **199** | **~127** | **~72** |

Verificación: 62 + 50 + 80 + 7 = 199 ✓

### Impacto en esfuerzo

El esfuerzo se revisa de **L** a **M**, dado que:
- Warnings únicos reales: 199 (no 419)
- Eliminables vía `pub(crate)`: ~127 (sin escribir docs)
- Restantes para documentación: ~72

## Reproducción

Para reproducir el conteo deduplicado:

```bash
./scripts/audit-missing-docs.sh
```

El script deduplica automáticamente por `file:line` usando `sort + awk`.


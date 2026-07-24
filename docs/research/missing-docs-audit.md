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

Esfuerzo estimado: M

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

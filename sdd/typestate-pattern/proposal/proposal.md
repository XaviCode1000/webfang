# Proposal: Typestate Pattern para validación compile-time

## Intent

Reemplazar validación runtime de `DocumentChunk` (vía `From<ScrapedContent>`) con el patrón **Typestate** para garantías compile-time. Actualmente, cualquier `DocumentChunk` puede ser creado sin validación explícita, lo que causa errores runtime cuando contenido inválido llega a `FileExporter`.

## Scope

### In Scope
- Definición de tipos estados para `DocumentChunk` (`Draft` → `Validated` → `Exported`)
- Transiciones de estado via métodos que solo existen en estados válidos
- Integración en `file_exporter.rs` para requerir estado `Validated`
- Migración gradual desde `From<ScrapedContent>` impl

### Out of Scope
- Otros usos de `DocumentChunk` (Obsidian, AI semantic cleaner)
- Patrones de estado en crawler_service o TUI
- Cambio de arquitectura de tipos existente

## Capabilities

### New Capabilities
- `typestate-document-chunk`: Tipos con estado (Draft, Validated, Exported) con transicionescompile-time

### Modified Capabilities
- `file-exporter`: Atualización para requerir `DocumentChunk` en estado `Validated`

## Approach

Usar **private state pattern** con tiposmarker:

```rust
// estadosprivate states:
// - Draft: newly created, not validated// - Validated: passed validation checks// - Exported: successfully written to disk

pub struct DocumentChunk<S = Draft> {    url: Url,    title: String,
    content: String,    timestamp: DateTime<Utc>,
    _state: PhantomData<S>,
}

// Solo permite creación desde ScrapedContent (Draft)
impl From<ScrapedContent> for DocumentChunk {}

// Métodos de transiciónimpl DocumentChunk {
    pub fn validate(self) -> DocumentChunk<Validated> { ... }
    pub fn export(self) -> ExportResult<()> where S: Validated { ... }
}
```

**Ventajas**:
- Errores en compilación, no runtime
- API auto-documentada (métodos solo visibles en estados válidos)
- Sin runtime overhead

## Affected Areas

| Area | Impact | Description |
|------|--------|-------------|
| `src/infrastructure/export/file_exporter.rs` | Modified | Requiere estado `Validated` |
| `src/domain/document_chunk.rs` | New | Definición de tipos estados |
| `src/application/` consumers | Modified | Actualizar a nuevo API |

## Risks

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| breaking-api-change en FileExporter | High | Mantener backward compat temporal |
| Migración incremental compleja | Med | Phased rollout por módulo |
| Tests existentes fallan | High | Actualizar test helpers |

## Rollback Plan

1. Mantener tipo `DocumentChunk` original como alias: `type DocumentChunk = DocumentChunk<Draft>;`
2. Feature flag `typestate` para enable/desable
3. Revert simple: disable feature y usar alias

## Dependencies

- None — solo Rust stdlib + PhantomData

## Success Criteria

- [ ] Tipo `DocumentChunk` con estados privados compila
- [ ] `file_exporter.rs` rechaza `DocumentChunk<Draft>` en compilación
- [ ] Transiciones `Draft → Validated → Exported` funcionan
- [ ] Tests existentes pasan (actualizados)
- [ ] `cargo clippy` clean
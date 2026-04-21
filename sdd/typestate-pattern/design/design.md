# Design: Typestate Pattern para validación compile-time

## Technical Approach

Implementar el patrón **typestate** usando tipos marker privados con `PhantomData<S>` para garantizar en tiempo de compilación que `DocumentChunk` fue validado antes de ser exportado. El estado inicial es `Draft`, la transición a `Validated` ocurre vía método `.validate()`, y solo `DocumentChunk<Validated>` puede ser exportado.

## Architecture Decisions

### Decision: Tipos estado marker

**Choice**: Structs vacío privados (`Draft`, `Validated`, `Exported`)
**Alternatives considered**: Enums con un valor (menos idiomático), trait objects (runtime overhead)
**Rationale**: El patrón typestate en Rust usa típicamente structs vacíos con PhantomData. Alternatives añaden overhead innecesario.

### Decision: Localización del tipo DocumentChunk

**Choice**: Mantener en `src/domain/entities.rs`, añadir tipo alias para backward compat
**Alternatives considered**: Nuevo módulo `document_chunk.rs` (más refactoring)
**Rationale**: Minimiza cambios - solo añadir estados y métodos de transición donde ya existe.

### Decision: Validación en tiempo de compilación vs trait bounds

**Choice**: Trait bound `where S: Validated` en método `export()` del trait Exporter
**Alternatives considered**: Nueva trait genérica (overkill), type state en configuración del exporter (menos flexible)
**Rationale**: El trait Exporter ya existe y recibe `DocumentChunk`. Cambiar a `DocumentChunk<Validated>` requiere cambios en la trait, pero es el approach más directo.

### Decision: API backward compatible

**Choice**: Tipo alias `type DocumentChunk = DocumentChunk<Draft>;` + mantener métodos
**Alternatives considered**: Forzar uso de `.validate()` siempre (breaking change)
**Rationale**: La propuesta especifica "Mantener backward compat temporal". El alias permite código existente que usa `DocumentChunk` sin estado explícito.

## Data Flow

```
ScrapedContent ──→ DocumentChunk<Draft> ──→ .validate() ──→ DocumentChunk<Validated>
                              │                                    │
                              └────────── Exporter.export() ────────┘
                                                        │
                                            (solo acepta DocumentChunk<Validated>)
```

## File Changes

| File | Action | Description |
|------|--------|-------------|
| `src/domain/entities.rs` | Modify | Añadir estados marker, tipos estado a DocumentChunk, métodos validate/export |
| `src/domain/exporter.rs` | Modify | Actualizar trait Exporter para aceptar `DocumentChunk<Validated>` |
| `src/infrastructure/export/file_exporter.rs` | Modify | Actualizar implementacion Exporter |
| `src/export_flow.rs` | Modify | Actualizar consumers |
| `src/export_factory.rs` | Modify | Actualizar consumers |

## Interfaces / Contracts

```rust
// States marker (private - no public constructor)
struct Draft {};
struct Validated {};
struct Exported {};

// DocumentChunk estados
pub struct DocumentChunk<S = Draft> {
    id: Uuid,
    url: String,
    title: String,
    content: String,
    metadata: HashMap<String, String>,
    timestamp: DateTime<Utc>,
    embeddings: Option<Vec<f32>>,
    correlation_id: Option<String>,
    _state: PhantomData<S>,
}

// Alias para backward compatibility
type DocumentChunk = DocumentChunk<Draft>;

// Transiciones
impl DocumentChunk<Draft> {
    pub fn validate(self) -> DocumentChunk<Validated> { ... }
}

impl DocumentChunk<Validated> {
    pub fn export(self) -> ExportResult<()>
    where
        Self: Send + Sync + 'static,
    { ... }
}

// Trait Exporter actualizado
pub trait Exporter: Send + Sync + 'static {
    fn export(&self, document: DocumentChunk<Validated>) -> ExportResult<()>;
    // ...
}
```

## Testing Strategy

| Layer | What to Test | Approach |
|-------|------------|----------|
| Unit | Estados marker, transiciones | Test en `entities.rs` |
| Unit | Reject Draft en export | Test que `DocumentChunk<Draft>` no compila en export |
| Integration | FileExporter con Validated | Test de integración en `file_exporter.rs` |

## Migration / Rollout

1. **Fase 1**: Add estados marker y métodos, tipo alias backward compatible
2. **Fase 2**: Actualizar trait Exporter para requerir `Validated`
3. **Fase 3**: Actualizar consumers (`export_flow.rs`, `export_factory.rs`)
4. **Rollback**: Disable feature flag, usar tipo alias existente

## Open Questions

- [ ] ¿Validación específica? La proposal dice `validate()` pero no especifica qué checks hace — ¿content length, URL format, required fields?
- [ ] El campo `embeddings` en DocumentChunk - ¿También requiere estado? ( proposal out of scope dice AI cleaner separate)
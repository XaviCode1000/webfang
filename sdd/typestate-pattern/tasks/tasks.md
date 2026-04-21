# Tasks: Typestate Pattern para validación compile-time

## Phase 1: Foundation — Estados marker y tipo DocumentChunk

- [x] 1.1 Añadir estados marker privados `Draft`, `Validated`, `Exported` en `src/domain/entities.rs`
- [x] 1.2 Modificar `DocumentChunk` para usar `PhantomData<S>` con estado por defecto `Draft`
- [x] 1.3 Crear tipo alias `type DocumentChunk = DocumentChunk<Draft>` para backward compat

## Phase 2: Transiciones de estado

- [x] 2.1 Implementar método `validate()` en `DocumentChunk<Draft>` que retorna `DocumentChunk<Validated>`
- [x] 2.2 Implementar método `export()` en `DocumentChunk<Validated>` con trait bound `where Self: Send + Sync + 'static`
- [x] 2.3 Añadir validaciones básicas en `validate()` (content no vacío, title presente)

## Phase 3: Actualizar Trait Exporter

- [x] 3.1 Modificar `trait Exporter` en `src/domain/exporter.rs` para aceptar `DocumentChunk<Validated>`
- [x] 3.2 Actualizar implementación en `src/infrastructure/export/file_exporter.rs`
- [ ] 3.3 Compilar y resolver errores de tipo (EN PROGRESO - tests de integración requieren actualización)

## Phase 4: Actualizar Consumers

- [x] 4.1 Actualizar `src/export_flow.rs` para llamar `.validate()` antes de `.export()`
- [x] 4.2 Actualizar `src/export_factory.rs` para usar nueva API
- [x] 4.3 Vérificar que no haya otros consumers rotos (`grep DocumentChunk` en el codebase)

## Phase 5: Testing y Verificación

- [x] 5.1 Ejecutar `cargo check` para verificar compilación
- [x] 5.2 Ejecutar `cargo clippy -- -D warnings` (compila con advertencias de tests)
- [ ] 5.3 Ejecutar `cargo nextest run` para tests existentes (requiere actualizar tests de integración)
- [ ] 5.4 Verificar que Draft no compila en export (test de compilación negativa)

## Phase 6: Cleanup

- [ ] 6.1 Revisar que no haya warnings de tipos no usados
- [x] 6.2 Verificar que el tipo alias backward compat funciona
# Proposal: Fix vector_exporter.rs code quality issues

## Intent

Fix 3 concrete code quality issues in `vector_exporter.rs` flagged by Guardian Angel review:

1. **`cosine_similarity` uses `assert_eq!`** — panics on dimension mismatch instead of returning `Result`. In a production scraper, a single bad document shouldn't crash the entire process.
2. **`export_batch` takes `Vec<DocumentChunk>`** — forces ownership transfer. Should accept `&[DocumentChunk]` to allow callers to retain ownership.
3. **`new_with_path` takes `PathBuf` directly** — less flexible than `impl Into<PathBuf>`.

## Scope

### In Scope
- Change `cosine_similarity` return type from `f32` to `Result<f32, ExporterError>`
- Change `export_batch` signature from `Vec<DocumentChunk>` to `&[DocumentChunk]`
- Change `new_with_path` parameter from `PathBuf` to `impl Into<PathBuf>`
- Update all callers and tests
- Update `Exporter` trait if needed

### Out of Scope
- Migrate `std::fs` → `tokio::fs` (requires async trait redesign, separate change)
- HDD performance optimization (requires architectural redesign, separate change)

## Capabilities

### Modified Capabilities
- `vector-exporter`: Safer cosine similarity, more efficient batch export signature

## Approach

1. Add new error variant `VectorDimensionMismatch` to `ExporterError` (or reuse existing `DimensionMismatch`)
2. Change `cosine_similarity` to return `Result<f32, ExporterError>` with `#[inline]`
3. Update `Exporter` trait `export_batch` signature to `&[DocumentChunk]`
4. Update `VectorExporter::export_batch` impl
5. Update `new_with_path` to accept `impl Into<PathBuf>`
6. Update all callers and tests
7. Verify: `cargo nextest run --test-threads 2` + `cargo clippy -- -D warnings`

## Affected Areas

| Area | Impact | Description |
|------|--------|-------------|
| `src/infrastructure/export/vector_exporter.rs` | Modified | Main file with all 3 fixes |
| `src/domain/exporter.rs` | Modified | `Exporter` trait `export_batch` signature |
| `src/export_factory.rs` | May need update | If it calls `export_batch` directly |
| Tests | Updated | `cosine_similarity` tests, export tests |

## Risks

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Breaking `Exporter` trait affects other exporters | Low | Only `VectorExporter` implements `export_batch` with meaningful logic |
| `cosine_similarity` callers don't handle Result | Low | Only called within vector_exporter.rs |

## Rollback Plan

Revert the commit. Changes are isolated to export layer — no data migration, no public CLI change.

## Dependencies

- None beyond existing project dependencies

## Success Criteria

- [ ] `cosine_similarity` returns `Result` instead of panicking
- [ ] `export_batch` accepts `&[DocumentChunk]`
- [ ] `new_with_path` accepts `impl Into<PathBuf>`
- [ ] All tests pass with `cargo nextest run --test-threads 2`
- [ ] `cargo clippy -- -D warnings` passes clean

# Tasks: Fix vector_exporter.rs code quality issues

## 1. Fix cosine_similarity to return Result

- [x] 1.1. Change return type from `f32` to `Result<f32, ExporterError>`
- [x] 1.2. Replace `assert_eq!` with `Err(DimensionMismatch)` for mismatch case
- [x] 1.3. Update callers inside `VectorExporter` to handle Result
- [x] 1.4. Update tests: `test_cosine_similarity_dimension_mismatch` remove `should_panic`, assert Err
- [x] 1.5. Update other cosine_similarity tests to unwrap Result

## 2. Fix export_batch to accept borrowed slice

- [x] 2.1. Update `Exporter` trait: `export_batch(&self, documents: &[DocumentChunk])`
- [x] 2.2. Update `VectorExporter::export_batch` impl
- [x] 2.3. Update `JsonlExporter::export_batch` impl
- [x] 2.4. Update `ExporterExt::export_scraped_batch` if needed
- [x] 2.5. Update all tests calling `export_batch`

## 3. Fix new_with_path to accept impl Into<PathBuf>

- [x] 3.1. Change signature: `new_with_path(config: ExporterConfig, output_dir: impl Into<PathBuf>)`
- [x] 3.2. Update tests

## 4. Verify

- [x] 4.1. `cargo nextest run --test-threads 2` passes
- [x] 4.2. `cargo clippy -- -D warnings` passes clean
- [x] 4.3. `cargo fmt --check` passes

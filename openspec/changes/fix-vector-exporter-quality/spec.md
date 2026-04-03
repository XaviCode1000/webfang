# Spec: Fix vector_exporter.rs code quality issues

## Requirement 1: cosine_similarity returns Result instead of panicking

The function `cosine_similarity` SHALL return `Result<f32, ExporterError>` instead of panicking on dimension mismatch.

### Scenarios

**Given** two vectors of different dimensions
**When** `cosine_similarity(a, b)` is called
**Then** it SHALL return `Err(ExporterError::DimensionMismatch { ... })`

**Given** two vectors of the same dimensions
**When** `cosine_similarity(a, b)` is called
**Then** it SHALL return `Ok(similarity_value)`

**Given** two zero-magnitude vectors
**When** `cosine_similarity(a, b)` is called
**Then** it SHALL return `Ok(0.0)`

## Requirement 2: export_batch accepts borrowed slice

The `Exporter` trait method `export_batch` SHALL accept `&[DocumentChunk]` instead of `Vec<DocumentChunk>`.

### Scenarios

**Given** a slice of `DocumentChunk`
**When** `exporter.export_batch(&chunks)` is called
**Then** the caller retains ownership of `chunks`

**Given** an empty slice
**When** `exporter.export_batch(&[])` is called
**Then** it SHALL return `Ok(())` without side effects

## Requirement 3: new_with_path accepts impl Into<PathBuf>

The method `VectorExporter::new_with_path` SHALL accept `impl Into<PathBuf>` instead of `PathBuf` directly.

### Scenarios

**Given** a `&str` path
**When** `VectorExporter::new_with_path(config, "/tmp/output")` is called
**Then** it SHALL compile and work correctly

**Given** a `PathBuf`
**When** `VectorExporter::new_with_path(config, path_buf)` is called
**Then** it SHALL compile and work correctly

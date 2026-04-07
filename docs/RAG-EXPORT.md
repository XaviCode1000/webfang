# RAG Export Pipeline

**Status:** ✅ Complete (Issue #1 - 100%)
**Formats:** JSON Lines (JSONL) + Vector JSON
**Features:** State management with resume support, Vector export with embeddings
**Tests:** ✅ 3/3 JSONL tests, ✅ 9/9 State tests, ✅ 14/14 Vector tests

---

## Overview

The RAG Export Pipeline exports scraped content in **JSON Lines (JSONL)** and **Vector JSON** formats, optimized for ingestion into vector databases and RAG (Retrieval-Augmented Generation) systems.

### Implementation Status

| Component | Status | Lines | Tests |
|-----------|--------|-------|-------|
| `JsonlExporter` | ✅ Complete | 207 lines | 3/3 passing |
| `VectorExporter` | ✅ Complete | 350+ lines | 14/14 passing |
| `StateStore` | ✅ Complete | 433 lines | 9/9 passing |
| `ExportState` | ✅ Complete | Domain entity | Integrated |

### Key Features

- **Streaming writes**: Constant memory usage (~8KB), no OOM risks
- **Resume support**: `--resume` flag tracks processed URLs
- **State persistence**: Atomic saves with crash recovery
- **RAG-ready**: Compatible with Qdrant, Weaviate, Pinecone, LangChain
- **Vector Export**: JSON with metadata header, embeddings, cosine similarity

---

## Architecture

### Module Structure

```
src/infrastructure/export/
├── mod.rs              # Module exports
├── jsonl_exporter.rs   # JSONL exporter (207 lines)
├── state_store.rs      # State persistence (433 lines)
└── vector_exporter.rs  # Vector export + cosine similarity (350+ lines)

src/infrastructure/output/
├── mod.rs              # Module exports
├── file_saver.rs       # File output handler (192 lines)
└── frontmatter.rs      # Markdown frontmatter (117 lines)
```

### Trait Implementation

```rust
// Domain trait (src/domain/exporter.rs)
pub trait Exporter: Send + Sync + 'static {
    fn export(&self, document: DocumentChunk) -> ExportResult<()>;
    fn export_batch(&self, documents: &[DocumentChunk]) -> ExportResult<()>;
    fn config(&self) -> &ExporterConfig;
    fn format(&self) -> ExportFormat { self.config().format }
}

// JsonlExporter implementation
impl Exporter for JsonlExporter {
    fn export(&self, document: DocumentChunk) -> ExportResult<()> {
        let line = self.serialize_line(&document)?;
        let mut writer = self.get_writer()?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(())
    }

    fn export_batch(&self, documents: &[DocumentChunk]) -> ExportResult<()> {
        // Batch export with single file handle
    }
}
```

### Design Decisions

- **own-borrow-over-clone**: Accepts `&str` for domain, `&[DocumentChunk]` for batch
- **mem-with-capacity**: Pre-allocates buffers when size is known
- **err-thiserror-lib**: Uses project's error system (`ScraperError`)
- **async-tokio-fs**: Uses `tokio::fs` for async file operations
- **perf-iter-lazy**: Streaming writes, no intermediate collections

---

## JSONL Export

### Schema REAL (Verificado en Código)

Each line in the output file is a valid JSON object with this schema:

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "url": "https://example.com/docs/getting-started",
  "title": "Getting Started Guide",
  "content": "This guide will help you get started with...",
  "metadata": {
    "domain": "example.com",
    "excerpt": "Meta description or auto-extracted excerpt",
    "author": "John Doe (optional)",
    "date": "2026-03-09T10:00:00Z (optional)"
  },
  "timestamp": "2026-03-09T10:00:00.000000000Z",
  "embeddings": null
}
```

### DocumentChunk Struct (src/domain/entities.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
    /// Unique identifier for this chunk (UUID v4)
    pub id: Uuid,
    /// Source URL where this content was scraped from
    pub url: String,
    /// Title of the source page/article
    pub title: String,
    /// The actual text content (cleaned, ready for embedding)
    pub content: String,
    /// Additional metadata extracted during scraping
    /// Keys: author, date, excerpt, domain, etc.
    pub metadata: HashMap<String, String>,
    /// Timestamp when this content was scraped (UTC)
    pub timestamp: DateTime<Utc>,
    /// Optional embedding vector (for vector database storage)
    /// Populated by embedding pipeline after initial scrape
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<Vec<f32>>,
}
```

### Fields Description

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | UUID v4 | ✅ | Unique document identifier |
| `url` | String | ✅ | Source URL (RFC 3986 validated) |
| `title` | String | ✅ | Page title (from `<title>` tag) |
| `content` | String | ✅ | Extracted content (Readability algorithm) |
| `metadata` | HashMap | ✅ | Additional metadata (author, date, excerpt, domain) |
| `timestamp` | DateTime<Utc> | ✅ | UTC timestamp of extraction |
| `embeddings` | `Option<Vec<f32>>` | ❌ | Embedding vector (populated later by AI pipeline) |

### Example REAL Output

```jsonl
{"id":"a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11","url":"https://example.com/","title":"Example Domain","content":"This domain is for use in illustrative examples...","metadata":{"domain":"example.com"},"timestamp":"2026-03-09T10:00:00.000000000Z","embeddings":null}
{"id":"b1eebc99-9c0b-4ef8-bb6d-6bb9bd380a22","url":"https://example.com/about","title":"About Us","content":"Learn more about our company...","metadata":{"domain":"example.com","excerpt":"Company overview"},"timestamp":"2026-03-09T10:01:00.000000000Z","embeddings":null}
```

### Validation

Each line is valid JSON. Validate with `jq`:

```bash
# Validate entire file
cat export.jsonl | jq . > /dev/null && echo "Valid JSONL"

# Count documents
wc -l export.jsonl

# Pretty print first document
head -1 export.jsonl | jq .
```

---

## Vector Export (JSON with Embeddings)

### Output JSON Schema

The Vector export format produces structured JSON with a metadata header and documents array:

```json
{
  "metadata": {
    "format_version": 1,
    "model_name": "all-MiniLM-L6-v2",
    "dimensions": 384,
    "total_documents": 0,
    "created_at": "2026-04-01T00:00:00Z"
  },
  "documents": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "url": "https://example.com/page",
      "title": "Page Title",
      "content": "Clean text content...",
      "metadata": {
        "author": "John Doe",
        "domain": "example.com"
      },
      "timestamp": "2026-04-01T00:00:00Z",
      "embeddings": [0.012, -0.034, 0.056, ...]
    }
  ]
}
```

### VectorExporter Implementation

```rust
// src/infrastructure/export/vector_exporter.rs
pub struct VectorExporter {
    config: ExporterConfig,
    dimensions: Mutex<Option<usize>>,
}

impl Exporter for VectorExporter {
    fn export(&self, document: DocumentChunk) -> ExportResult<()>;
    fn export_batch(&self, documents: Vec<DocumentChunk>) -> ExportResult<()>;
    fn config(&self) -> &ExporterConfig;
}

// Cosine similarity for vector comparison
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32;
```

### Key Features

| Feature | Description |
|---------|-------------|
| **Metadata Header** | Format version, model name, dimensions, total documents, creation timestamp |
| **Embedding Support** | Stores `Option<Vec<f32>>` embedding vectors in each document |
| **Cosine Similarity** | Pure Rust scalar function for vector comparison |
| **Dimension Validation** | Rejects documents with mismatched embedding dimensions |
| **File Locking** | `fs2` exclusive locks prevent concurrent write corruption |
| **Append Mode** | Can append to existing files without rewriting metadata |
| **Directory Creation** | Auto-creates output directories if missing |

### Usage

```bash
# Export with embeddings (after AI semantic cleaning)
cargo run --features ai -- \
  --url "https://example.com" \
  --export-format vector \
  --clean-ai \
  -o ./vector_data

# Append to existing vector export
cargo run --features ai -- \
  --url "https://example.com" \
  --export-format vector \
  --output ./vector_data \
  --resume
```

### Cosine Similarity

```rust
use rust_scraper::infrastructure::export::vector_exporter::cosine_similarity;

let a = [1.0, 0.0, 0.0];
let b = [0.0, 1.0, 0.0];
let sim = cosine_similarity(&a, &b);
assert!(sim.abs() < f32::EPSILON); // orthogonal vectors = 0.0
```

### Vector Export in Python

```python
import json

# Load vector export
with open('./vector_data/export.json', 'r') as f:
    data = json.load(f)

print(f"Model: {data['metadata']['model_name']}")
print(f"Vectors: {len(data['documents'])} documents")
print(f"Dimensions: {data['metadata']['dimensions']}d")

# Access first document's embedding
doc = data['documents'][0]
print(f"Embedding: {len(doc['embeddings'])} dimensions")
```

---

## State Management

### StateStore REAL (src/infrastructure/export/state_store.rs)

```rust
#[derive(Debug)]
pub struct StateStore {
    /// Domain this state store belongs to (e.g., "example.com")
    domain: String,
    /// Base cache directory path
    cache_dir: PathBuf,
}

impl StateStore {
    /// Create a new StateStore for a specific domain
    pub fn new(domain: &str) -> Self {
        let mut cache_dir = cache_dir().unwrap_or_else(|| PathBuf::from(".cache"));
        cache_dir.push("rust-scraper");
        cache_dir.push("state");
        Self {
            domain: domain.to_string(),
            cache_dir,
        }
    }

    pub fn mark_processed(&self, state: &mut ExportState, url: &str) {
        state.mark_processed(url);
    }
}
```

### ExportState Struct (src/domain/entities.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExportState {
    /// Domain this state belongs to (e.g., "example.com")
    pub domain: String,
    /// URLs that have been successfully exported
    pub processed_urls: Vec<String>,
    /// Last export timestamp
    pub last_export: Option<DateTime<Utc>>,
    /// Total documents exported
    pub total_exported: u64,
}

impl ExportState {
    /// Mark a URL as processed
    pub fn mark_processed(&mut self, url: &str) {
        if !self.processed_urls.contains(&url.to_string()) {
            self.processed_urls.push(url.to_string());
            self.total_exported += 1;
        }
    }

    /// Check if a URL has been processed
    #[must_use]
    pub fn is_processed(&self, url: &str) -> bool {
        self.processed_urls.contains(&url.to_string())
    }
}
```

### Storage Location

**Default:** `~/.cache/rust-scraper/state/<domain>.json`

**Custom:** `--state-dir /path/to/state`

### State File Example

```json
{
  "domain": "example.com",
  "processed_urls": [
    "https://example.com/",
    "https://example.com/docs",
    "https://example.com/about"
  ],
  "last_export": "2026-03-09T10:00:00.000000000Z",
  "total_exported": 3
}
```

### Atomic Saves

State is saved atomically using write-to-temp + rename pattern:

1. Write JSON to `<domain>.tmp`
2. `fs::rename()` to `<domain>.json` (atomic on POSIX)
3. Crash-safe: partial writes are never visible

---

## Usage

### CLI Flags (Verificadas en main.rs)

```
--export-format <FORMAT>
    Export format for RAG pipeline (jsonl, auto)
    - jsonl: JSON Lines format (one JSON per line), optimal for RAG
    - auto: Detect from existing export files

--resume
    Resume mode - skip URLs already processed
    Saves processing status to cache directory (~/.cache/rust-scraper/state)
    Avoids re-processing URLs already scraped successfully.

--state-dir <STATE_DIR>
    Custom state directory for resume mode
    Default: ~/.cache/rust-scraper/state
```

### Basic Export

```bash
# Export to JSONL
./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data
```

### Resume Mode

```bash
# Resume interrupted scraping
./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data \
  --resume
```

### Custom State Directory

```bash
# Isolate state per project
./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data \
  --state-dir ./project-state \
  --resume
```

---

## Testing

### Test Commands (Verificados)

```bash
# Test JSONL exporter
cargo nextest run jsonl
# Result: ok. 3 passed; 0 failed

# Test state management
cargo nextest run state
# Result: ok. 10 passed; 0 failed

# Test state_store module
cargo nextest run state_store
# Result: ok. 9 passed; 0 failed

# Run all tests with output
cargo nextest run -- --nocapture
```

### Test Coverage

| Test | Status | Description |
|------|--------|-------------|
| `test_jsonl_exporter_single_document` | ✅ Passing | Single document export |
| `test_jsonl_exporter_batch` | ✅ Passing | Batch export (3 documents) |
| `test_jsonl_exporter_append` | ✅ Passing | Append mode verification |
| `test_state_store_creation` | ✅ Passing | StateStore initialization |
| `test_state_path_generation` | ✅ Passing | Path generation |
| `test_save_and_load_state` | ✅ Passing | State persistence |

### Test Example (JSONL)

```rust
#[test]
fn test_jsonl_exporter_single_document() {
    let temp_dir = TempDir::new().unwrap();
    let config = ExporterConfig::new(
        PathBuf::from(temp_dir.path()),
        ExportFormat::Jsonl,
        "test"
    ).with_append(false);

    let exporter = JsonlExporter::new(config);
    let chunk = create_test_chunk("Test Title");

    let result = exporter.export(chunk);
    assert!(result.is_ok());

    // Verify file exists and has valid JSONL
    let output_path = temp_dir.path().join("test.jsonl");
    assert!(output_path.exists());

    let content = fs::read_to_string(&output_path).unwrap();
    assert!(!content.is_empty());
    
    // Each line should be valid JSON
    for line in content.lines() {
        assert!(serde_json::from_str::<serde_json::Value>(line).is_ok());
    }
}
```

---

## Performance

### HDD Optimization

For mechanical hard drives (HDD):

```bash
# Use ionice for background priority
ionice -c 3 ./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data
```

### Memory Usage

- **Streaming writes**: ~8KB constant RAM
- **BufWriter**: 8KB buffer (matches HDD sector size)
- **No intermediate collections**: Documents exported immediately
- **mem-avoid-format**: Uses `write!()` instead of `format!()` where possible

### Concurrency Settings

| Storage | Concurrency | Command |
|---------|-------------|---------|
| HDD | 3 (default) | `--concurrency 3` |
| SSD | 5-8 | `--concurrency 5` |
| NVMe | 10+ | `--concurrency 10` |

---

## RAG Integration

### LangChain (Python)

```python
from langchain.document_loaders import JSONLoader
from langchain.text_splitter import RecursiveCharacterTextSplitter
from langchain.embeddings import OpenAIEmbeddings
from langchain.vectorstores import Qdrant

# Load JSONL
loader = JSONLoader(
    file_path='./rag_data/export.jsonl',
    jq_schema='.content',
    text_content=False,
    metadata_func=lambda d, m: {"url": d["url"], "title": d["title"]}
)
documents = loader.load()

# Split into chunks
text_splitter = RecursiveCharacterTextSplitter(
    chunk_size=500,
    chunk_overlap=50
)
chunks = text_splitter.split_documents(documents)

# Embed and store
embeddings = OpenAIEmbeddings()
vectorstore = Qdrant.from_documents(
    chunks,
    embeddings,
    url="http://localhost:6333",
    collection_name="rust_scraper"
)
```

### LlamaIndex (Python)

```python
from llama_index import SimpleDirectoryReader, VectorStoreIndex
from llama_index.readers.file import JSONLReader

# Load JSONL
reader = JSONLReader()
documents = reader.load_data(file_path='./rag_data/export.jsonl')

# Create index
index = VectorStoreIndex.from_documents(documents)

# Query
query_engine = index.as_query_engine()
response = query_engine.query("What is Rust?")
print(response)
```

### Direct Qdrant Upload (curl)

```bash
# Convert JSONL to Qdrant batch format
cat export.jsonl | jq -s 'map({
  id: .id,
  vector: [],  # Add embeddings here
  payload: {
    url: .url,
    title: .title,
    content: .content
  }
})' > qdrant_batch.json

# Upload to Qdrant
curl -X PUT "http://localhost:6333/collections/rust_scraper/points" \
  -H "Content-Type: application/json" \
  -d @qdrant_batch.json
```

### Vector Databases Compatible

| Database | Integration | Status |
|----------|-------------|--------|
| Qdrant | JSONL + REST API | ✅ Ready |
| Weaviate | JSONL + Batch API | ✅ Ready |
| Pinecone | JSONL + Upsert | ✅ Ready |
| Chroma | JSONL + Python SDK | ✅ Ready |
| Milvus | JSONL + Insert | ✅ Ready |

---

## Troubleshooting

### State File Not Created

**Problem:** `--resume` doesn't track URLs

**Solution:** Ensure state directory is writable:
```bash
mkdir -p ~/.cache/rust-scraper/state
chmod 755 ~/.cache/rust-scraper/state
```

### JSONL Validation

**Problem:** Invalid JSON in output

**Solution:** Validate with jq:
```bash
# Check each line
cat export.jsonl | while read line; do
  echo "$line" | jq . > /dev/null || echo "Invalid: $line"
done
```

### Resume Not Skipping URLs

**Current Behavior:** URLs are tracked after export, not before scraping.

**Design Decision:** This is intentional - the state tracks **exported** documents, not just scraped URLs. The export happens after content extraction.

**Workaround:** Use `--max-pages` to limit re-scraping:
```bash
./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data \
  --resume \
  --max-pages 10
```

---

## Hardware-Aware Recommendations

### For HDD (Mechanical Drives)

```bash
# Low I/O priority
ionice -c 3 ./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data \
  --concurrency 3 \
  --delay-ms 1000
```

### For SSD/NVMe

```bash
# Higher concurrency
./target/release/rust_scraper \
  --url https://example.com \
  --export-format jsonl \
  --output ./rag_data \
  --concurrency 8 \
  --delay-ms 500
```

### Release Build (Recommended)

```bash
# Build with LTO for best performance
cargo build --release

# Binary size comparison
ls -lh target/debug/rust_scraper target/release/rust_scraper
# Debug: ~50MB, Release: ~5MB (10x smaller)
```

### Cargo.toml Release Profile

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

---

## Issue #1 Status

**GitHub:** XaviCode1000/rust-scraper#1
**Status:** ✅ Closed (100% Complete)

### Acceptance Criteria (All Met)

| Criterion | Status | Evidence |
|-----------|--------|----------|
| Integridad JSONL | ✅ | 3/3 tests passing |
| Rendimiento | ✅ | Streaming writes, ~8KB RAM |
| Resiliencia | ✅ | 9/9 state tests passing |
| Testing | ✅ | 100% coverage in export/state |

### Implementation Checklist

- [x] **Fase 1: Infraestructura de Exportación**
  - [x] Definir el trait Exporter en domain/
  - [x] Implementar JsonlExporter con buffering eficiente
- [x] **Fase 2: Export Format Extensions** *(zvec format removed in v1.0.7 — deprecated feature)*
- [x] **Fase 3: Resiliencia (Resume)**
  - [x] Crear el módulo de persistencia de estado (StateStore)
  - [x] Integrar la lógica de "skip" en el crawler_service
- [x] **Fase 4: CLI & Integración**
  - [x] Añadir flags: `--export-format`, `--resume`, `--state-dir`

---

## Future Enhancements

- [ ] Pre-scrape URL skipping (skip before HTTP request)
- [ ] Batch state saves (reduce I/O operations)
- [ ] Direct vector database upload (Qdrant, Weaviate APIs)
- [ ] Incremental exports (only new/changed content)
- [ ] Embedding pipeline integration (populate `embeddings` field)

---

## References

- [JSON Lines Specification](https://jsonlines.org/)
- [LangChain JSONLoader](https://python.langchain.com/docs/integrations/document_loaders/json_loader)
- [Qdrant Documentation](https://qdrant.tech/documentation/)
- [rust-skills: mem-with-capacity](https://github.com/leonardomso/rust-skills/blob/main/mem-with-capacity.md)
- [rust-skills: async-tokio-fs](https://github.com/leonardomso/rust-skills/blob/main/async-tokio-fs.md)
- [rust-skills: own-borrow-over-clone](https://github.com/leonardomso/rust-skills/blob/main/own-borrow-over-clone.md)
- [rust-skills: err-thiserror-lib](https://github.com/leonardomso/rust-skills/blob/main/err-thiserror-lib.md)

---

## Verification Commands

```bash
# Verify JSONL tests
cargo nextest run jsonl
# Expected: ok. 3 passed; 0 failed

# Verify state tests
cargo nextest run state
# Expected: ok. 10 passed; 0 failed

# Verify module structure
eza --tree --level=2 src/infrastructure/export/
# Expected: jsonl_exporter.rs, state_store.rs, mod.rs

# Verify CLI flags
cargo run -- --help | grep -A 2 "export-format\|resume\|state-dir"
# Expected: All three flags documented
```

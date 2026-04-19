# 🕷️ Rust Scraper

**Extrae contenido de cualquier sitio web y guárdalo en Markdown, JSON o directamente en tu Obsidian.**

---

## 🚀 ¿Qué hace?

Rust Scraper es una herramienta de línea de comandos que te permite descargar páginas web completas con contenido limpio y bien organizado.

- **Modo interactivo** — Explora las URLs de un sitio, elige cuáles descargar y confirma antes de empezar
- **Exportación a Obsidian** — Guarda artículos directamente en tu vault con wiki-links y metadatos
- **Limpieza con IA** — Extrae solo el contenido relevante, ignora menús y publicidad
- **Sitemaps automáticos** — Encuentra todas las páginas de un sitio sin que tengas que decírselas
- **Descarga de imágenes y documentos** — PDFs, imágenes, presentaciones — todo se descarga automáticamente
- **Múltiples formatos** — Markdown, JSON, JSONL (para RAG) y Vector (con embeddings)

---

## 📦 Instalación

### Opción 1: Instalar con Cargo (Recomendado)

```bash
cd rust-scraper
cargo install --path . --features "ai,full"
```

Esto compila en modo release e instala el binario automáticamente en `~/.cargo/bin/`, listo para usar desde cualquier directorio:

```bash
rust_scraper --help
```

**Features incluidas con `ai,full`:**
- ✅ Limpieza semántica con IA (modelo ONNX local)
- ✅ Detección y descarga de imágenes
- ✅ Detección y descarga de documentos (PDF, DOCX, XLSX)

> **Nota:** La primera compilación tarda ~4 minutos. El modelo de IA (~90MB) se descarga y cachea automáticamente en `~/.cache/rust-scraper/models/` al primer uso.

### Opción 2: Compilar manualmente

```bash
git clone https://github.com/XaviCode1000/rust-scraper.git
cd rust-scraper
cargo build --release
```

Luego copia el binario a tu PATH:

```bash
cp target/release/rust_scraper ~/.local/bin/rust_scraper
# o en tu sistema:
sudo cp target/release/rust_scraper /usr/local/bin/rust_scraper
```

### Requisitos del sistema

- **Rust:** 1.88 o superior
- **Sistema operativo:** Linux, macOS o Windows

---

## 🎯 Uso rápido

Una vez instalado, ejecuta `rust_scraper` desde tu terminal:

### Tu primer raspado

```bash
rust_scraper --url https://example.com
```

Esto descarga la página principal y guarda el contenido en Markdown en la carpeta `output/`.

### Descubrir páginas automáticamente

```bash
rust_scraper --url https://example.com --use-sitemap
```

Encuentra todas las páginas del sitio usando su sitemap.

### Modo automático (sin argumentos)

```bash
# Sin --url — detecta terminal y pregunta interactivamente
rust_scraper
# → "Enter the URL to scrape: https://example.com"
```

El scraper detecta automáticamente si estás en un terminal interactivo y te pide la URL. En pipelines o scripts, falla gracefully con mensaje de error claro.

**Control automático:**

| Entorno | Comportamiento |
|---------|-------------|
| Terminal | Pide la URL interactivamente |
| Pipe | Error: "--url is required" |
| CI=true | Error: "--url is required (CI mode)" |

### Modo interactivo (TUI completo)

```bash
rust_scraper --url https://example.com --interactive
```

Se abre una interfaz en la terminal donde puedes:
- Ver todas las URLs encontradas
- Seleccionar cuáles quieres descargar
- Ver progreso en tiempo real durante scraping
- Ver errores mientras ocurren

**Controles del TUI:**

| Fase | Tecla | Acción |
|------|-------|--------|
| Selección | `↑` / `↓` | Navegar entre URLs |
| Selección | `Espacio` | Seleccionar / deseleccionar |
| Selección | `A` | Seleccionar todo |
| Selección | `D` | Deseleccionar todo |
| Selección | `Enter` | Confirmar y empezar |
| Scraping | `j` / `k` | Scroll errores |
| Cualquiera | `q` | Salir |

### Guardar en Obsidian

```bash
# Guardar directamente en tu vault
rust_scraper --url https://example.com/articulo --obsidian-wiki-links --quick-save
```

Detecta tu vault automáticamente y guarda la nota en `_inbox/`.

### Con limpieza de IA

```bash
rust_scraper --url https://example.com --clean-ai --export-format jsonl
```

La IA filtra menús, publicidad y contenido irrelevante, quedándose solo con el texto importante.

---

## 📚 Formatos de exportación

| Formato | Para qué sirve |
|---------|---------------|
| `markdown` | Lectura humana, documentación (por defecto) |
| `json` | Integración con otras aplicaciones |
| `jsonl` | Pipelines de RAG e inteligencia artificial |
| `vector` | Bases de datos vectoriales con embeddings |

---

## ⚙️ Opciones más usadas

```bash
# Guardar en una carpeta específica
rust_scraper --url https://example.com --output ./mi-carpeta

# Descargar imágenes y documentos
rust_scraper --url https://example.com --download-images --download-documents

# Limitar a 50 páginas con 2 segundos entre peticiones
rust_scraper --url https://example.com --max-pages 50 --delay-ms 2000

# Previsualizar URLs sin descargar nada
rust_scraper --url https://example.com --dry-run

# Modo silencioso (sin barras de progreso)
rust_scraper --url https://example.com --quiet
```

### Reanudar un raspado interrumpido

```bash
rust_scraper --url https://example.com --use-sitemap --max-pages 100 --resume
```

Si se interrumpe, vuelve a ejecutar el mismo comando y continúa donde lo dejó.

### Referencia completa

Para ver todas las opciones disponibles, ejecuta:

```bash
rust_scraper --help
```

O consulta la [referencia completa del CLI](docs/CLI.md) con todas las opciones, variables de entorno y ejemplos avanzados.

---

## 🔧 Configuración

Puedes crear un archivo con tus preferencias en `~/.config/rust-scraper/config.toml`:

```toml
# Valores por defecto para cada ejecución
format = "markdown"
max_pages = 50
delay_ms = 500
use_sitemap = true
```

Las opciones que pases en la línea de comandos siempre tienen prioridad sobre este archivo.

---

## 📖 Documentación

| Recurso | Para quién |
|---------|-----------|
| [Guía del CLI](docs/CLI.md) | Todos los usuarios — referencia completa de opciones |
| [Modo interactivo TUI](docs/TUI.md) | Guía completa del selector de URLs |
| [Guía de uso](docs/USAGE.md) | Ejemplos prácticos y resolución de problemas |
| [Integración con Obsidian](docs/OBSIDIAN.md) | Usuarios de Obsidian — vault, wiki-links, metadatos |
| [Limpieza con IA](docs/AI-SEMANTIC-CLEANING.md) | Usuarios avanzados — pipeline RAG |
| [Exportación RAG](docs/RAG-EXPORT.md) | Desarrolladores — JSONL, embeddings, state store |
| [Arquitectura](docs/ARCHITECTURE.md) | Desarrolladores — diseño interno del proyecto |
| [Guía de desarrollo](DEVELOPMENT.md) | Contribuidores — cómo compilar, probar y contribuir |
| [CHANGELOG](CHANGELOG.md) | Historial de cambios por versión |

---

## 📄 Licencia

MIT OR Apache-2.0

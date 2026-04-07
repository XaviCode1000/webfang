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

### Compilar desde código fuente

```bash
git clone https://github.com/XaviCode1000/rust-scraper.git
cd rust-scraper
cargo build --release
```

Luego copia el binario a tu PATH:

```bash
cp target/release/rust_scraper ~/.local/bin/rust-scraper
# o en tu sistema:
sudo cp target/release/rust_scraper /usr/local/bin/rust-scraper
```

### Requisitos del sistema

- **Rust:** 1.88 o superior
- **Sistema operativo:** Linux, macOS o Windows

### Características opcionales

| Característica | Descripción |
|---------------|-------------|
| Limpieza con IA | Extrae solo el contenido relevante usando modelos locales |
| Descarga de imágenes | Detecta y descarga imágenes automáticamente |
| Descarga de documentos | Detecta y descarga PDFs, DOCX, XLSX, etc. |

Para compilar con características opcionales, consulta la [guía de desarrollo](DEVELOPMENT.md).

---

## 🎯 Uso rápido

Una vez instalado, ejecuta `rust-scraper` desde tu terminal:

### Tu primer raspado

```bash
rust-scraper --url https://example.com
```

Esto descarga la página principal y guarda el contenido en Markdown en la carpeta `output/`.

### Descubrir páginas automáticamente

```bash
rust-scraper --url https://example.com --use-sitemap
```

Encuentra todas las páginas del sitio usando su sitemap.

### Modo interactivo (recomendado)

```bash
rust-scraper --url https://example.com --interactive
```

Se abre una interfaz en la terminal donde puedes:
- Ver todas las URLs encontradas
- Seleccionar cuáles quieres descargar
- Confirmar antes de empezar

**Controles del TUI:**

| Tecla | Acción |
|-------|--------|
| `↑` / `↓` | Navegar entre URLs |
| `Espacio` | Seleccionar / deseleccionar |
| `A` | Seleccionar todo |
| `D` | Deseleccionar todo |
| `Enter` | Confirmar y empezar |
| `q` | Salir |

### Guardar en Obsidian

```bash
# Guardar directamente en tu vault
rust-scraper --url https://example.com/articulo --obsidian-wiki-links --quick-save
```

Detecta tu vault automáticamente y guarda la nota en `_inbox/`.

### Con limpieza de IA

```bash
rust-scraper --url https://example.com --clean-ai --export-format jsonl
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
rust-scraper --url https://example.com --output ./mi-carpeta

# Descargar imágenes y documentos
rust-scraper --url https://example.com --download-images --download-documents

# Limitar a 50 páginas con 2 segundos entre peticiones
rust-scraper --url https://example.com --max-pages 50 --delay-ms 2000

# Previsualizar URLs sin descargar nada
rust-scraper --url https://example.com --dry-run

# Modo silencioso (sin barras de progreso)
rust-scraper --url https://example.com --quiet
```

### Reanudar un raspado interrumpido

```bash
rust-scraper --url https://example.com --use-sitemap --max-pages 100 --resume
```

Si se interrumpe, vuelve a ejecutar el mismo comando y continúa donde lo dejó.

### Referencia completa

Para ver todas las opciones disponibles, ejecuta:

```bash
rust-scraper --help
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

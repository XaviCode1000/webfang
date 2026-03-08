# 🐟 Función Fish `scraper` - Comandos Inteligentes

Función fish para usar rust-scraper con comandos abreviados y descriptivos.

## 📦 Instalación

La función se instala automáticamente en:
```
~/.config/fish/functions/scraper.fish
```

Y se carga automáticamente en `~/.config/fish/config.fish`.

## 🚀 Uso

```fish
scraper <comando> [opciones] <url>
```

## 📋 Comandos

| Comando | Descripción | Flag Equivalente |
|---------|-------------|------------------|
| `(ninguno)` | Scraping básico (todos los assets) | - |
| `sitemap` | Usa sitemap.xml (auto-descubre de robots.txt) | `--use-sitemap` |
| `md` | Solo documentos Markdown | (default) |
| `img` | Solo imágenes | `--download-images` |
| `doc` | Solo documentos (PDF, DOCX, XLSX, PPTX) | `--download-documents` |
| `all` | Todos los assets (imágenes + documentos) | `--download-images --download-documents` |
| `ui` | Modo interactivo TUI (selector visual) | `--interactive` |
| `help` | Mostrar ayuda | `--help` |

## 🔧 Opciones

| Opción | Descripción | Default |
|--------|-------------|---------|
| `-o, --output <dir>` | Directorio de salida | `./output` |
| `-c, --concurrency <n>` | Descargas simultáneas | `5` |
| `-v, --verbose` | Logging detallado (debug) | - |

## 📖 Ejemplos

### Scraping Básico

```fish
# Scraping básico (todos los assets)
scraper https://example.com

# Con directorio personalizado
scraper -o ./mi-scrape https://example.com
```

### Con Sitemap

```fish
# Auto-descubre sitemap de robots.txt
scraper sitemap https://example.com

# Sitemap explícito
scraper sitemap --sitemap-url https://example.com/sitemap.xml.gz https://example.com
```

### Por Tipo de Asset

```fish
# Solo imágenes
scraper img https://example.com

# Solo documentos
scraper doc https://example.com

# Imágenes + documentos
scraper all https://example.com

# Combinado con sitemap
scraper sitemap img https://example.com
```

### Modo Interactivo (TUI)

```fish
# Selector interactivo de URLs
scraper ui https://example.com

# Con sitemap
scraper ui sitemap https://example.com
```

### Avanzado

```fish
# Concurrency personalizada
scraper -c 10 sitemap https://example.com

# Logging detallado
scraper -v sitemap img https://example.com

# Combinado completo
scraper -v -c 3 -o ./docs sitemap doc https://example.com
```

## 🎯 Casos de Uso Comunes

### 1. Backup de Blog

```fish
scraper sitemap md img https://myblog.com
```

### 2. Descargar Documentación

```fish
scraper sitemap doc -o ./docs https://docs.example.com
```

### 3. Scraping Selectivo (TUI)

```fish
scraper ui sitemap https://example.com
```

### 4. Descarga Rápida (poca concurrencia)

```fish
scraper -c 2 sitemap https://example.com
```

### 5. Debug/Verbose

```fish
scraper -v sitemap https://example.com
```

## 🔍 Ayuda

```fish
scraper --help
# o
scraper help
```

## 🎨 Alias Útiles (Opcional)

Agregá estos alias a `~/.config/fish/config.fish`:

```fish
alias scrap="scraper"
alias scrap-ui="scraper ui"
alias scrap-sitemap="scraper sitemap"
alias scrap-img="scraper img"
alias scrap-doc="scraper doc"
```

## 📊 Comparativa: CLI vs Función Fish

| CLI Original | Función Fish |
|--------------|--------------|
| `rust_scraper --url https://example.com` | `scraper https://example.com` |
| `rust_scraper --use-sitemap --url https://example.com` | `scraper sitemap https://example.com` |
| `rust_scraper --download-images --url https://example.com` | `scraper img https://example.com` |
| `rust_scraper --interactive --use-sitemap --url https://example.com` | `scraper ui sitemap https://example.com` |
| `rust_scraper --download-images --download-documents --url https://example.com` | `scraper all https://example.com` |

## 🐛 Troubleshooting

### "Command not found: scraper"

Recargá fish:
```fish
exec fish
```

O sourceá manualmente:
```fish
source ~/.config/fish/functions/scraper.fish
```

### "❌ Error: URL requerida"

La URL es obligatoria. Ejemplo correcto:
```fish
scraper sitemap https://example.com  # ✅
scraper sitemap                       # ❌
```

### "❌ Error: Argumento desconocido"

Verificá que los comandos estén en orden:
```fish
# ✅ Correcto: comando antes de URL
scraper sitemap img https://example.com

# ❌ Incorrecto: URL antes de comando
scraper https://example.com sitemap img
```

## 📝 Notas

- La función usa `rust_scraper` desde `~/Dev/my_apps/rust_scraper/target/release/rust_scraper`
- Si el binario no existe, construiló con: `cargo build --release`
- Los comandos se pueden combinar: `scraper sitemap img doc` = sitemap + imágenes + documentos

# TUI — Selector Interactivo de URLs

**Versión:** 1.1.0 · **Última actualización:** Abril 2026

---

## ¿Qué es?

El modo TUI (Terminal User Interface) es una interfaz interactiva en la terminal que te permite explorar, seleccionar y elegir qué páginas web descargar antes de empezar.

En lugar de darle una lista de URLs al scraper y esperar que procese todo, puedes **ver todas las URLs encontradas, marcar las que te interesan y confirmar** antes de que empiece la descarga.

---

## ¿Cuándo usarlo?

| Situación | Modo recomendado |
|-----------|-----------------|
| Quieres elegir qué páginas descargar | **TUI** (`--interactive`) |
| Quieres descargar todas las páginas | Headless (normal) |
| Estás automatizando con un script | Headless (normal) |
| No estás seguro de qué contiene el sitio | **TUI** (`--interactive`) |

---

## Cómo empezar

```bash
rust-scraper --url https://example.com --interactive
```

Si también quieres usar el sitemap:

```bash
rust-scraper --url https://example.com --use-sitemap --interactive
```

---

## Pantalla del TUI

Cuando se abre el TUI, verás algo así:

```
┌────────────────────────────────────────────────────────┐
│ 🕷️ URL Selector - Space: Select, Enter: Download, q: │
└────────────────────────────────────────────────────────┘
┌────────────────────────────────────────────────────────┐
│ URLs (2/15)                                            │
│ ▶ ✅ https://example.com/                             │
│   ✅ https://example.com/about                        │
│   ⬜ https://example.com/contact                       │
│   ⬜ https://example.com/blog/post-1                   │
│   ⬜ https://example.com/blog/post-2                   │
│   ⬜ https://example.com/docs/getting-started           │
│   ...                                                   │
└────────────────────────────────────────────────────────┘
┌────────────────────────────────────────────────────────┐
│ 📊 2 selected (15 total) | ↑↓: Navigate | Space:     │
│    Toggle | A: All | D: None                           │
└────────────────────────────────────────────────────────┘
```

### Elementos de la pantalla

| Elemento | Descripción |
|----------|-------------|
| **Barra superior** | Recordatorio de teclas principales |
| **Lista de URLs** | Todas las URLs encontradas con casilla (✅/⬜) |
| **Cursor (▶)** | Indica la URL seleccionada actualmente |
| **Barra inferior** | Contador de seleccionadas y atajos disponibles |

---

## Controles

### Navegación

| Tecla | Acción |
|-------|--------|
| `↑` | Mover cursor hacia arriba |
| `↓` | Mover cursor hacia abajo |

### Selección

| Tecla | Acción |
|-------|--------|
| `Espacio` | Marcar / desmarcar la URL bajo el cursor |
| `A` | Marcar **todas** las URLs |
| `D` | Desmarcar **todas** las URLs |

### Confirmación y salida

| Tecla | Acción |
|-------|--------|
| `Enter` | Pedir confirmación para empezar la descarga |
| `Y` | Sí, empezar descarga (dentro de la confirmación) |
| `N` | No, volver a la selección (dentro de la confirmación) |
| `Esc` | Cancelar confirmación, volver a la selección |
| `q` | Salir sin descargar nada |

---

## Flujo completo

```
1. rust-scraper --url https://example.com --interactive
                          │
2. Descubre URLs (con spinner)
   ├── Sin sitemap: extrae links de la página
   └── Con --use-sitemap: lee sitemap.xml
                          │
3. Abre el TUI con todas las URLs encontradas
                          │
4. El usuario navega, selecciona y confirma
                          │
5. Descarga solo las URLs marcadas
   └── Con barra de progreso por página
                          │
6. Guarda los resultados en output/
```

---

## Flujo de confirmación

Cuando pulsas `Enter` y al menos una URL está marcada, aparece la confirmación:

```
┌────────────────────────────────────────────────────────┐
│ 🚀 Start download? (Y/N)                              │
└────────────────────────────────────────────────────────┘
```

- **`Y`** — Empieza la descarga de las URLs marcadas
- **`N`** o **`Esc`** — Vuelve a la pantalla de selección
- **`q`** — Sale sin descargar nada

---

## Seguridad del terminal

### Restauración automática

El TUI **siempre restaura el terminal** a su estado normal, incluso si:

- Pulsas `q` para salir
- La aplicación termina normalmente
- Ocurre un panic (error catastrófico)

### ¿La terminal se quedó rota?

Si por alguna razón tu terminal no se restauró correctamente, ejecuta:

```bash
reset
```

Esto devuelve la terminal a su estado normal.

---

## Limitaciones conocidas

| Limitación | Detalle |
|------------|---------|
| **No funciona en pipes** | El TUI necesita un terminal interactivo. No puedes hacer `echo "url" \| rust-scraper --interactive` |
| **SSH** | Funciona sobre SSH siempre que el terminal sea interactivo |
| **Tamaño mínimo** | Se recomienda un terminal de al menos 80×24 caracteres |
| **Emojis** | Requiere una fuente que soporte emojis para los iconos ✅⬜▶ |

---

## Ejemplos prácticos

### Explorar un blog antes de descargar

```bash
# Descubre todas las entradas del blog y elige cuáles descargar
rust-scraper --url https://mi-blog.com --use-sitemap --interactive
```

### Elegir documentación específica

```bash
# Encuentra todas las páginas del sitio y selecciona solo la docs
rust-scraper --url https://docs.ejemplo.com --use-sitemap --interactive
```

### Explorar con límite de páginas

```bash
# Descubre hasta 50 URLs, elige las que quieras y descarga
rust-scraper --url https://ejemplo.com --max-pages 50 --interactive
```

---

## Integración con otras funciones

El TUI funciona con todas las demás opciones del scraper:

| Opción | Compatible con TUI |
|--------|-------------------|
| `--use-sitemap` | ✅ — Descubre más URLs |
| `--max-pages` | ✅ — Limita cuántas URLs aparecen |
| `--delay-ms` | ✅ — Controla velocidad de descarga |
| `--format` | ✅ — Elige formato de salida |
| `--download-images` | ✅ — Descarga imágenes de las URLs seleccionadas |
| `--obsidian-wiki-links` | ✅ — Guarda en Obsidian |
| `--clean-ai` | ✅ — Limpia contenido con IA |
| `--quiet` | ⚠️ — No recomendado con TUI (silencia info) |
| `--dry-run` | ⚠️ — El dry-run muestra URLs sin TUI |

---

## Arquitectura (para desarrolladores)

El TUI está implementado como un **Adapter** en Clean Architecture:

```
src/adapters/tui/
├── mod.rs              # Módulo público (run_selector, TuiError)
├── terminal.rs         # Setup/restore del terminal + panic hook
└── url_selector.rs     # State machine + widget de ratatui
```

**Principio clave:** La capa Application **nunca** importa `ratatui` ni `crossterm`. El TUI es un puerto de entrada, no lógica de negocio.

**Tecnologías:**
- **ratatui** — Framework de UI para terminal
- **crossterm** — Control del terminal (raw mode, alternate screen, eventos)

**Seguridad:** Panic hook independiente restaura el terminal paso a paso — si un paso falla, los demás se ejecutan igualmente.

---

## Véase también

- [Guía completa del CLI](CLI.md) — Todas las opciones y variables de entorno
- [Guía de uso](USAGE.md) — Ejemplos prácticos
- [Integración con Obsidian](OBSIDIAN.md) — Guardar en tu vault

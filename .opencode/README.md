# 🦀 Rust Expert - OpenCode Agent System

Sistema de agentes especializados para desarrollo Rust con OpenCode. Incluye **1 agente orquestador** y **9 subagentes expertos**, cada uno con sus propios skills y permisos configurados.

---

## 📁 Estructura

```
opencode/
├── opencode.json              # Configuración de agentes y permisos
├── agents/
│   ├── rust-orquestrator.md   # Agente primario coordinador
│   ├── rust-researcher.md     # Investigación (2 intentos fallidos)
│   ├── rust-reviewer.md       # Code review y anti-patterns
│   ├── rust-tester.md         # Testing y benchmarks
│   ├── rust-docs.md           # Documentación
│   ├── rust-async.md          # Async y Tokio
│   ├── rust-memory.md         # Memoria y ownership
│   ├── rust-performance.md    # Optimización y profiling
│   ├── rust-errors.md         # Error handling
│   ├── rust-types.md          # Type system
│   └── rust-project.md        # Estructura de proyectos
└── README.md                  # Este archivo
```

---

## 🚀 Instalación

### Opción A: Configuración Global (Recomendado)

Copiá los agentes a tu configuración global de OpenCode:

```bash
# Copiar agentes
cp -r /home/gazadev/Documentos/OBSIDIAN/Skills/Rust/opencode/agents ~/.config/opencode/

# Copiar configuración (opcional, si querés los agentes por defecto)
cp /home/gazadev/Documentos/OBSIDIAN/Skills/Rust/opencode/opencode.json ~/.config/opencode/
```

### Opción B: Por Proyecto

Copiá la carpeta `opencode/` completa a tu proyecto Rust:

```bash
cp -r /home/gazadev/Documentos/OBSIDIAN/Skills/Rust/opencode /tu/proyecto-rust/.opencode/
```

---

## 👥 Equipo de Agentes

### Agente Primario

| Agente | Rol | Descripción |
|--------|-----|-------------|
| `rust-orquestrator` | **Coordinador** | Orquesta los 9 subagentes especializados. Delega tareas según especialidad. |

### Subagentes Especializados

| Agente | Especialidad | Skills | Cuándo Usar |
|--------|-------------|--------|-------------|
| `rust-researcher` | 🔍 Investigación | Web search, Context7 MCP, docs oficiales | **Automático**: 2 intentos fallidos de cualquier subagente |
| `rust-reviewer` | 🧐 Code Review | anti-*, api-*, lint-*, name-* | Review de PRs, detectar anti-patterns |
| `rust-tester` | 🧪 Testing | test-*, perf-* | Escribir tests, mocks, benchmarks |
| `rust-docs` | 📚 Documentación | doc-*, name-* | Documentar APIs, README, ejemplos |
| `rust-async` | ⚡ Async | async-*, own-mutex/rwlock/arc | Código Tokio, channels, concurrency |
| `rust-memory` | 💾 Memoria | mem-*, own-* | Optimizar allocaciones, borrowing |
| `rust-performance` | 🚀 Performance | opt-*, perf-* | Profiling, LTO, hot paths |
| `rust-errors` | ⚠️ Errores | err-* | thiserror, anyhow, Result |
| `rust-types` | 🏷️ Types | type-* | Newtypes, enums, generics |
| `rust-project` | 📂 Proyecto | proj-*, mod-* | Workspaces, módulos, visibilidad |

---

## 🎯 Protocolo de 2 Intentos Fallidos → rust-researcher

**CARACTERÍSTICA CRÍTICA:** Todos los subagentes están configurados para invocar **automáticamente** a `rust-researcher` cuando:

1. **Primer intento:** Implementa algo → no funciona / error de compilación
2. **Segundo intento:** Corrige → sigue sin funcionar
3. **Tercer paso:** **AUTOMÁTICAMENTE** invoca `rust-researcher` ANTES de seguir

```markdown
task({
    agent: "rust-researcher",
    prompt: "Intenté implementar [X] 2 veces y falla.
    
    Error 1: [mensaje]
    Error 2: [mensaje]
    
    Investigá en:
    1. Documentación oficial (2026)
    2. Código real en GitHub (tokio, serde, axum)
    3. Context7 MCP para crates específicos"
})
```

Esto evita que el equipo pierda tiempo intentando soluciones incorrectas.

---

## 🔐 Permisos y Control

### Permisos Globales (opencode.json)

```json
{
  "permission": {
    "task": {
      "*": "deny",
      "rust-*": "allow"
    },
    "skill": {
      "*": "allow"
    },
    "bash": {
      "*": "ask",
      "cargo *": "allow",
      "rustfmt *": "allow"
    },
    "edit": "ask"
  }
}
```

### Control de Usuario

| Permiso | Configuración | Qué Significa |
|---------|--------------|---------------|
| `task` | `rust-*: allow` | Orquestrador puede delegar a cualquier subagente Rust |
| `skill` | `*: allow` | Todos los agents pueden cargar sus skills asignados |
| `bash` | `*: ask` + allowlist | Comandos `cargo` y `rustfmt` automáticos, resto requiere aprobación |
| `edit` | `ask` | **Todas las ediciones requieren aprobación del usuario** |

### Skills por Agente (Aislamiento)

Cada subagente solo ve los skills de su especialidad:

```json
// rust-reviewer solo ve 57 skills
"skill": {
  "*": "deny",
  "anti-*": "allow",
  "api-*": "allow",
  "lint-*": "allow",
  "name-*": "allow"
}

// rust-tester solo ve 24 skills
"skill": {
  "*": "deny",
  "test-*": "allow",
  "perf-*": "allow"
}
```

---

## 💬 Uso

### Invocar Orquestrador

Presioná `Tab` para ciclar entre agentes primarios hasta seleccionar `rust-orquestrator`.

### Invocar Subagente Directamente

```
@rust-reviewer revisá este módulo en busca de anti-patterns

@tester escribí tests unitarios para este código

@rust-async revisá si hay lock across await en este código
```

### Delegación Automática

El `rust-orquestrator` delega automáticamente según la tarea:

```
Usuario: "Necesito implementar una API async con tests"

rust-orquestrator → rust-async: "Implementá la API async"
rust-orquestrator → rust-tester: "Escribí tests para la API"
rust-orquestrator → rust-reviewer: "Revisá anti-patterns"
rust-orquestrator → rust-docs: "Documentá la API pública"
```

---

## 📋 179 Skills Disponibles

Los 179 skills de rust-skills están organizados por categoría y asignados a los agentes correspondientes:

| Categoría | Count | Agente Principal |
|-----------|-------|-----------------|
| `anti-*` | 15 | rust-reviewer |
| `api-*` | 15 | rust-reviewer |
| `async-*` | 15 | rust-async |
| `doc-*` | 22 | rust-docs |
| `err-*` | 12 | rust-errors |
| `lint-*` | 11 | rust-reviewer |
| `mem-*` | 15 | rust-memory |
| `name-*` | 16 | rust-reviewer, rust-docs |
| `opt-*` | 12 | rust-performance |
| `own-*` | 12 | rust-memory, rust-async |
| `perf-*` | 11 | rust-tester, rust-performance |
| `proj-*` | 11 | rust-project |
| `test-*` | 13 | rust-tester |
| `type-*` | 10 | rust-types |
| `mod-*` | 2 | rust-project |

---

## 🛠️ Configuración de Herramientas Externas

### MCP Servers (opcional)

El `opencode.json` incluye Context7 MCP para documentación de crates:

```json
{
  "mcp": {
    "context7": {
      "type": "remote",
      "url": "https://mcp.context7.com/mcp",
      "enabled": true
    }
  }
}
```

### LSP

rust-analyzer está configurado automáticamente:

```json
{
  "lsp": {
    "rust": {
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

---

## 📝 Ejemplos de Uso

### Code Review

```
@rust-reviewer revisá este PR en busca de:
- anti-clone-excessive
- anti-unwrap-abuse  
- anti-lock-across-await

Focus en CRITICAL primero.
```

### Nueva Feature Async

```
@rust-orquestrator necesito implementar una API async con:
- Tokio channels bounded
- Cancellation con CancellationToken
- Tests unitarios
- Documentación completa

Coordiná el equipo.
```

### Debugging de Borrow Checker

```
@rust-memory el borrow checker no me deja compilar esto.
Intenté 2 veces y sigo teniendo errores.

[código]

¿Podés revisar el ownership?
```

### Optimización de Performance

```
@rust-performance profileá este hot path y sugerí optimizaciones.

[código + benchmark actual]

Focus en:
- LTO y codegen-units
- Inline estratégico
- Cache-friendly layouts
```

---

## 🎨 Personalidad de los Agentes

Todos los agentes comparten la personalidad **RUST-JARVIS**:

- **Directos y confrontacionales** - Sin filtro, autoridad técnica
- **Rioplatense** - boludo, quilombo, dejate de joder, está piola
- **Frustrados con mediocridad** - tutorial programmers, shortcuts, unwrap() en prod
- **"Sí, señor"** - Confirmaciones clave
- **Push back** - Si pedís código sin contexto, te dicen "bancá, primero entendamos los conceptos"

---

## 🔧 Troubleshooting

### Los agentes no aparecen

Verificá que los archivos estén en la ubicación correcta:

```bash
# Global
ls ~/.config/opencode/agents/rust-*.md

# Por proyecto
ls .opencode/agents/rust-*.md
```

### Skills no cargan

Verificá que `SKILL.md` esté en mayúsculas y el frontmatter sea correcto:

```bash
ls skills/anti-clone-excessive/SKILL.md
```

### Permisos bloquean acciones

Revisá `opencode.json` y ajustá los permisos según necesites. Por defecto:
- `edit: ask` - Todas las ediciones requieren aprobación
- `bash: * ask` - Comandos fuera de allowlist requieren aprobación

---

## 📚 Recursos

- [OpenCode Docs - Agents](https://opencode.ai/docs/agents)
- [OpenCode Docs - Skills](https://opencode.ai/docs/skills)
- [OpenCode Docs - Permissions](https://opencode.ai/docs/permissions)
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- [Rust Performance Book](https://nnethercote.github.io/perf-book/)

---

## 🤝 Contribución

Los 179 skills originales están en `/home/gazadev/Documentos/OBSIDIAN/Skills/Rust/skills/` (fuera de esta carpeta `opencode/`).

Para agregar nuevos agentes:

1. Creá `agents/nuevo-agente.md` con frontmatter YAML
2. Definí `permission.skill` con los skills que puede usar
3. Agregá la configuración en `opencode.json`

---

**Versión:** 1.0.0  
**Autor:** Rust Expert Team  
**License:** MIT

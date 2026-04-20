# Exploration: Reemplazar Arc<Mutex<Vec<T>>> con canales asíncronos mpsc

## Current State

### ResultsCollector Actual (deduplicator.rs:152-211)
- Usa `Arc<Mutex<Vec<T>>>` para almacenar resultados
- Métodos: `add()`, `get_all()`, `len()`, `is_empty()`, `clear()`
- **Problema**: Lock contention cuando múltiples tareas escriben simultáneamente

### Uso en crawler_service.rs (línea 405)
```rust
let results = Arc::new(Mutex::new(Vec::new()));
```

**Lock contention ocurre en:**
1. **Línea 420-425**: Verificación de `max_pages` (lectura)
2. **Línea 485-487**: push de `DiscoveredUrl` en task async
3. **Línea 556-562**: Recolección final de resultados

### Patrón Existente en el Proyecto
El proyecto YA usa `tokio::sync::mpsc` exitosamente para:
- `ScrapeProgress` en TUI (progress_view.rs, event_loop.rs)
- Eventos de aplicación en el event loop
- Canal de 100 capacidad con bounded backpressure

## Affected Areas

- `src/application/deduplicator.rs` — Definición de `ResultsCollector<T>` (líneas 152-211)
- `src/application/crawler_service.rs` — Uso de results mutex (líneas 405, 420, 485, 556)
- `src/domain/result/crawl_result.rs` — Tipo `CrawlResult` retornado

## Approaches

### Approach 1: mpsc con worker dedicado (RECOMENDADO)
- Crear `ResultsCollectorMpsc<T>` con `mpsc::Sender<T>` compartible
- Worker task que colecta en un `Vec<T>` interno
- Shutdown con señal de `None` o `Sender` dropped

| Pros | Cons |
|------|------|
| Elimina lock contention completamente | Más complejo de implementar |
| Backpressure natural con canal bounded | Requiere shutdown graceful |
| Patrón ya usado en el proyecto | Memory extra para el worker |

**Esfuerzo**: Medium-High

### Approach 2: Mutex por operación + reduce locking
- Mantener `Arc<Mutex<Vec<T>>>` pero minimizando tiempo de lock
- Usar `try_lock()` y reintentar en vez de `lock().await`
- Colectar en batches locales y hacer flush periódico

| Pros | Cons |
|------|------|
| Menor cambio, más incremental | No elimina el problema de raíz |
| Backward compatible con API existente | Still has some contention |

**Esfuerzo**: Low

### Approach 3: lockfree::flavors (crossbeam)
- Usar `crossbeam::queue::SegQueue` o similar
- No requiere cambios en arquitectura de tasks

| Pros | Cons |
|------|------|
| Extremadamente rápido | Dependencia adicional |
| Sin lock contention | Puede ser overkill |

**Esfuerzo**: Medium

## Recommendation

**Approach 1: mpsc con worker dedicado**

### Diseño Propuesto

```rust
// Nuevo tipo en deduplicator.rs
pub struct ResultsCollectorMpsc<T: Clone + Send> {
    tx: mpsc::Sender<T>,
    // Handle al worker para shutdown
    worker_handle: tokio::task::JoinHandle<Vec<T>>,
}

// Constructor
impl<T: Clone + Send> ResultsCollectorMpsc<T> {
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        
        let worker_handle = tokio::spawn(async move {
            let mut results = Vec::new();
            while let Some(item) = rx.recv().await {
                results.push(item);
            }
            results
        });
        
        Self { tx, worker_handle }
    }
    
    pub async fn add(&self, result: T) {
        // Backpressure: send() espera si canal lleno
        self.tx.send(result).await.ok();
    }
    
    pub async fn shutdown(self) -> Vec<T> {
        drop(self.tx); // Cierra el canal
        self.worker_handle.await.unwrap_or_default()
    }
}
```

### Integración en crawler_service.rs

```rust
// Reemplazar línea 405:
let (results_tx, results_rx) = tokio::sync::mpsc::channel(100);
let results_worker = tokio::spawn(async move {
    let mut results = Vec::new();
    while let Some(item) = results_rx.recv().await {
        results.push(item);
    }
    results
});

// En task (línea 485):
results_tx.send(discovered_url_task).await.ok();

// Verificación max_pages (línea 420):
// Necesita AtomicUsize counter o similar

// Shutdown (línea 556):
drop(results_tx);
let collected = results_worker.await.unwrap();
let total_pages = collected.len();
```

### Manejo de shutdown graceful
1. **Canal cerrado**: Worker termina cuando todos los `Sender` son dropeados
2. **Signal de shutdown**: Enviar `None` como mensaje especial
3. **Timeout**: Usar `tokio::time::timeout` para no bloquear infinitamente

## Risks

1. **Memory pressure**: Si el canal está lleno y producers son rápidos, memory crece
   - **Mitigación**: Usar `channel(capacity)` con capacidad razonable (100-1000)

2. **Deadlock si no se hace drop del Sender**: El worker nunca termina
   - **Mitigación**: Always drop `tx` antes de esperar `worker_handle`

3. **Backpressure puede bloquear producers**: `send()` espera si canal lleno
   - **Mitigación**: Usar `try_send()` + log warning si no puede enviar

4. **Verificación de max_pages requiere contador separado**: Con mpsc no se puede hacer `.len()` en el canal
   - **Mitigación**: Usar `Arc<AtomicUsize>` para contar resultados

5. **Breaking API change**: `ResultsCollector` actual es `Clone`, el nuevo no lo sería
   - **Mitigación**: Mantener ambos tipos o hacer `Clone` con `Arc<Inner>`

## Ready for Proposal

**Sí** — La exploración está completa. El orchestrator debería indicar al usuario que:
- Approach recomendado: mpsc con worker dedicado
- Cambios principales: deduplicator.rs + crawler_service.rs
- El patrón ya existe en el proyecto (TUI progress)
- Requiere manejo cuidadoso de shutdown

## Archivos a Modificar

| Archivo | Cambios |
|---------|---------|
| `src/application/deduplicator.rs` | Agregar `ResultsCollectorMpsc<T>` |
| `src/application/crawler_service.rs` | Reemplazar mutex con mpsc channel |
| `src/domain/result/crawl_result.rs` | Probablemente ninguno |
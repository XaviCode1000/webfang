## 1. Descripción General

Actualmente, **Rust Scraper** exporta contenido principalmente a archivos Markdown individuales. Si bien esto es útil para lectura humana, es ineficiente para pipelines de **RAG (Retrieval-Augmented Generation)**, donde se requiere ingesta masiva de datos estructurados y búsqueda semántica de baja latencia.

Esta issue propone implementar un pipeline de exportación robusto que soporte formatos estructurados (JSONL) y una base de datos vectorial embebida (Zvec) para permitir búsquedas semánticas locales sin dependencias de red o servidores externos.

## 2. Objetivos Principales

*   **Estandarización de Datos:** Implementar exportación a JSONL (JSON Lines) para facilitar la ingesta en bases de datos vectoriales y LLMs.
*   **Búsqueda Semántica Local:** Integrar alibaba/zvec como motor de almacenamiento vectorial embebido para habilitar RAG in-process.
*   **Resiliencia de Pipeline:** Implementar un sistema de estado (.scraper_state.json) para permitir la reanudación de procesos interrumpidos (--resume).

## 3. Especificaciones Técnicas

*   **Formato JSONL:** Cada línea debe ser un objeto JSON válido con el esquema:
    ```json
    {"url": "...", "title": "...", "content": "...", "metadata": {...}, "timestamp": "..."}
    ```
*   **Adaptador Zvec:**
    *   Utilizar zvec-bindings para la interacción con el motor Proxima.
    *   Implementar un esquema de colección que soporte: id (UUID), text (String), embedding (Vec<f32>).
*   **Sistema de Estado:**
    *   Archivo: ~/.cache/rust-scraper/state/<domain>.json.
    *   Lógica: Registrar URLs procesadas exitosamente para evitar duplicados en ejecuciones posteriores.

## 4. Arquitectura (Clean Architecture)

*   **Domain:** Definir el trait Exporter y las entidades DocumentChunk.
*   **Application:** Refactorizar el flujo de guardado para que sea agnóstico al formato de salida.
*   **Infrastructure:** Implementar ZvecExporter y JsonlExporter en src/infrastructure/export/.

## 5. Plan de Implementación (Task List)

- [ ] **Fase 1: Infraestructura de Exportación**
    - [ ] Definir el trait Exporter en domain/.
    - [ ] Implementar JsonlExporter con buffering eficiente.
- [ ] **Fase 2: Integración Zvec**
    - [ ] Añadir zvec-bindings y configurar el esquema de colección.
    - [ ] Implementar ZvecExporter con inserciones en lote (batch inserts).
- [ ] **Fase 3: Resiliencia (Resume)**
    - [ ] Crear el módulo de persistencia de estado (StateStore).
    - [ ] Integrar la lógica de "skip" en el crawler_service si la URL ya existe en el estado.
- [ ] **Fase 4: CLI & Integración**
    - [ ] Añadir flags: --export-format [markdown|jsonl|zvec], --resume.

## 6. Criterios de Aceptación (QA)

1.  **Integridad:** Los archivos JSONL generados son válidos y parseables por herramientas estándar (jq, pandas).
2.  **Rendimiento:** La inserción en Zvec no bloquea el hilo principal de scraping (uso de canales asíncronos).
3.  **Resiliencia:** Si el proceso es interrumpido (Ctrl+C), al ejecutar con --resume, el scraper omite las URLs ya procesadas.
4.  **Testing:** 100% de cobertura en la lógica de exportación y persistencia de estado.

---

## Notas para el desarrollador:

*   Recuerda que para Zvec necesitamos vectores. Esta issue prepara el terreno para que, en la siguiente fase (IA), simplemente inyectemos el generador de embeddings en el pipeline.
*   Prioriza el uso de tokio::sync::mpsc para enviar los documentos al exportador, evitando que la escritura en disco/DB ralentice el scraping.

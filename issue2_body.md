## 1. Contexto y Motivación

Actualmente, la limpieza de contenido en **Rust Scraper** depende de selectores CSS manuales y heurísticas de densidad de texto. Este enfoque es frágil ante cambios en el DOM y a menudo incluye "ruido" (menús de navegación, footers, scripts, imágenes base64) que degrada la calidad de los datasets para RAG.

Para elevar la calidad de los datos a un estándar de producción en 2026, implementaremos una capa de inferencia local utilizando **Small Language Models (SLMs)**. Esto permitirá clasificar bloques de contenido semánticamente y extraer únicamente el texto relevante, optimizando el uso de hardware local (AVX2).

## 2. Objetivos Principales

*   **Limpieza Semántica:** Clasificar bloques de contenido (chunks) mediante inferencia local para filtrar ruido visual y estructural.
*   **Optimización de Hardware:** Exprimir las capacidades de instrucciones **AVX2** del sistema host mediante el motor `tract-onnx`.
*   **Privacidad Total:** Procesamiento 100% local; ningún dato del scraping sale de la máquina del usuario.

## 3. Especificaciones Técnicas

*   **Modelo:** `all-MiniLM-L6-v2` (formato ONNX).
*   **Motor de Inferencia:** `tract-onnx` (100% Rust, nativo).
*   **Tokenización:** `tokenizers` (HuggingFace).
*   **Arquitectura:** Implementación del trait `SemanticCleaner` en la capa de Dominio.
*   **Caché de Modelos:** Descarga automática y persistencia en `~/.cache/rust-scraper/ai_models/`.

## 4. Diseño Arquitectónico (Clean Architecture)

*   **Domain:** Definir el trait `SemanticCleaner` y los tipos de error `CleanerError`.
*   **Infrastructure:** Implementar el motor de inferencia (`MiniLmCleaner`) utilizando `tract`.
*   **Adapters:** Integrar el flag `--clean-ai` en la CLI y el indicador visual `[AI Processing]` en la TUI.

## 5. Plan de Implementación (Task List)

- [ ] **Fase 1: Integración de IA**
    - [ ] Añadir `tract-onnx`, `tokenizers` y `hf-hub` al `Cargo.toml`.
    - [ ] Implementar el cargador de modelos con soporte para caché local.
- [ ] **Fase 2: Algoritmo de Segmentación**
    - [ ] Crear un parser que divida el HTML en "chunks" semánticos manteniendo la jerarquía del DOM.
- [ ] **Fase 3: Lógica de Inferencia**
    - [ ] Implementar la función de tokenización y paso por el modelo ONNX.
    - [ ] Implementar la lógica de filtrado basada en umbrales de relevancia semántica.
- [ ] **Fase 4: Optimización de Rendimiento**
    - [ ] Asegurar que el pipeline de inferencia sea asíncrono y no bloquee el hilo principal.
    - [ ] Documentar la compilación optimizada (`-C target-cpu=haswell`) para maximizar AVX2.

## 6. Criterios de Aceptación (QA)

1.  **Calidad:** El comando `./rust_scraper --url <url> --clean-ai` genera un Markdown limpio, libre de menús, footers y ruido estructural.
2.  **Rendimiento:** El tiempo de procesamiento por página no aumenta más de 100ms respecto al modo estándar.
3.  **Eficiencia:** El footprint de memoria total (incluyendo el modelo cargado) no excede los 150MB.
4.  **Resiliencia:** El modelo se descarga y verifica automáticamente; si la red falla, el sistema informa claramente sin entrar en pánico.
5.  **Testing:** 100% de cobertura en la nueva capa de infraestructura de IA.

---

## Notas para el desarrollador:

*   Recuerda implementar el `panic_hook` en la TUI si esta feature se activa junto con el modo interactivo.
*   Prioriza el uso de `mmap` para cargar el modelo ONNX desde el disco, esto reducirá drásticamente el tiempo de inicio en sistemas con HDD.
*   La limpieza determinista (eliminar `data:image/` y `<script>`) debe ocurrir ANTES de la inferencia para ahorrar ciclos de CPU.

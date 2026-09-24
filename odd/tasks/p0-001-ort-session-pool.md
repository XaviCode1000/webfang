# P0-001 — Remediación serialización sesión ORT (issue #1456)

**Issue:** #1456 `feat: diseño de remediación P0-001 (serialización sesión ORT tras Mutex)`
**Estado:** OPEN · `type:feature` · área AI / semantic cleaning
**Jerarquía:** CORRECTNESS > ROBUSTNESS > PREDICTABILITY > PERFORMANCE
**Metodología:** la del propio issue + comentario maintainer (paso 0 bloqueante primero).

## Diagnóstico confirmado en código

- `crates/webfang_ai/src/infrastructure_ai/inference_engine.rs` — `InferencePool`
  construye UNA `ort::Session` (`intra_threads(1)`) compartida como
  `Arc<Mutex<Session>>` entre `(num_cpus-1)` OS threads (`inference-worker-*`).
  `Session::run` exige `&mut self` (ort 2.0) → el Mutex serializa toda inferencia.
- `crates/webfang_ai/src/infrastructure_ai/semantic_cleaner_impl.rs` — `clean()`
  por página dispara `try_join_all(pool.infer())` por chunk (fan-out M chunks).
- `crates/webfang_core/src/cli/export_flow.rs` — `clean_all_pages()` dispara
  `join_all(cleaner.clean())` por página (fan-out N páginas).
- Fan-out total N×M colapsa en 1 lock. Baseline issue: speedup 1→8 = 1.02×,
  fase AI = 100% del tiempo restante. Trade-off memoria deliberado (#648, #1315:
  2.05 GiB 1 sesión vs 3.2 GiB extremo N).
- P2-001 (fan-out N×M retenido) y P2-002 (fan-in `join_all` head-of-line) quedan
  como `riesgo_hipotesis_no_demostrada`: el paso 0 decide si son downstream.

## Orden de ejecución (del comentario maintainer — no alterar)

1. **Paso 0 (bloqueante, ~medio día):** mock `InferenceEngine` con latencia fija
   por chunk, sin Mutex real. Mismo benchmark 1/2/4/8 páginas.
   - Speedup ~8× → P2-001/002 se archivan como downstream, NO se toca `export_flow.rs`.
   - Speedup ~1× → fan-out/fan-in es causa independiente, entra al backlog con
     su propio MEASURE (buffering fan-in: `join_all` → `FuturesUnordered` + backpressure).
2. **Spike batch dinámico (~1 día, solo tras paso 0):** verificar si el ONNX
   exportado tiene dim 0 dinámica. Si sí, evaluar micro-batcher como tercera
   opción (1 copia de pesos, paralelismo en matriz de batch) antes de
   comprometerse al pool.
3. **Pool de N sesiones tras feature flag** (solo si el mock confirma el pool
   como causa raíz): `SessionPool`, `intra_threads = total_cores / N`,
   `inter_threads(1)`, `GraphOptimizationLevel::Level3`, coordinación SIN
   segundo Mutex centralizado (`AtomicUsize % N` o semáforo por sesión).
   `export_flow.rs` sigue viendo un trait object `InferenceEngine` — cambio
   quirúrgico. `inter_op` en sesión única se mide pero con prior baja: no
   resuelve fan-out de requests independientes (grafo transformer ≈ secuencial).
4. **MEASURE obligatorio antes de fijar N:** barrido N ∈ {1,2,4,8,15} × páginas
   {1,2,4,8}; registrar tiempo_AI, speedup, pico RSS + snapshot correctitud
   (mismo corpus). Criterio: mínimo N con speedup(8) ≥ 6.0× y RSS dentro de
   presupuesto ops. N documentado con números, nunca por intuición.

## Restricciones duras

- Memoria es la restricción dura (RSS ≈ pesos×N + overhead arena×N).
- Nada de no-determinismo en el resultado de limpieza (sin batching dinámico
   que reordene/fusione chunks de forma no reproducible).
- Verificar aislamiento de estado por chunk si el modelo tuviera KV-cache
  (improbable en limpieza feedforward tipo BERT — verificar, no asumir).
- `run_chunk` reintentable; timeout 30s; backpressure real (no apilar 15
  workers en `acquire()` indefinidamente).
- Preguntas abiertas que bloquean calibrar N: N exacto de la medición #1315,
  tamaño de pesos aislado del working set, EP disponible (¿solo CPU?).

## Tareas

- [x] Exploración y confirmación del diagnóstico en código
- [x] Paso 0: mock + benchmark 1/2/4/8 → VEREDICTO (2026-09-16, rama
  `feat/1456-p0-001-mock`, sin commits): speedup 1→8 = 4.08× con mock 45ms
  sin Mutex (tabla 0.063s → 0.123s; coste marginal/página ≈8.6ms vs piso
  serial 63ms, 7.3× mejor). **P2-001/002 archivados como downstream de
  P0-001 — NO tocar `export_flow.rs`.** Gap 8×→4.08× atribuido a overhead
  CPU fijo (chunk/tokenize/score ≈18ms/página), no a serialización.
- [x] Spike dim 0 dinámica → VEREDICTO (2026-09-16): **MICRO-BATCHING VIABLE**
  pendiente smoke run. Ambos modelos (`97m` y `311m`) declaran
  `input_ids`/`attention_mask` como `['batch_size', 'sequence_length']`
  (simbólicos, opset 18) — verificado con parse ONNX directo del blob en
  caché HF local, sin descargas. `Session::run` acepta cualquier shape del
  grafo; nada lo prohíbe (nombres validados, shapes no).
- [x] Prototipo batch → VEREDICTO (2026-09-16): **BATCH VIABLE SÍ**.
  Paridad bit-idéntica (diff 0.0 en 4 filas, S={7,13,5,11}) con modelo 97m
  real; 153 lib tests verdes, clippy/fmt limpios. Coste ~lineal con
  `intra_threads(1)` — sin ganancia de throughput todavía (esperable:
  sesión monohilo no paraleliza, solo valida correctitud). El throughput
  (batch+intra>1 vs pool N) lo decide MEASURE.
- [x] Pool N sesiones + feature flag → HECHO (2026-09-16):
  `PooledInferenceEngine` (N sesiones `commit_from_file`,
  `intra=(cores/N).max(1)`, semáforos por slot + `AtomicUsize % N`, cero
  Mutex centralizado) + `EngineConfig::{Single,Pool}` + `build_engine` +
  `?Sized` en `SemanticCleanerImpl` (cero cambios de firmas, workspace verde).
  Rollback = `EngineConfig::Single`. 11 tests mock-backed verdes.
- [x] Barrido MEASURE + decisión N documentada + rollout → VEREDICTO
  (2026-09-24, rama `feat/1456-p0-001-measure`, release + modelos reales):
  **N=4, default flippeado a Pool.** Tablas y criterio (knee + RSS,
  tie-break hacia menos maquinaria) en `docs/p0-001-n-decision.md`.
  El criterio literal 6.0× era inalcanzable (techo medido 5.71×: una página
  ya satura la CPU con cualquier config; la ganancia es por-página
  15.28→2.81 s/pág). `batch` eliminado: pierde en wall (2.3× vs pool4) Y en
  RSS pico (2,535 vs 1,822 MiB). Paridad bit-idéntica pool2/4/8 vs single.
  Alcance del flip: CLI sí; MCP sigue `Single` hasta generalizar el seam de
  puertos (vault/Tier-2 degradan honestamente en CLI-Pool, hatch
  `WEBFANG_AI_ENGINE=single`).

## Seguimiento 2026-09-17 (rama `feat/1456-p0-001-mock`, sin PR, default sigue `Single`)

- **Celda Batch en MEASURE** (`p0_001_measure.rs`, `batch` harness-only: 1 sesión
  `intra_threads=16`, `run_batched_inference` por página, misma línea
  `P0_001_CELL`): implementada y verde en check/clippy; sin números todavía
  (el barrido release con modelos reales no se corrió aquí).
- **Curvas mock A/B/C** (`mock_inference_benchmark.rs`, REPS=3 mediana,
  `worker_threads=8` fijo): B = 4.13×, C = 7.68×, gap B−C = 16.6ms/página.
  Piso de assert: se MANTIENE 3.0 (fija libertad-de-serialización, no
  throughput); el viejo "~18ms/página" queda reemplazado por el gap medido.
- **P2-001/002**: archivados como downstream de P0-001 (C≈8× lo demuestra);
  `export_flow.rs` no se toca. Decisión N: PENDIENTE del barrido release.
- **Circuit breaker**: diferido por decisión explícita; re-apertura solo como
  gate cuando `Pool` se active vía `WEBFANG_AI_ENGINE` bajo tráfico sostenido.
- Detalle completo: `docs/p0-001-n-decision.md`.

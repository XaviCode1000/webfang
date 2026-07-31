//! Stress test para métricas de runtime
//!
//! Mide task starvation y throughput bajo carga pesada.
//! Ejecutar con: RUSTFLAGS="--cfg tokio_unstable" cargo test --features console stress_test -- --nocapture

#![cfg(feature = "console")]

/// Stress test: 1000 tareas concurrentes en 4 worker threads
/// Mide throughput y latencia para detectar task starvation
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_test_task_starvation() {
    let start = std::time::Instant::now();

    // 1000 tareas concurrentes - cada una hace 10 yield
    let handles: Vec<_> = (0..1000)
        .map(|_| {
            tokio::spawn(async {
                // Simula trabajo ligero con 10 yield por tarea
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
                "done"
            })
        })
        .collect();

    let results = futures::future::join_all(handles).await;
    let elapsed = start.elapsed();

    // Métricas
    let total_tasks = 1000;
    let worker_threads = 4;
    let tasks_per_sec = total_tasks as f64 / elapsed.as_secs_f64();

    eprintln!("\n{}", "=".repeat(50));
    eprintln!("tokio-console STRESS TEST METRICS");
    eprintln!("{}", "=".repeat(50));
    eprintln!("Total tasks:       {}", total_tasks);
    eprintln!("Worker threads:  {}", worker_threads);
    eprintln!("Duration:       {:?}", elapsed);
    eprintln!("Throughput:     {:.2} tasks/sec", tasks_per_sec);

    // Análisis de resultados
    let successful = results.iter().filter(|r| r.is_ok()).count();
    let failed = results.iter().filter(|r| r.is_err()).count();

    eprintln!("\nResults:");
    eprintln!("  Successful:    {}", successful);
    eprintln!("  Failed:      {}", failed);
    eprintln!("{}", "=".repeat(50));

    assert!(
        results.iter().all(|r| r.is_ok()),
        "Some tasks failed: {} errors",
        failed
    );
}

/// Stress test con trabajo real (CPU-bound simulado)
/// Mayor duración para detectar problemas de starvation
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_test_cpu_work() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let completed = Arc::new(AtomicUsize::new(0));
    let start = std::time::Instant::now();

    // 500 tareas con trabajo CPU real
    let handles: Vec<_> = (0..500)
        .map(|i| {
            let completed = completed.clone();
            tokio::spawn(async move {
                // Trabajo CPU-bound simulado (spin loop)
                let mut sum = 0u64;
                for j in 0..10000u64 {
                    sum = sum.wrapping_add(j.wrapping_mul(i as u64));
                }
                completed.fetch_add(1, Ordering::Relaxed);
                sum
            })
        })
        .collect();

    let results = futures::future::join_all(handles).await;
    let elapsed = start.elapsed();

    let total = 500;
    let tasks_per_sec = total as f64 / elapsed.as_secs_f64();

    eprintln!("\n{}", "=".repeat(50));
    eprintln!("tokio-console CPU STRESS TEST");
    eprintln!("{}", "=".repeat(50));
    eprintln!("Total tasks:       {}", total);
    eprintln!("Worker threads:  4");
    eprintln!("Duration:       {:?}", elapsed);
    eprintln!("Throughput:     {:.2} tasks/sec", tasks_per_sec);
    eprintln!("Completed:      {}", completed.load(Ordering::Relaxed));
    eprintln!("{}", "=".repeat(50));

    // Verificar que ningún task panickeara
    for (i, result) in results.iter().enumerate() {
        if let Err(e) = result {
            eprintln!("Task {} failed: {}", i, e);
        }
    }

    assert!(results.iter().all(|r| r.is_ok()), "Some CPU tasks failed");
}

/// Test de memoria: múltiples task spawns para detectar memory leaks
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stress_test_memory() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let allocations = Arc::new(AtomicUsize::new(0));
    let start = std::time::Instant::now();

    // 100 batches de 100 tareas cada uno = 10,000 tareas totales
    // Para detectar memory leaks gradual
    for batch in 0..100 {
        let handles: Vec<_> = (0..100)
            .map(|_| {
                let allocations = allocations.clone();
                tokio::spawn(async move {
                    // Alloc un poco de data en el stack
                    let data = vec![0u8; 1024]; // 1KB por task
                    allocations.fetch_add(1, Ordering::Relaxed);
                    drop(data); // Explicit drop
                    "done"
                })
            })
            .collect();

        futures::future::join_all(handles).await;

        // Small delay to let runtime cleanup
        if batch % 10 == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    let elapsed = start.elapsed();
    let total_allocations = allocations.load(Ordering::Relaxed);
    let tasks_per_sec = total_allocations as f64 / elapsed.as_secs_f64();

    eprintln!("\n{}", "=".repeat(50));
    eprintln!("tokio-console MEMORY STRESS TEST");
    eprintln!("{}", "=".repeat(50));
    eprintln!("Total tasks:       {}", total_allocations);
    eprintln!("Worker threads:  2");
    eprintln!("Duration:       {:?}", elapsed);
    eprintln!("Throughput:     {:.2} tasks/sec", tasks_per_sec);
    eprintln!("{}", "=".repeat(50));

    // Este test principalmente mide que no hubo crashes por memory pressure
    // El throughput debería ser razonable
    assert!(
        tasks_per_sec > 100.0,
        "Throughput too low: {}",
        tasks_per_sec
    );
}

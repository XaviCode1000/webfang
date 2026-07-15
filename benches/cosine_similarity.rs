use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

// Only compile with ai feature
#[cfg(feature = "ai")]
use webfang::infrastructure::ai::embedding_ops::cosine_similarity;

#[cfg(feature = "ai")]
fn bench_cosine_similarity(c: &mut Criterion) {
    let dims = 384;
    // Deterministic data — no rand crate needed
    let a: Vec<f32> = (0..dims).map(|i| (i as f32 * 0.01).sin()).collect();
    let b: Vec<f32> = (0..dims).map(|i| (i as f32 * 0.01 + 1.0).cos()).collect();

    let mut group = c.benchmark_group("cosine_similarity");
    group.throughput(Throughput::Elements(1));
    group.bench_function("simd_384d", |bencher| {
        bencher.iter(|| {
            let result = cosine_similarity(black_box(&a), black_box(&b));
            assert!((-1.0..=1.0).contains(&result));
            black_box(result)
        })
    });
    group.finish();
}

#[cfg(feature = "ai")]
criterion_group!(benches, bench_cosine_similarity);

#[cfg(feature = "ai")]
criterion_main!(benches);

// Placeholder when ai feature is disabled
#[cfg(not(feature = "ai"))]
fn main() {
    println!("Benchmarks require --features ai");
}

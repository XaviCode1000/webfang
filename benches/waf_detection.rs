use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use webfang_core::domain::waf::InspectionContext;
use webfang_core::infrastructure::http::waf_engine::WafInspector;

fn bench_waf_inspect(c: &mut Criterion) {
    // Generate ~500KB HTML body with some WAF signatures embedded
    let body = generate_html_body();
    let ctx = InspectionContext::default();

    let mut group = c.benchmark_group("waf_detection");
    group.throughput(Throughput::Bytes(body.len() as u64));
    group.bench_function("inspect_500kb", |b| {
        b.iter(|| {
            let verdict = WafInspector::inspect(black_box(&body), &ctx);
            black_box(verdict)
        })
    });
    group.finish();
}

fn generate_html_body() -> String {
    let template = r#"<html><head><title>Page</title></head><body>
<div class="content">
<h1>Article Title</h1>
<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris.</p>
<div class="article-content">
<p>Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.</p>
</div>
</div>
</body></html>"#;

    // Repeat the clean template until ~500KB. No WAF signatures are embedded:
    // `inspect` should scan the whole body and return a clean verdict, which
    // benchmarks the full clean-scan path (the common case, and the worst case
    // for scanning since nothing short-circuits on an early detection).
    let mut body = String::new();
    while body.len() < 500_000 {
        body.push_str(template);
    }
    body
}

criterion_group!(benches, bench_waf_inspect);
criterion_main!(benches);

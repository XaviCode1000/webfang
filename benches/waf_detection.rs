use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use webfang::infrastructure::http::waf_engine::WafInspector;
use wreq::header::HeaderMap;

fn bench_waf_verify_integrity(c: &mut Criterion) {
    // Generate ~500KB HTML body with some WAF signatures embedded
    let body = generate_html_body();

    let mut group = c.benchmark_group("waf_detection");
    group.throughput(Throughput::Bytes(body.len() as u64));
    group.bench_function("verify_integrity_500kb", |b| {
        b.iter(|| {
            let result =
                WafInspector::verify_integrity(black_box(&HeaderMap::new()), black_box(&body));
            assert!(result.is_ok());
            black_box(result)
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

    // Include some WAF signatures for realistic detection
    let signatures = [
        "cf-turnstile",
        "Just a moment...",
        "g-recaptcha",
        "datadome",
        "challenge-platform",
    ];

    let mut body = String::new();
    // Repeat until ~500KB
    while body.len() < 500_000 {
        body.push_str(template);
        // Insert a signature every 10 templates for realism
        if body.len() % (template.len() * 10) < template.len() {
            if let Some(sig) = signatures.get((body.len() / template.len()) % signatures.len()) {
                body.push_str(&format!("<div class=\"waf-marker\">{}</div>", sig));
            }
        }
    }
    body
}

criterion_group!(benches, bench_waf_verify_integrity);
criterion_main!(benches);

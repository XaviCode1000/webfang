use criterion::{black_box, criterion_group, criterion_main, Criterion};
use url::Url;

fn bench_url_parse(c: &mut Criterion) {
    let urls = [
        "https://example.com/path?q=1&b=2#frag",
        "http://localhost:3000/api/v1/users",
        "https://sub.domain.co.uk/very/deep/nested/path/that/goes/on",
    ];

    c.bench_function("url_parse_single", |b| {
        b.iter(|| {
            for url in &urls {
                let _ = black_box(Url::parse(black_box(url)));
            }
        })
    });

    let batch: Vec<String> = (0..1000)
        .map(|i| format!("https://example.com/page/{i}"))
        .collect();
    c.bench_function("url_parse_1000", |b| {
        b.iter(|| {
            for url in &batch {
                let _ = black_box(Url::parse(black_box(url.as_str())));
            }
        })
    });
}

criterion_group!(benches, bench_url_parse);
criterion_main!(benches);

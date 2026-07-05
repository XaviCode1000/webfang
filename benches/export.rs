use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use rust_scraper::domain::entities::ScrapedContent;
use rust_scraper::domain::value_objects::ValidUrl;

fn sample_chunks(count: usize) -> Vec<ScrapedContent> {
    (0..count)
        .map(|i| ScrapedContent {
            title: format!("Article Title {i}"),
            content: format!(
                "This is the content of article number {i}. \
                 It contains multiple sentences to simulate realistic text length. \
                 The quick brown fox jumps over the lazy dog. \
                 Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                 Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
                 Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris. \
                 Number: {i}."
            ),
            url: ValidUrl::new(
                url::Url::parse(&format!("https://example.com/article/{i}")).unwrap(),
            ),
            excerpt: Some(format!("Excerpt for article {i}")),
            author: Some("Jane Developer".to_string()),
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
        })
        .collect()
}

fn bench_export(c: &mut Criterion) {
    let chunks_100: Vec<_> = sample_chunks(100)
        .into_iter()
        .map(|sc| {
            let chunk: rust_scraper::domain::DocumentChunk = sc.into();
            chunk
        })
        .collect();
    let chunks_1000: Vec<_> = sample_chunks(1000)
        .into_iter()
        .map(|sc| {
            let chunk: rust_scraper::domain::DocumentChunk = sc.into();
            chunk
        })
        .collect();

    let mut group = c.benchmark_group("export_jsonl");
    group.throughput(Throughput::Elements(100));
    group.bench_function("serialize_100_chunks", |b| {
        b.iter(|| {
            let mut output = String::new();
            for chunk in black_box(&chunks_100) {
                let line = serde_json::to_string(black_box(chunk)).unwrap();
                output.push_str(&line);
                output.push('\n');
            }
            black_box(output);
        })
    });

    group.throughput(Throughput::Elements(1000));
    group.bench_function("serialize_1000_chunks", |b| {
        b.iter(|| {
            let mut output = String::new();
            for chunk in black_box(&chunks_1000) {
                let line = serde_json::to_string(black_box(chunk)).unwrap();
                output.push_str(&line);
                output.push('\n');
            }
            black_box(output);
        })
    });
    group.finish();
}

criterion_group!(benches, bench_export);
criterion_main!(benches);

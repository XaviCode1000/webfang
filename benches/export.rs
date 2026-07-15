use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use webfang::domain::entities::ScrapedContent;
use webfang::domain::value_objects::ValidUrl;

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
            let chunk: webfang::domain::DocumentChunk = sc.into();
            chunk
        })
        .collect();
    let chunks_1000: Vec<_> = sample_chunks(1000)
        .into_iter()
        .map(|sc| {
            let chunk: webfang::domain::DocumentChunk = sc.into();
            chunk
        })
        .collect();

    // JSONL export
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
            assert!(!output.is_empty());
            black_box(output)
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
            assert!(!output.is_empty());
            black_box(output)
        })
    });
    group.finish();

    // Vector format export (with metadata wrapper)
    let mut vec_group = c.benchmark_group("export_vector");
    vec_group.throughput(Throughput::Elements(100));
    vec_group.bench_function("serialize_vector_100", |b| {
        b.iter(|| {
            let wrapper = serde_json::json!({
                "format": "vector",
                "version": "1.0",
                "chunks": black_box(&chunks_100).iter().map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "url": c.url,
                        "title": c.title,
                        "content": c.content,
                        "metadata": c.metadata,
                        "timestamp": c.timestamp.to_rfc3339(),
                    })
                }).collect::<Vec<_>>(),
            });
            let result = serde_json::to_string(&wrapper).unwrap();
            assert!(!result.is_empty());
            black_box(result)
        })
    });

    vec_group.throughput(Throughput::Elements(1000));
    vec_group.bench_function("serialize_vector_1000", |b| {
        b.iter(|| {
            let wrapper = serde_json::json!({
                "format": "vector",
                "version": "1.0",
                "chunks": black_box(&chunks_1000).iter().map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "url": c.url,
                        "title": c.title,
                        "content": c.content,
                        "metadata": c.metadata,
                        "timestamp": c.timestamp.to_rfc3339(),
                    })
                }).collect::<Vec<_>>(),
            });
            let result = serde_json::to_string(&wrapper).unwrap();
            assert!(!result.is_empty());
            black_box(result)
        })
    });
    vec_group.finish();

    // Deserialization benchmark
    let jsonl_data: String = chunks_1000
        .iter()
        .map(|c| serde_json::to_string(c).unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    let mut deser_group = c.benchmark_group("export_deserialize");
    deser_group.throughput(Throughput::Bytes(jsonl_data.len() as u64));
    deser_group.bench_function("deserialize_1000_jsonl", |b| {
        b.iter(|| {
            let mut count = 0;
            for line in black_box(&jsonl_data).lines() {
                let _: webfang::domain::DocumentChunk = serde_json::from_str(line).unwrap();
                count += 1;
            }
            assert_eq!(count, 1000);
            black_box(count)
        })
    });
    deser_group.finish();
}

criterion_group!(benches, bench_export);
criterion_main!(benches);

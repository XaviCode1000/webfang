use criterion::{black_box, criterion_group, criterion_main, Criterion};
use webfang::validate_and_parse_url;
use url::Url;

fn bench_url_parse(c: &mut Criterion) {
    // Standard URLs
    let urls = [
        "https://example.com/path?q=1&b=2#frag",
        "http://localhost:3000/api/v1/users",
        "https://sub.domain.co.uk/very/deep/nested/path/that/goes/on",
    ];

    c.bench_function("url_parse_single", |b| {
        b.iter(|| {
            for url in &urls {
                let result = Url::parse(black_box(url));
                assert!(result.is_ok());
            }
        })
    });

    // Batch: 1000 URLs
    let batch: Vec<String> = (0..1000)
        .map(|i| format!("https://example.com/page/{i}"))
        .collect();
    c.bench_function("url_parse_1000", |b| {
        b.iter(|| {
            let mut count = 0;
            for url in &batch {
                let result = Url::parse(black_box(url.as_str()));
                assert!(result.is_ok());
                count += 1;
            }
            assert_eq!(count, 1000);
        })
    });

    // Special characters
    let special_urls = [
        "https://example.com/path%20with%20spaces?key=value&foo=bar+baz",
        "https://example.com/search?q=rust+programming&lang=en&sort=date",
        "https://example.com/redirect?url=https%3A%2F%2Fother.com%2Fpage",
        "https://example.com/path;params?query=value#section-1",
        "https://user:pass@example.com:8080/path",
    ];

    c.bench_function("url_parse_special_chars", |b| {
        b.iter(|| {
            for url in &special_urls {
                let result = Url::parse(black_box(url));
                assert!(result.is_ok());
            }
        })
    });

    // Unicode URLs (IDN / punycode)
    let unicode_urls = [
        "https://例え.jp/path",
        "https://münchen.de/straße",
        "https://пример.рф/page",
        "https://café.com/menu",
        "https://例子.测试/路径",
    ];

    c.bench_function("url_parse_unicode", |b| {
        b.iter(|| {
            for url in &unicode_urls {
                let result = Url::parse(black_box(url));
                assert!(result.is_ok());
            }
        })
    });

    // validate_and_parse_url (with scheme/host validation)
    c.bench_function("validate_and_parse_url_batch", |b| {
        b.iter(|| {
            for url in &batch {
                let result = validate_and_parse_url(black_box(url.as_str()));
                assert!(result.is_ok());
            }
        })
    });
}

criterion_group!(benches, bench_url_parse);
criterion_main!(benches);

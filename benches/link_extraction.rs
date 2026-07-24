//! Benchmarks for link extraction and URL normalization
//!
//! These are hot-path functions called per-page during crawling.
//!
//! # Rules Applied
//!
//! - `test-criterion-bench`: Use criterion for benchmarking
//! - `perf-black-box-bench`: Use black_box to prevent compiler optimizations
//! - `own-slice-over-accept`: Accept &str not &String

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use webfang::infrastructure::crawler::{extract_links, normalize_url};

/// Generate HTML with n links for benchmarking
fn generate_html_with_links(n: usize) -> String {
    let mut html = String::with_capacity(50 + n * 40);
    html.push_str("<html><body>");
    for i in 0..n {
        html.push_str(&format!("<a href='/page{i}'>Link {i}</a>"));
    }
    html.push_str("</body></html>");
    html
}

// ============================================================================
// extract_links benchmarks
// ============================================================================

fn bench_extract_links_small(c: &mut Criterion) {
    let html = "<html><body><a href='/page1'>Link 1</a><a href='/page2'>Link 2</a></body></html>";
    c.bench_function("extract_links_2_links", |b| {
        b.iter(|| {
            let result = extract_links(black_box(html), black_box("https://example.com")).unwrap();
            assert_eq!(result.len(), 2);
            black_box(result)
        })
    });
}

fn bench_extract_links_medium(c: &mut Criterion) {
    let html = generate_html_with_links(50);
    c.bench_function("extract_links_50_links", |b| {
        b.iter(|| {
            let result = extract_links(black_box(&html), black_box("https://example.com")).unwrap();
            assert_eq!(result.len(), 50);
            black_box(result)
        })
    });
}

fn bench_extract_links_large(c: &mut Criterion) {
    let html = generate_html_with_links(200);
    c.bench_function("extract_links_200_links", |b| {
        b.iter(|| {
            let result = extract_links(black_box(&html), black_box("https://example.com")).unwrap();
            assert_eq!(result.len(), 200);
            black_box(result)
        })
    });
}

// ============================================================================
// normalize_url benchmarks
// ============================================================================

fn bench_normalize_url_simple(c: &mut Criterion) {
    c.bench_function("normalize_url_simple", |b| {
        b.iter(|| {
            let result = normalize_url(black_box("https://example.com/page"), true);
            assert!(!result.is_empty());
            black_box(result)
        })
    });
}

fn bench_normalize_url_relative(c: &mut Criterion) {
    c.bench_function("normalize_url_relative", |b| {
        b.iter(|| {
            let result = normalize_url(black_box("/page?id=1&sort=name"), true);
            assert!(!result.is_empty());
            black_box(result)
        })
    });
}

fn bench_normalize_url_with_fragment(c: &mut Criterion) {
    c.bench_function("normalize_url_with_fragment", |b| {
        b.iter(|| {
            let result = normalize_url(black_box("https://example.com/page#section"), true);
            assert!(!result.contains('#'));
            black_box(result)
        })
    });
}

criterion_group!(
    benches,
    bench_extract_links_small,
    bench_extract_links_medium,
    bench_extract_links_large,
    bench_normalize_url_simple,
    bench_normalize_url_relative,
    bench_normalize_url_with_fragment
);
criterion_main!(benches);

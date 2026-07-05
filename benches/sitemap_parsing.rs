use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rust_scraper::application::crawler::parse_sitemap;
use url::Url;

fn generate_sitemap_xml(url_count: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#,
    );
    for i in 0..url_count {
        xml.push_str(&format!(
            r#"
  <url>
    <loc>https://example.com/page/{i}</loc>
    <lastmod>2025-01-15</lastmod>
    <changefreq>weekly</changefreq>
    <priority>0.8</priority>
  </url>"#
        ));
    }
    xml.push_str("\n</urlset>");
    xml
}

fn bench_sitemap_parsing(c: &mut Criterion) {
    let base_url = Url::parse("https://example.com/sitemap.xml").unwrap();

    let xml_10 = generate_sitemap_xml(10);
    let xml_100 = generate_sitemap_xml(100);
    let xml_1000 = generate_sitemap_xml(1000);

    let mut group = c.benchmark_group("sitemap_parsing");
    group.bench_function("parse_10_urls", |b| {
        b.iter(|| black_box(parse_sitemap(black_box(&xml_10), black_box(&base_url))))
    });
    group.bench_function("parse_100_urls", |b| {
        b.iter(|| black_box(parse_sitemap(black_box(&xml_100), black_box(&base_url))))
    });
    group.bench_function("parse_1000_urls", |b| {
        b.iter(|| black_box(parse_sitemap(black_box(&xml_1000), black_box(&base_url))))
    });
    group.finish();
}

criterion_group!(benches, bench_sitemap_parsing);
criterion_main!(benches);

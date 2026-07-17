use criterion::{black_box, criterion_group, criterion_main, Criterion};
use webfang::application::crawler::parse_sitemap;
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

fn generate_sitemap_index(sitemap_count: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#,
    );
    for i in 0..sitemap_count {
        xml.push_str(&format!(
            r#"
  <sitemap>
    <loc>https://example.com/sitemaps/sitemap-{i}.xml</loc>
    <lastmod>2025-01-15</lastmod>
  </sitemap>"#
        ));
    }
    xml.push_str("\n</sitemapindex>");
    xml
}

fn bench_sitemap_parsing(c: &mut Criterion) {
    let base_url = Url::parse("https://example.com/sitemap.xml").unwrap();

    let xml_10 = generate_sitemap_xml(10);
    let xml_100 = generate_sitemap_xml(100);
    let xml_1000 = generate_sitemap_xml(1000);

    let mut group = c.benchmark_group("sitemap_parsing");
    group.bench_function("parse_10_urls", |b| {
        b.iter(|| {
            let result = parse_sitemap(black_box(&xml_10), black_box(&base_url));
            assert!(result.is_ok());
            black_box(result)
        })
    });
    group.bench_function("parse_100_urls", |b| {
        b.iter(|| {
            let result = parse_sitemap(black_box(&xml_100), black_box(&base_url));
            assert!(result.is_ok());
            black_box(result)
        })
    });
    group.bench_function("parse_1000_urls", |b| {
        b.iter(|| {
            let result = parse_sitemap(black_box(&xml_1000), black_box(&base_url));
            assert!(result.is_ok());
            black_box(result)
        })
    });
    group.finish();

    // Sitemap index parsing (sitemapindex XML)
    let sitemap_index_10 = generate_sitemap_index(10);
    let sitemap_index_100 = generate_sitemap_index(100);

    let mut index_group = c.benchmark_group("sitemap_index_parsing");
    index_group.bench_function("parse_index_10_sitemaps", |b| {
        b.iter(|| {
            let result = parse_sitemap(black_box(&sitemap_index_10), black_box(&base_url));
            assert!(result.is_ok());
            black_box(result)
        })
    });
    index_group.bench_function("parse_index_100_sitemaps", |b| {
        b.iter(|| {
            let result = parse_sitemap(black_box(&sitemap_index_100), black_box(&base_url));
            assert!(result.is_ok());
            black_box(result)
        })
    });
    index_group.finish();
}

criterion_group!(benches, bench_sitemap_parsing);
criterion_main!(benches);

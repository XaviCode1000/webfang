use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use webfang::infrastructure::converter::html_cleaner::clean_html;
use webfang::infrastructure::scraper::readability;

fn realistic_html() -> String {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
    <title>Python Data Science Handbook</title>
    <meta name="description" content="A comprehensive guide to Python data science">
    <style>body { font-family: sans-serif; }</style>
    <script>var analytics = { track: function() {} };</script>
</head>
<body>
<nav class="global-nav">
    <a href="/">Home</a>
    <a href="/docs">Documentation</a>
    <a href="/blog">Blog</a>
</nav>
<div class="sidebar">
    <ul>
        <li><a href="#ch1">Chapter 1</a></li>
        <li><a href="#ch2">Chapter 2</a></li>
    </ul>
</div>
<header>
    <h1 class="site-title">Python Data Science Handbook</h1>
</header>
<main>
<article>
<h1 id="ch1">Chapter 1: IPython Beyond Normal Python</h1>
<p>The IPython shell and Jupyter Notebook provide a productive environment for
exploring and working with Python code. IPython goes beyond the traditional
Python shell with features designed to make interactive computing more efficient.</p>
<h2>Tab Completion</h2>
<p>One of the most useful features of IPython is tab completion. When you type
a character or two and press the Tab key, IPython will show you all the
matching attributes, methods, and variables available in the current scope.</p>
<pre><code class="language-python">import numpy as np
arr = np.array([1, 2, 3, 4, 5])
print(arr.mean())  # Tab completion works here!</code></pre>
<h2>Magics</h2>
<p>IPython's magic commands are not Python syntax; they are enhancements to the
Python interpreter that enable convenient features that would be difficult
or impossible in standard Python.</p>
<ul>
<li><code>%timeit</code> - Time the execution of a statement</li>
<li><code>%run</code> - Run a Python script</li>
<li><code>%who</code> - List all variables</li>
</ul>
<h1 id="ch2">Chapter 2: NumPy</h1>
<p>NumPy provides the basic building blocks for almost all scientific computing
in Python. Its ndarray object provides fast, flexible container for large
datasets.</p>
<h3>Array Creation</h3>
<ol>
<li>From Python lists: <code>np.array([1, 2, 3])</code></li>
<li>From zeros: <code>np.zeros(10)</code></li>
<li>From ranges: <code>np.arange(0, 10, 1)</code></li>
</ol>
<blockquote><p>NumPy arrays are the fundamental data structure of the entire scientific
Python ecosystem. Everything else is built on top of them.</p></blockquote>
<table>
<thead>
<tr><th>Function</th><th>Description</th></tr>
</thead>
<tbody>
<tr><td><code>np.array()</code></td><td>Create array from list</td></tr>
<tr><td><code>np.zeros()</code></td><td>Create array of zeros</td></tr>
<tr><td><code>np.ones()</code></td><td>Create array of ones</td></tr>
<tr><td><code>np.arange()</code></td><td>Create array with range</td></tr>
</tbody>
</table>
<p>This chapter covers the essential NumPy operations that every data scientist
should know, including indexing, slicing, reshaping, and broadcasting.</p>
</article>
</main>
<aside class="right-sidebar">
    <h3>Table of Contents</h3>
    <ul><li>Chapter 1</li><li>Chapter 2</li></ul>
</aside>
<footer>
    <p>Copyright 2025. All rights reserved.</p>
</footer>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
    <circle cx="50" cy="50" r="40" />
</svg>
<noscript>Please enable JavaScript</noscript>
</body>
</html>"##
        .to_string()
}

fn complex_layout_html() -> String {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
    <title>Technical Blog Post with Complex Layout</title>
    <style>.content { max-width: 800px; } .sidebar { width: 250px; }</style>
</head>
<body>
<div class="site-header">
    <nav class="main-nav">
        <a href="/">Home</a>
        <a href="/blog">Blog</a>
        <a href="/about">About</a>
    </nav>
    <div class="search-bar">
        <input type="text" placeholder="Search...">
        <button>Search</button>
    </div>
</div>
<div class="breadcrumb">
    <a href="/">Home</a> &gt; <a href="/blog">Blog</a> &gt; <span>Post</span>
</div>
<div class="layout">
    <aside class="left-sidebar">
        <div class="table-of-contents">
            <h3>Contents</h3>
            <ul>
                <li><a href="#intro">Introduction</a></li>
                <li><a href="#setup">Setup</a></li>
                <li><a href="#usage">Usage</a></li>
                <li><a href="#conclusion">Conclusion</a></li>
            </ul>
        </div>
        <div class="related-posts">
            <h3>Related</h3>
            <ul>
                <li><a href="/post/1">Related Post 1</a></li>
                <li><a href="/post/2">Related Post 2</a></li>
            </ul>
        </div>
    </aside>
    <article class="main-content">
        <header>
            <h1 id="intro">Building a Web Scraper in Rust</h1>
            <div class="meta">
                <span class="author">By John Developer</span>
                <span class="date">March 15, 2025</span>
                <span class="reading-time">12 min read</span>
            </div>
            <div class="tags">
                <span class="tag">rust</span>
                <span class="tag">web-scraping</span>
                <span class="tag">tutorial</span>
            </div>
        </header>
        <div class="content">
            <p>Web scraping is a powerful technique for extracting data from websites.
            In this tutorial, we will build a production-ready web scraper in Rust,
            leveraging its safety guarantees and performance characteristics.</p>

            <h2 id="setup">Project Setup</h2>
            <p>First, let us set up our Rust project with the necessary dependencies.
            We will use <code>reqwest</code> for HTTP requests and <code>scraper</code>
            for HTML parsing.</p>

            <pre><code>[dependencies]
reqwest = "0.11"
scraper = "0.13"
thiserror = "1"</code></pre>

            <h2 id="usage">Implementation</h2>
            <p>The core of our scraper consists of three main components: the HTTP client,
            the HTML parser, and the data extractor.</p>

            <h3>HTML Parser</h3>
            <p>The parser extracts structured data from the HTML content using CSS selectors.</p>

            <ol>
                <li>Parse the HTML document</li>
                <li>Apply CSS selectors to find target elements</li>
                <li>Extract text content and attributes</li>
                <li>Handle relative URLs</li>
            </ol>

            <h2 id="conclusion">Conclusion</h2>
            <p>Building a web scraper in Rust provides excellent performance and safety.</p>

            <blockquote>
                <p>Rust ownership model is particularly beneficial for web scrapers.</p>
            </blockquote>

            <table>
                <thead>
                    <tr><th>Feature</th><th>Rust</th><th>Python</th></tr>
                </thead>
                <tbody>
                    <tr><td>Memory Safety</td><td>Compile-time</td><td>Runtime</td></tr>
                    <tr><td>Performance</td><td>Native</td><td>Interpreted</td></tr>
                    <tr><td>Error Handling</td><td>Result type</td><td>Exceptions</td></tr>
                </tbody>
            </table>
        </div>
    </article>
</div>
<div class="comments-section">
    <h3>Comments (5)</h3>
    <div class="comment">
        <span class="commenter">Alice</span>
        <p>Great tutorial! Very helpful.</p>
    </div>
</div>
<footer>
    <p>Copyright 2025. All rights reserved.</p>
    <div class="social-links">
        <a href="https://twitter.com">Twitter</a>
        <a href="https://github.com">GitHub</a>
    </div>
</footer>
</body>
</html>"##
        .to_string()
}

fn bench_readability(c: &mut Criterion) {
    let html = realistic_html();
    let size = html.len();

    // HTML cleaning (boilerplate removal)
    let mut clean_group = c.benchmark_group("readability");
    clean_group.throughput(Throughput::Bytes(size as u64));
    clean_group.bench_function("clean_html", |b| {
        b.iter(|| {
            let result = clean_html(black_box(&html));
            assert!(!result.is_empty());
            black_box(result)
        })
    });
    clean_group.finish();

    // Readability extraction (legible algorithm)
    let mut extract_group = c.benchmark_group("readability_extract");
    extract_group.throughput(Throughput::Bytes(size as u64));
    extract_group.bench_function("extract_content", |b| {
        b.iter(|| {
            let result = readability::parse(
                black_box(&html),
                black_box(Some("https://example.com/blog/python-handbook")),
            );
            assert!(result.is_ok());
            black_box(result)
        })
    });
    extract_group.finish();

    // Complex layout extraction
    let complex_html = complex_layout_html();
    let complex_size = complex_html.len();

    let mut complex_group = c.benchmark_group("readability_complex");
    complex_group.throughput(Throughput::Bytes(complex_size as u64));
    complex_group.bench_function("extract_complex_layout", |b| {
        b.iter(|| {
            let result = readability::parse(
                black_box(&complex_html),
                black_box(Some("https://blog.example.com/webfang")),
            );
            assert!(result.is_ok());
            black_box(result)
        })
    });
    complex_group.finish();
}

criterion_group!(benches, bench_readability);
criterion_main!(benches);

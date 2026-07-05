use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use rust_scraper::infrastructure::converter::html_cleaner::clean_html;

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

fn bench_html_cleaning(c: &mut Criterion) {
    let html = realistic_html();
    let size = html.len();

    let mut group = c.benchmark_group("readability");
    group.throughput(Throughput::Bytes(size as u64));
    group.bench_function("clean_html", |b| {
        b.iter(|| black_box(clean_html(black_box(&html))))
    });
    group.finish();
}

criterion_group!(benches, bench_html_cleaning);
criterion_main!(benches);

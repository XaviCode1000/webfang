use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use rust_scraper::infrastructure::converter::html_to_markdown::convert_to_markdown;

fn realistic_html() -> String {
    r#"<!DOCTYPE html>
<html lang="en">
<head><title>Understanding Rust Ownership</title></head>
<body>
<nav><a href="/">Home</a> | <a href="/blog">Blog</a></nav>
<article>
<h1>Understanding Rust Ownership</h1>
<p class="meta">Published on January 15, 2025 by Jane Developer</p>
<p>Rust's ownership system is its most unique feature. It enables memory safety
guarantees without a garbage collector, and without runtime overhead.</p>
<h2>The Stack and the Heap</h2>
<p>The stack is used for fixed-size data. The heap is used for data whose size
may change at runtime. When you push data to the stack, the data is copied.
When you add data to the heap, you get a pointer.</p>
<pre><code>fn main() {
    let s1 = String::from("hello");
    let s2 = s1.clone();
    println!("{s1} {s2}");
}</code></pre>
<h2>Ownership Rules</h2>
<ul>
<li>Each value has exactly one owner</li>
<li>When the owner goes out of scope, the value is dropped</li>
<li>You can have either one mutable reference OR any number of immutable references</li>
</ul>
<h3>Move Semantics</h3>
<p>When you assign one variable to another, the original variable is no longer
available. This is called a <em>move</em>.</p>
<blockquote><p>Ownership is Rust's most distinctive feature, and it enables Rust to make
memory safety guarantees without garbage collection.</p></blockquote>
<h2>References and Borrowing</h2>
<p>References let you refer to some value without taking ownership. A reference
is like a pointer in that it's an address we can follow to access the data
stored at that address.</p>
<ol>
<li>Create a reference with <code>&amp;</code></li>
<li>The reference borrows the value</li>
<li>The original value remains valid</li>
</ol>
<table>
<tr><th>Type</th><th>Description</th></tr>
<tr><td><code>&amp;T</code></td><td>Immutable reference</td></tr>
<tr><td><code>&amp;mut T</code></td><td>Mutable reference</td></tr>
</table>
<h2>Lifetimes</h2>
<p>Lifetimes are the scope for which that reference is valid. Most of the time,
lifetimes are implicit and can be elided.</p>
<pre><code>fn longest&lt;'a&gt;(x: &amp;'a str, y: &amp;'a str) -&gt; &amp;'a str {
    if x.len() &gt; y.len() { x } else { y }
}</code></pre>
<p>In summary, ownership provides a set of rules that the compiler checks at
compile time. These rules don't slow down your program while it's running.</p>
</article>
<aside>Related articles</aside>
<footer>Copyright 2025</footer>
<script>console.log("tracking");</script>
</body>
</html>"#
        .to_string()
}

fn bench_html_conversion(c: &mut Criterion) {
    let html = realistic_html();
    let size = html.len();

    let mut group = c.benchmark_group("html_conversion");
    group.throughput(Throughput::Bytes(size as u64));
    group.bench_function("convert_to_markdown", |b| {
        b.iter(|| black_box(convert_to_markdown(black_box(&html))))
    });
    group.finish();
}

criterion_group!(benches, bench_html_conversion);
criterion_main!(benches);

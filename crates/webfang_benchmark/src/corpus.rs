//! Tier A simulated-WAF corpus (FR-1).
//!
//! A deterministic, fully local wiremock server: static HTML pages, SPA-shell
//! fixtures, and a WAF-guarded URL whose atomic-counter sequence answers
//! 403 → 429 (`Retry-After`) → 200 on repeated requests from a fresh serve.
//! Identical runs see identical responses, which is what makes downstream
//! counts reproducible (design §7 Block A). The port is ephemeral and never
//! enters compared output.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::error::Result;

/// Minimal article body with enough substantive text to clear content guards.
const ARTICLE_HTML: &str = r#"<html><body><article><h1>Benchmark Page</h1><p>Deterministic corpus content for the benchmark harness. This paragraph carries enough substantive text to clear the minimum content guard on every run.</p></article></body></html>"#;

/// Static HTML shell representing a JS-rendered page (SPA marker fixture).
const SPA_SHELL_HTML: &str = r#"<html><head><title>SPA Shell</title></head><body><div id="root" data-spa-shell="true"></div><noscript>JavaScript required</noscript></body></html>"#;

/// Body served after the WAF sequence completes.
const GUARDED_HTML: &str = r#"<html><body><article><h1>Gate Passed</h1><p>The benchmark crawler survived the simulated WAF sequence; this guarded page carries enough substantive text to clear the minimum content guard.</p></article></body></html>"#;

/// What kind of corpus page a manifest entry describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    /// Plain static HTML page.
    Static,
    /// JS-rendered shell (SPA marker) — exercises SPA detection.
    SpaShell,
    /// URL protected by the 403 → 429 → 200 WAF sequence.
    WafGuarded,
}

/// One entry of the corpus manifest: a relative path and its kind.
#[derive(Debug, Clone)]
pub struct CorpusPage {
    pub path: String,
    pub kind: PageKind,
}

/// Declarative description of the served corpus. Relative paths only — the
/// manifest never references an external host (AC-1.2).
#[derive(Debug, Clone)]
pub struct CorpusManifest {
    pub pages: Vec<CorpusPage>,
    /// Convenience accessor target: the WAF-guarded path.
    pub waf_guarded_path: String,
}

/// A live corpus server. Dropping this handle stops the server.
#[derive(Debug)]
pub struct CorpusHandle {
    /// Loopback base URL (`http://127.0.0.1:<ephemeral port>`).
    pub base_url: String,
    pub manifest: CorpusManifest,
    /// Held only to keep the wiremock server alive for the handle's lifetime;
    /// never read (mock mounting in wiremock 0.6 is infallible, so no shutdown
    /// API is needed).
    #[allow(dead_code)]
    server: MockServer,
}

/// Serve the Tier A corpus on `127.0.0.1` at an ephemeral port.
///
/// # Errors
///
/// Infallible in practice (wiremock 0.6 mounting cannot fail); keeps the
/// [`crate::error::Result`] contract so future failure sources map to
/// [`crate::error::BenchmarkError::Corpus`].
pub async fn serve() -> Result<CorpusHandle> {
    let server = MockServer::start().await;
    let base_url = server.uri();

    let mut manifest_pages = Vec::new();

    // Static pages.
    for p in [
        "/",
        "/about",
        "/articles/one",
        "/articles/two",
        "/articles/three",
    ] {
        mount_static(&server, p).await;
        manifest_pages.push(CorpusPage {
            path: p.to_string(),
            kind: PageKind::Static,
        });
    }

    // SPA shells (JS-rendered markers).
    for p in ["/app", "/dashboard"] {
        mount_spa_shell(&server, p).await;
        manifest_pages.push(CorpusPage {
            path: p.to_string(),
            kind: PageKind::SpaShell,
        });
    }

    // WAF-guarded URL: atomic counter 403 → 429(Retry-After) → 200.
    const GATE: &str = "/protected/gate";
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);
    Mock::given(method("GET"))
        .and(path(GATE))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            match count {
                0 => ResponseTemplate::new(403),
                1 => ResponseTemplate::new(429).insert_header("Retry-After", "0"),
                _ => ResponseTemplate::new(200).set_body_string(GUARDED_HTML),
            }
        })
        .mount(&server)
        .await;
    manifest_pages.push(CorpusPage {
        path: GATE.to_string(),
        kind: PageKind::WafGuarded,
    });

    Ok(CorpusHandle {
        base_url,
        manifest: CorpusManifest {
            pages: manifest_pages,
            waf_guarded_path: GATE.to_string(),
        },
        server,
    })
}

async fn mount_static(server: &MockServer, page_path: &str) {
    Mock::given(method("GET"))
        .and(path(page_path))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(server)
        .await;
}

async fn mount_spa_shell(server: &MockServer, page_path: &str) {
    Mock::given(method("GET"))
        .and(path(page_path))
        .respond_with(ResponseTemplate::new(200).set_body_string(SPA_SHELL_HTML))
        .mount(server)
        .await;
}

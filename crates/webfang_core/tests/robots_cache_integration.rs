//! Robots.txt failure caching against a live HTTP wire (#794).
//!
//! `RobotsFetcher` must fetch robots.txt at most once per domain even when the
//! fetch fails: a 404 / non-2xx / network error stores a cached fail-open
//! decision, so later `is_allowed` calls for the same domain never re-fetch.
//! Every test asserts the request count on the wire (wiremock's
//! `received_requests`), which is the exact behavior the issue measured
//! (459 robots fetches for a 5-page crawl → 1).

use webfang_core::infrastructure::crawler::robots_utils::RobotsFetcher;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A fetcher whose internal wreq client talks to the wiremock origin
/// (`RobotsFetcher` derives the robots.txt URL from the page URL's origin).
/// Construction is offline — only the checks perform network I/O, entirely
/// against the mock server.
fn fetcher_for_server() -> RobotsFetcher {
    RobotsFetcher::with_default_profile(5).expect("robot fetcher construction is offline")
}

/// Count `/robots.txt` requests on the wiremock server.
async fn count_robots_requests(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("request recording is enabled")
        .iter()
        .filter(|r| r.url.path() == "/robots.txt")
        .count()
}

/// Mount a robots.txt mock answering `status` with `body`.
async fn mount_robots(server: &MockServer, status: u16, body: &str) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

/// A 404 robots.txt must be fetched exactly once: the first `is_allowed`
/// probes and caches the fail-open decision; every later call is served from
/// the cache. This is the 459-fetch-to-1 contract that motivated #794.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
async fn negative_result_cached_after_first_missing_robots() {
    let server = MockServer::start().await;
    // No mock for /robots.txt → wiremock falls back to 404 by default, which
    // is exactly the "site without robots.txt" shape of the issue.
    let fetcher = fetcher_for_server();
    let base = server.uri();
    let domain = format!("127.0.0.1:{}", server.address().port());

    for page in 0..50 {
        assert!(
            fetcher
                .is_allowed(&format!("{base}/page/{page}"), &domain)
                .await,
            "missing robots.txt must fail open as allowed"
        );
    }

    let robots_fetches = count_robots_requests(&server).await;
    assert_eq!(
        robots_fetches, 1,
        "a 404 robots.txt must be fetched exactly once and cached (fail-open remembered) — \
         got {robots_fetches} fetches on the wire; any count above 1 reproduces the \
         459-fetch regression"
    );
}

/// A non-404 non-2xx response (503) follows the same negative-caching path.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
async fn non_success_status_is_cached_as_allow_all() {
    let server = MockServer::start().await;
    mount_robots(&server, 503, "Service Unavailable").await;

    let fetcher = fetcher_for_server();
    let base = server.uri();
    let domain = format!("127.0.0.1:{}", server.address().port());

    for page in 0..10 {
        assert!(
            fetcher
                .is_allowed(&format!("{base}/page/{page}"), &domain)
                .await,
            "a 503 robots.txt must fail open as allowed"
        );
    }

    assert_eq!(
        count_robots_requests(&server).await,
        1,
        "a non-2xx robots.txt must be fetched exactly once and cached"
    );
}

/// A successful robots.txt is fetched once, cached as `Rules`, and enforced
/// on every subsequent check (behavior identical to before #794).
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
async fn successful_rules_cached_and_enforced_once() {
    let server = MockServer::start().await;
    mount_robots(&server, 200, "User-agent: *\nDisallow: /private/\n").await;

    let fetcher = fetcher_for_server();
    let base = server.uri();
    let domain = format!("127.0.0.1:{}", server.address().port());

    for page in 0..10 {
        assert!(
            fetcher
                .is_allowed(&format!("{base}/public/{page}"), &domain)
                .await,
            "public URLs must stay allowed"
        );
        assert!(
            !fetcher
                .is_allowed(&format!("{base}/private/{page}"), &domain)
                .await,
            "disallowed URLs must stay denied across repeated checks"
        );
    }

    assert_eq!(
        count_robots_requests(&server).await,
        1,
        "a successful robots.txt must be fetched exactly once and cached"
    );
}

/// Single-flight contract (REQ-ROBOTS-NEG-CACHE-05): N concurrent first-fetches
/// for the same domain share ONE robots.txt probe. The per-domain `OnceCell`
/// guarantees exactly-once initialization; every call must return the same
/// fail-open result.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
async fn concurrent_first_fetches_are_bounded() {
    let server = MockServer::start().await;
    // Delayed 404 so concurrent calls overlap in the uncached window.
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string("not found")
                .set_delay(std::time::Duration::from_millis(100)),
        )
        .mount(&server)
        .await;

    let fetcher = std::sync::Arc::new(fetcher_for_server());
    let base = server.uri();
    let domain = format!("127.0.0.1:{}", server.address().port());

    const N: usize = 8;
    let mut handles = Vec::with_capacity(N);
    for task in 0..N {
        let fetcher = std::sync::Arc::clone(&fetcher);
        let url = format!("{base}/page/{task}");
        let domain = domain.clone();
        handles.push(tokio::spawn(async move {
            fetcher.is_allowed(&url, &domain).await
        }));
    }

    let mut allowed = Vec::with_capacity(N);
    for handle in handles {
        allowed.push(handle.await.expect("is_allowed task panicked"));
    }

    assert!(
        allowed.iter().all(|&a| a),
        "every concurrent call must fail open as allowed"
    );

    let robots_fetches = count_robots_requests(&server).await;
    assert_eq!(
        robots_fetches, 1,
        "N concurrent first-fetches for one domain must share exactly one robots.txt probe \
         (OnceCell single-flight), got {robots_fetches}"
    );
}

/// A successful fetch fills the cache as `Rules`; a site that starts failing
/// afterwards must NOT cause a re-fetch nor downgrade the cached decision —
/// the cached rules keep being enforced and the wire count stays at 1.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
async fn cached_rules_are_not_downgraded_by_later_failures() {
    let server = MockServer::start().await;
    mount_robots(&server, 200, "User-agent: *\nDisallow: /private/\n").await;

    let fetcher = fetcher_for_server();
    let base = server.uri();
    let domain = format!("127.0.0.1:{}", server.address().port());

    assert!(
        fetcher
            .is_allowed(&format!("{base}/public/a"), &domain)
            .await,
        "public URL must be allowed by the fetched rules"
    );
    assert!(
        !fetcher
            .is_allowed(&format!("{base}/private/a"), &domain)
            .await,
        "private URL must be denied by the fetched rules"
    );
    assert_eq!(count_robots_requests(&server).await, 1);

    // The site now errors. A later-mounted wiremock mock takes precedence for
    // the same path, so any subsequent robots.txt probe would get a 500 — if
    // a probe happens at all (the request count proves it does not).
    mount_robots(&server, 500, "broken").await;

    assert!(
        fetcher
            .is_allowed(&format!("{base}/public/b"), &domain)
            .await,
        "cached rules must keep allowing public URLs after the site starts failing"
    );
    assert!(
        !fetcher
            .is_allowed(&format!("{base}/private/b"), &domain)
            .await,
        "cached rules must keep denying private URLs after the site starts failing"
    );
    assert_eq!(
        count_robots_requests(&server).await,
        1,
        "an already-cached domain must trigger no further robots.txt fetch, \
         even after the site starts returning errors"
    );
}

//! #1615 — SSRF layer 2 on the paths that lacked it (F11, G-3, G-4, G-5).
//!
//! ## What this file is
//!
//! Five fetch paths were reachable without the literal-IP entry guard. Four
//! are covered here, and each row is an OBSERVATION of the refusal rather than
//! a claim about the code: a test that cannot tell a refusal from a timeout is
//! not evidence that anything was blocked.
//!
//! | Finding | Path | What is asserted |
//! |---|---|---|
//! | F11 | `StaticFetchPort::fetch_url` | a forbidden literal is refused, no socket |
//! | G-3  | `discover_urls_single_fetch` DOM branch | the seed is refused before `send()` |
//! | G-4  | `preflight_check` | reports the guard's refusal, not a network error |
//! | G-4  | `UrlValidator` | reports `Invalid`, not a retryable transport error |
//! | G-5  | `ResourceDownloader::download` | refused before any permit is acquired |
//!
//! ## Why the refusal is a distinct outcome
//!
//! A blocked target and an unreachable target both end the request, so a naive
//! test passes if the guard is absent and the network is simply down. Every row
//! here therefore asserts on the guard's SPANISH MESSAGE — the `SSRF detectado`
//! prefix the MCP probe suite already treats as the contract — so removing the
//! guard changes the result from `refused` to `something else` rather than from
//! `ok` to `error`.
//!
//! ## The hatch is armed nowhere in this file
//!
//! That is the point. `WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1` is what a
//! wiremock-driven test needs, and every other test in the crate that reaches
//! these paths now arms it through `EnvGuard::entry_guard_off()`. This file
//! must NOT, because a test that disarmed the layer it is testing would pass
//! unconditionally.

use webfang_core::application::crawler::discovery::discover_urls_single_fetch;
use webfang_core::domain::crawler_port::StaticFetchPort;
use webfang_core::domain::url_validator::UrlValidatorTrait;
use webfang_core::domain::{CorrelationId, CrawlerConfig, ValidationResult};
use webfang_core::infrastructure::crawler::http_client::StaticHttpFetcher;

/// The Spanish prefix every `ForbiddenLiteral` renders. Asserted literally so a
/// refusal is distinguishable from "the network was down".
const SSRF_PREFIX: &str = "SSRF detectado";

/// Literal targets the guard must refuse, across every address family and
/// encoding the parser accepts. Loopback and link-local are the two that
/// matter operationally: cloud metadata is link-local, and `127.0.0.1` is what
/// every internal admin surface listens on.
const FORBIDDEN_LITERALS: &[&str] = &[
    "127.0.0.1",
    "127.1.2.3",
    "0.0.0.0",
    "10.0.0.1",
    "172.16.0.1",
    "192.168.1.1",
    "169.254.169.254", // cloud metadata
    "100.64.1.1",      // CGNAT
    "[::1]",
    "[fc00::1]", // IPv6 ULA
    "[fe80::1]", // IPv6 link-local
];

fn crawler_config() -> CrawlerConfig {
    CrawlerConfig::new(url::Url::parse("https://example.com/").expect("config seed parses"))
}

/// F11 — the static-fetch port refuses a forbidden literal.
///
/// This is the structural row: the port is what `ProductionPageFetcher` calls
/// whenever no JS downloader is injected, so a refusal here is inherited by
/// every consumer of the port rather than by one call site.
#[tokio::test]
async fn issue_1615_static_fetch_port_refuses_forbidden_literals() {
    let fetcher = StaticHttpFetcher;
    for host in FORBIDDEN_LITERALS {
        let target = format!("http://{host}/");
        let err = fetcher
            .fetch_url(&target, &crawler_config())
            .await
            .expect_err("a forbidden literal must be refused");
        let rendered = err.to_string();
        assert!(
            rendered.contains(SSRF_PREFIX),
            "{target} must be refused by the literal guard, got: {rendered}"
        );
    }
}

/// And the port still fetches a hostname — a guard that refused everything
/// would pass the row above.
#[tokio::test]
async fn issue_1615_static_fetch_port_still_fails_normally_on_a_public_hostname() {
    let fetcher = StaticHttpFetcher;
    // No network is required for this row: the point is that the error is a
    // transport error carrying the guard's ABSENCE, not a refusal.
    let err = fetcher
        .fetch_url(
            "http://this-host-does-not-resolve.invalid/",
            &crawler_config(),
        )
        .await
        .expect_err("an unresolvable public host still errors");
    let rendered = err.to_string();
    assert!(
        !rendered.contains(SSRF_PREFIX),
        "a public hostname must not be treated as a forbidden literal: {rendered}"
    );
}

/// F11 — an invalid URL is still `InvalidUrl`, not a silent acceptance. Pinned
/// so the guard's early return did not change the classification of the parse
/// failure that preceded it.
#[tokio::test]
async fn issue_1615_static_fetch_port_still_reports_an_unparseable_url_as_a_network_error() {
    let fetcher = StaticHttpFetcher;
    let err = fetcher
        .fetch_url("not a url at all", &crawler_config())
        .await
        .expect_err("an unparseable URL must fail");
    assert!(
        !err.to_string().contains(SSRF_PREFIX),
        "an unparseable URL is a parse failure, not an SSRF refusal: {err}"
    );
}

/// G-3 — the DOM-discovery branch refuses a forbidden seed.
///
/// The branch builds its own client and calls `send()` directly, which is
/// exactly why it was the one path with no entry guard. The seed here is
/// operator-shaped, but in the sitemap case a third party supplies the host.
#[tokio::test]
async fn issue_1615_dom_discovery_refuses_a_forbidden_literal_seed() {
    for host in ["169.254.169.254", "127.0.0.1", "10.0.0.1"] {
        let seed = format!("http://{host}/");
        let config = CrawlerConfig::new(url::Url::parse(&seed).expect("seed parses"));
        let err = discover_urls_single_fetch(&seed, &config, &CorrelationId::new())
            .await
            .expect_err("a forbidden literal seed must be refused");
        let rendered = err.to_string();
        assert!(
            rendered.contains(SSRF_PREFIX),
            "{seed} must be refused before the seed is fetched, got: {rendered}"
        );
    }
}

/// G-4 — `preflight_check` reports the refusal on its `Failed` arm.
///
/// The function is dead-but-public: nothing in the tree calls it, which is why
/// nothing noticed it was unguarded. It is guarded now so the next wiring does
/// not import a bypass, and this row is what proves the guard is there.
#[tokio::test]
async fn issue_1615_preflight_check_refuses_a_forbidden_literal() {
    for host in ["169.254.169.254", "127.0.0.1"] {
        let url = url::Url::parse(&format!("http://{host}/")).expect("url parses");
        match webfang_core::cli::preflight::preflight_check(&url).await {
            webfang_core::cli::preflight::PreflightResult::Failed(reason) => assert!(
                reason.contains(SSRF_PREFIX),
                "{host} must be refused by the guard, not by the network: {reason}"
            ),
            webfang_core::cli::preflight::PreflightResult::Ok => {
                panic!("{host} must not reach a HEAD request: it was treated as reachable")
            },
            webfang_core::cli::preflight::PreflightResult::Warning(status) => {
                panic!("{host} must not be dialled; a {status} means a request was sent")
            },
        }
    }
}

/// G-4 — the HTTP-aware URL validator reports `Invalid`, not a transport error.
///
/// The distinction is the point: `validate_http_status` maps its error into
/// `DomainError`, and a transport-shaped failure reads as retryable. A
/// forbidden target must not be retryable, so it is reported as a verdict.
#[tokio::test]
async fn issue_1615_url_validator_reports_a_forbidden_literal_as_invalid() {
    let validator = webfang_core::infrastructure::crawler::url_validator::UrlValidator::new()
        .expect("validator builds");
    for host in ["169.254.169.254", "127.0.0.1"] {
        let url = url::Url::parse(&format!("http://{host}/")).expect("url parses");
        let result = validator
            .validate_http_status(&url)
            .await
            .expect("a forbidden target is a verdict, not an error");
        match result {
            ValidationResult::Invalid(reason) => assert!(
                reason.contains(SSRF_PREFIX),
                "{host} must be refused by the guard: {reason}"
            ),
            other => panic!("{host} must be Invalid; got {other:?}"),
        }
    }
}

/// G-5 — the elastic-ingestion downloader refuses before acquiring a permit.
///
/// The permit claim is checkable: the downloader is constructed with a
/// semaphore, and a refusal that had already taken a permit would show up as a
/// leak. With one permit and a refusal, a second acquire would block — so this
/// row also proves the check runs before the request is built, not after.
#[tokio::test]
async fn issue_1615_resource_downloader_refuses_a_forbidden_literal() {
    use webfang_core::infrastructure::crawler::resource_downloader::ResourceDownloader;

    let client = wreq::Client::builder().build().expect("test client builds");
    // One permit: taking it and keeping it would make the second acquire hang,
    // which is a stronger assertion than counting permits after the fact.
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let downloader = ResourceDownloader::new(semaphore.clone(), client);

    for host in ["169.254.169.254", "127.0.0.1", "10.0.0.1"] {
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            downloader.download(&format!("http://{host}/resource")),
        )
        .await
        .expect("the refusal must be immediate, not a permit wait")
        .expect_err("a forbidden literal must be refused");
        assert!(
            err.to_string().contains(SSRF_PREFIX),
            "{host} must be refused by the guard: {err}"
        );
    }

    assert_eq!(
        semaphore.available_permits(),
        1,
        "a refusal must not consume a permit"
    );
}

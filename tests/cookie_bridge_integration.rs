//! Integration tests for CookieBridge — domain matching, path matching,
//! cookie filtering, upsert behavior, and CDP conversion.

use webfang::infrastructure::downloader::cookie_bridge::{CdpCookie, CookieBridge};
use webfang::infrastructure::downloader::{Cookie, FetchedPage};
use url::Url;

fn make_cookie(name: &str, domain: &str, path: &str) -> Cookie {
    Cookie {
        name: name.into(),
        value: "val".into(),
        domain: domain.into(),
        path: path.into(),
        http_only: false,
        secure: false,
    }
}

fn make_page(url: &str, cookies: Vec<Cookie>) -> FetchedPage {
    FetchedPage {
        url: url.parse().unwrap(),
        html: String::new(),
        status: 200,
        cookies,
    }
}

// ── Domain matching ──────────────────────────────────────────────────────

#[test]
fn domain_exact_match() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", "example.com", "/"));

    let url: Url = "https://example.com/page".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].name, "s1");
}

#[test]
fn domain_subdomain_cookie_matches_bare_domain() {
    // Cookie set on ".example.com" should match "example.com"
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", ".example.com", "/"));

    let url: Url = "https://example.com/page".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert_eq!(matched.len(), 1);
}

#[test]
fn domain_subdomain_cookie_matches_subdomain() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", ".example.com", "/"));

    let url: Url = "https://sub.example.com/page".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert_eq!(matched.len(), 1);
}

#[test]
fn domain_different_domain_no_match() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", "example.com", "/"));

    let url: Url = "https://other.com/page".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert!(matched.is_empty());
}

#[test]
fn domain_subdomain_does_not_match_sibling() {
    // "a.example.com" cookie should NOT match "b.example.com"
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", "a.example.com", "/"));

    let url: Url = "https://b.example.com/page".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert!(matched.is_empty());
}

// ── Path matching ────────────────────────────────────────────────────────

#[test]
fn path_root_cookie_matches_all_paths() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", "example.com", "/"));

    for path in &["/anything", "/deep/nested/path", "/"] {
        let url: Url = format!("https://example.com{path}").parse().unwrap();
        let matched = bridge.cookies_for_url(&url);
        assert_eq!(matched.len(), 1, "path {path} should match root cookie");
    }
}

#[test]
fn path_specific_cookie_matches_prefix() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", "example.com", "/admin"));

    let url: Url = "https://example.com/admin/settings".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert_eq!(matched.len(), 1);
}

#[test]
fn path_specific_cookie_no_match_outside_prefix() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", "example.com", "/admin"));

    let url: Url = "https://example.com/public".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert!(matched.is_empty());
}

#[test]
fn path_exact_match_only() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", "example.com", "/admin"));

    let url: Url = "https://example.com/admin".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert_eq!(matched.len(), 1);
}

// ── Combined domain + path filtering ─────────────────────────────────────

#[test]
fn combined_domain_and_path_filtering() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("c1", "a.com", "/"));
    bridge.add(make_cookie("c2", "a.com", "/admin"));
    bridge.add(make_cookie("c3", "b.com", "/"));
    bridge.add(make_cookie("c4", "b.com", "/admin"));

    // Should match c1 (domain a.com, root path)
    let url: Url = "https://a.com/public".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].name, "c1");

    // Should match both c1 (root "/") and c2 ("/admin" prefix) for this URL
    let url: Url = "https://a.com/admin/dashboard".parse().unwrap();
    let matched = bridge.cookies_for_url(&url);
    assert_eq!(matched.len(), 2);
    let names: Vec<&str> = matched.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"c1"));
    assert!(names.contains(&"c2"));
}

// ── Ingest and upsert ───────────────────────────────────────────────────

#[test]
fn ingest_merges_cookies_from_page() {
    let mut bridge = CookieBridge::new();
    let page = make_page(
        "https://example.com",
        vec![
            make_cookie("session", "example.com", "/"),
            make_cookie("csrf", "example.com", "/"),
        ],
    );
    bridge.ingest(&page);
    assert_eq!(bridge.len(), 2);
}

#[test]
fn ingest_upserts_duplicate_cookies() {
    let mut bridge = CookieBridge::new();

    bridge.ingest(&make_page(
        "https://example.com",
        vec![Cookie {
            name: "session".into(),
            value: "old".into(),
            domain: "example.com".into(),
            path: "/".into(),
            http_only: false,
            secure: false,
        }],
    ));
    assert_eq!(bridge.len(), 1);
    assert_eq!(bridge.cookies()[0].value, "old");

    // Ingest again with same name+domain+path — should update value
    bridge.ingest(&make_page(
        "https://example.com",
        vec![Cookie {
            name: "session".into(),
            value: "new".into(),
            domain: "example.com".into(),
            path: "/".into(),
            http_only: true,
            secure: true,
        }],
    ));
    assert_eq!(bridge.len(), 1);
    assert_eq!(bridge.cookies()[0].value, "new");
    assert!(bridge.cookies()[0].http_only);
    assert!(bridge.cookies()[0].secure);
}

#[test]
fn ingest_does_not_duplicate_on_reingest() {
    let mut bridge = CookieBridge::new();
    let page = make_page(
        "https://example.com",
        vec![make_cookie("s1", "example.com", "/")],
    );
    bridge.ingest(&page);
    bridge.ingest(&page);
    bridge.ingest(&page);
    assert_eq!(bridge.len(), 1);
}

// ── CDP conversion ───────────────────────────────────────────────────────

#[test]
fn to_cdp_cookies_preserves_all_fields() {
    let mut bridge = CookieBridge::new();
    bridge.add(Cookie {
        name: "sid".into(),
        value: "abc123".into(),
        domain: ".example.com".into(),
        path: "/".into(),
        http_only: true,
        secure: true,
    });

    let cdp = bridge.to_cdp_cookies();
    assert_eq!(cdp.len(), 1);

    let c: &CdpCookie = &cdp[0];
    assert_eq!(c.name, "sid");
    assert_eq!(c.value, "abc123");
    assert_eq!(c.domain, ".example.com");
    assert_eq!(c.path, "/");
    assert!(c.http_only);
    assert!(c.secure);
}

#[test]
fn to_cdp_cookies_empty_bridge() {
    let bridge = CookieBridge::new();
    let cdp = bridge.to_cdp_cookies();
    assert!(cdp.is_empty());
}

// ── Clear and length ─────────────────────────────────────────────────────

#[test]
fn clear_removes_all_cookies() {
    let mut bridge = CookieBridge::new();
    bridge.add(make_cookie("s1", "a.com", "/"));
    bridge.add(make_cookie("s2", "b.com", "/"));
    assert_eq!(bridge.len(), 2);
    assert!(!bridge.is_empty());

    bridge.clear();
    assert_eq!(bridge.len(), 0);
    assert!(bridge.is_empty());
}

#[test]
fn len_and_is_empty_consistent() {
    let mut bridge = CookieBridge::new();
    assert_eq!(bridge.len(), 0);
    assert!(bridge.is_empty());

    bridge.add(make_cookie("s1", "a.com", "/"));
    assert_eq!(bridge.len(), 1);
    assert!(!bridge.is_empty());

    bridge.clear();
    assert_eq!(bridge.len(), 0);
    assert!(bridge.is_empty());
}

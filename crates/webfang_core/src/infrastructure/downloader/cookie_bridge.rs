//! Cookie bridge — extracts cookies from wreq responses and prepares them
//! for CDP injection (future Chromiumoxide use).
//!
//! Currently stores cookies in-memory. Future phases will call
//! `Network.setCookies` via CDP to inject cookies into a headless browser session.

use tracing::debug;
use url::Url;

use super::{Cookie, FetchedPage};

/// Extracts and stores cookies from fetched pages.
///
/// After each fetch, call [`CookieBridge::ingest`] to capture Set-Cookie
/// cookies. The accumulated cookie jar can then be injected into CDP
/// via [`CookieBridge::to_cdp_cookies`] (future).
#[derive(Debug, Clone, Default)]
pub struct CookieBridge {
    cookies: Vec<Cookie>,
}

impl CookieBridge {
    /// Create an empty cookie bridge.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest cookies from a fetched page.
    ///
    /// Merges new cookies with existing ones — duplicates (same name+domain+path)
    /// are replaced, new cookies are appended.
    pub fn ingest(&mut self, page: &FetchedPage) {
        for cookie in &page.cookies {
            self.upsert(cookie.clone());
        }
        if !page.cookies.is_empty() {
            debug!(
                "Ingested {} cookies from {} (total: {})",
                page.cookies.len(),
                page.url,
                self.cookies.len()
            );
        }
    }

    /// Add a single cookie, replacing any with the same name+domain+path.
    pub fn add(&mut self, cookie: Cookie) {
        self.upsert(cookie);
    }

    /// Get all stored cookies.
    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Get cookies that match a given URL's domain and path.
    pub fn cookies_for_url(&self, url: &Url) -> Vec<&Cookie> {
        let domain = url.host_str().unwrap_or("");
        self.cookies
            .iter()
            .filter(|c| domain_matches(domain, &c.domain) && path_matches(url.path(), &c.path))
            .collect()
    }

    /// Format cookies for CDP `Network.setCookies` (future use).
    ///
    /// Returns a `Vec` of cookie maps suitable for JSON serialization.
    /// Each map has keys: name, value, domain, path, httpOnly, secure.
    pub fn to_cdp_cookies(&self) -> Vec<CdpCookie> {
        self.cookies
            .iter()
            .map(|c| CdpCookie {
                name: c.name.clone(),
                value: c.value.clone(),
                domain: c.domain.clone(),
                path: c.path.clone(),
                http_only: c.http_only,
                secure: c.secure,
            })
            .collect()
    }

    /// Number of stored cookies.
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Whether the cookie jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Clear all stored cookies.
    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    fn upsert(&mut self, cookie: Cookie) {
        if let Some(existing) = self
            .cookies
            .iter_mut()
            .find(|c| c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path)
        {
            *existing = cookie;
        } else {
            self.cookies.push(cookie);
        }
    }
}

/// CDP-compatible cookie representation for `Network.setCookies`.
#[derive(Debug, Clone)]
pub struct CdpCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub http_only: bool,
    pub secure: bool,
}

/// Check if `cookie_domain` matches `request_domain`.
///
/// Follows standard cookie domain matching:
/// - Exact match: `example.com` matches `example.com`
/// - Subdomain match: `.example.com` matches `sub.example.com`
fn domain_matches(request_domain: &str, cookie_domain: &str) -> bool {
    if cookie_domain.starts_with('.') {
        // Subdomain cookie: .example.com matches example.com and sub.example.com
        request_domain == &cookie_domain[1..] || request_domain.ends_with(cookie_domain)
    } else {
        // Exact match only
        request_domain == cookie_domain
    }
}

/// Check if `request_path` matches `cookie_path`.
///
/// Follows standard cookie path matching: cookie path must be a prefix
/// of the request path.
fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if cookie_path == "/" {
        return true;
    }
    request_path.starts_with(cookie_path)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_ingest_merges_cookies() {
        let mut bridge = CookieBridge::new();
        let page = FetchedPage {
            url: "https://example.com".parse().unwrap(),
            html: "".into(),
            status: 200,
            cookies: vec![
                make_cookie("session", "example.com", "/"),
                make_cookie("csrf", "example.com", "/"),
            ],
        };
        bridge.ingest(&page);
        assert_eq!(bridge.len(), 2);

        // Ingest again — should upsert, not duplicate
        let page2 = FetchedPage {
            url: "https://example.com".parse().unwrap(),
            html: "".into(),
            status: 200,
            cookies: vec![make_cookie("session", "example.com", "/")],
        };
        bridge.ingest(&page2);
        assert_eq!(bridge.len(), 2);
    }

    #[test]
    fn test_cookies_for_url_filters_by_domain() {
        let mut bridge = CookieBridge::new();
        bridge.add(make_cookie("s1", "a.com", "/"));
        bridge.add(make_cookie("s2", "b.com", "/"));

        let url: Url = "https://a.com/page".parse().unwrap();
        let matched = bridge.cookies_for_url(&url);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "s1");
    }

    #[test]
    fn test_cookies_for_url_filters_by_path() {
        let mut bridge = CookieBridge::new();
        bridge.add(make_cookie("s1", "a.com", "/admin"));
        bridge.add(make_cookie("s2", "a.com", "/"));

        let url: Url = "https://a.com/public".parse().unwrap();
        let matched = bridge.cookies_for_url(&url);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "s2");
    }

    #[test]
    fn test_to_cdp_cookies() {
        let mut bridge = CookieBridge::new();
        bridge.add(Cookie {
            name: "sid".into(),
            value: "abc".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            http_only: true,
            secure: true,
        });
        let cdp = bridge.to_cdp_cookies();
        assert_eq!(cdp.len(), 1);
        assert!(cdp[0].http_only);
        assert!(cdp[0].secure);
    }

    #[test]
    fn test_domain_matches_exact() {
        assert!(domain_matches("example.com", "example.com"));
        assert!(!domain_matches("sub.example.com", "example.com"));
    }

    #[test]
    fn test_domain_matches_subdomain() {
        assert!(domain_matches("example.com", ".example.com"));
        assert!(domain_matches("sub.example.com", ".example.com"));
        assert!(!domain_matches("other.com", ".example.com"));
    }

    #[test]
    fn test_path_matches_root() {
        assert!(path_matches("/anything", "/"));
    }

    #[test]
    fn test_path_matches_prefix() {
        assert!(path_matches("/admin/settings", "/admin"));
        assert!(!path_matches("/public", "/admin"));
    }

    #[test]
    fn test_clear() {
        let mut bridge = CookieBridge::new();
        bridge.add(make_cookie("s1", "a.com", "/"));
        assert!(!bridge.is_empty());
        bridge.clear();
        assert!(bridge.is_empty());
    }
}

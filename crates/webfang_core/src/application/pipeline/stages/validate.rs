//! Validation pipeline stage.
//!
//! Checks incoming [`ScrapedItem`]s for basic validity before processing.
//! Rejects items with invalid URLs, empty content, or error status codes.

use std::future::Future;
use std::pin::Pin;

use crate::domain::pipeline_item::{
    FilterReason, PipelineStage, RejectReason, ScrapedItem, StageOutcome,
};

/// Patterns in URLs that indicate non-content pages.
const SKIP_PATHS: &[&str] = &[
    "/robots.txt",
    "/sitemap.xml",
    "/favicon.ico",
    "/.well-known/",
    "/ads.txt",
];

/// Pipeline stage that validates scraped items for basic correctness.
///
/// # Validation rules
///
/// - **URL validity**: must be non-empty, parseable, with http(s) scheme
/// - **Status code**: must be in 200..=399
/// - **Content length**: raw_html must be non-empty
/// - **Skip patterns**: robots.txt, sitemap.xml, and similar non-content pages
///   are silently skipped
pub struct ValidateStage;

impl PipelineStage for ValidateStage {
    fn name(&self) -> &str {
        "validate"
    }

    fn process(
        &self,
        item: ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = StageOutcome> + Send + '_>> {
        Box::pin(async move { validate(item) })
    }
}

fn validate(item: ScrapedItem) -> StageOutcome {
    // 1. URL must be non-empty
    if item.url.is_empty() {
        return StageOutcome::Rejected {
            reason: RejectReason::EmptyUrl,
        };
    }

    // 2. URL must be parseable with http(s) scheme
    let parsed = match url::Url::parse(&item.url) {
        Ok(u) => u,
        Err(_) => {
            return StageOutcome::Rejected {
                reason: RejectReason::InvalidUrl {
                    url: item.url.clone(),
                },
            };
        },
    };

    match parsed.scheme() {
        "http" | "https" => {},
        scheme => {
            return StageOutcome::Rejected {
                reason: RejectReason::UnsupportedScheme {
                    scheme: scheme.to_string(),
                },
            };
        },
    }

    if parsed.host_str().is_none() {
        return StageOutcome::Rejected {
            reason: RejectReason::MissingHost,
        };
    }

    // 3. Skip non-content pages (robots.txt, sitemap, etc.)
    let path = parsed.path().to_lowercase();
    if SKIP_PATHS.iter().any(|p| path == *p || path.starts_with(p)) {
        return StageOutcome::Filtered {
            reason: FilterReason::NonContentPath,
        };
    }

    // 4. Status code must be in 200..=399
    if !(200..=399).contains(&item.status_code) {
        return StageOutcome::Rejected {
            reason: RejectReason::HttpStatus {
                code: item.status_code,
            },
        };
    }

    // 5. raw_html must not be empty
    if item.raw_html.trim().is_empty() {
        return StageOutcome::Rejected {
            reason: RejectReason::EmptyContent,
        };
    }

    StageOutcome::Continue(item)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(url: &str, status: u16, html: &str) -> ScrapedItem {
        ScrapedItem {
            url: url.into(),
            raw_html: html.into(),
            status_code: status,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_valid_item_passes() {
        let item = make_item("https://example.com", 200, "<p>hi</p>");
        let stage = ValidateStage;
        let result = stage.process(item).await;
        assert!(matches!(result, StageOutcome::Continue(_)));
    }

    #[tokio::test]
    async fn test_empty_url_rejected() {
        let item = make_item("", 200, "<p>hi</p>");
        let result = ValidateStage.process(item).await;
        assert!(matches!(
            result,
            StageOutcome::Rejected {
                reason: RejectReason::EmptyUrl
            }
        ));
    }

    #[tokio::test]
    async fn test_invalid_url_rejected() {
        let item = make_item("not-a-url", 200, "<p>hi</p>");
        let result = ValidateStage.process(item).await;
        assert!(matches!(
            result,
            StageOutcome::Rejected {
                reason: RejectReason::InvalidUrl { .. }
            }
        ));
        if let StageOutcome::Rejected {
            reason: RejectReason::InvalidUrl { url },
        } = result
        {
            assert_eq!(url, "not-a-url");
        }
    }

    #[tokio::test]
    async fn test_ftp_scheme_rejected() {
        let item = make_item("ftp://example.com/file", 200, "<p>hi</p>");
        let result = ValidateStage.process(item).await;
        assert!(matches!(
            result,
            StageOutcome::Rejected {
                reason: RejectReason::UnsupportedScheme { .. }
            }
        ));
        if let StageOutcome::Rejected {
            reason: RejectReason::UnsupportedScheme { scheme },
        } = result
        {
            assert_eq!(scheme, "ftp");
        }
    }

    #[tokio::test]
    async fn test_error_status_rejected() {
        let item = make_item("https://example.com", 404, "<p>not found</p>");
        let result = ValidateStage.process(item).await;
        assert_eq!(
            result,
            StageOutcome::Rejected {
                reason: RejectReason::HttpStatus { code: 404 }
            }
        );
    }

    #[tokio::test]
    async fn test_server_error_rejected() {
        let item = make_item("https://example.com", 500, "<p>error</p>");
        let result = ValidateStage.process(item).await;
        assert_eq!(
            result,
            StageOutcome::Rejected {
                reason: RejectReason::HttpStatus { code: 500 }
            }
        );
    }

    #[tokio::test]
    async fn test_empty_content_rejected() {
        let item = make_item("https://example.com", 200, "   ");
        let result = ValidateStage.process(item).await;
        assert!(matches!(
            result,
            StageOutcome::Rejected {
                reason: RejectReason::EmptyContent
            }
        ));
    }

    #[tokio::test]
    async fn test_robots_txt_skipped() {
        let item = make_item("https://example.com/robots.txt", 200, "User-agent: *");
        let result = ValidateStage.process(item).await;
        assert_eq!(
            result,
            StageOutcome::Filtered {
                reason: FilterReason::NonContentPath
            }
        );
    }

    #[tokio::test]
    async fn test_sitemap_xml_skipped() {
        let item = make_item("https://example.com/sitemap.xml", 200, "<urlset>");
        let result = ValidateStage.process(item).await;
        assert_eq!(
            result,
            StageOutcome::Filtered {
                reason: FilterReason::NonContentPath
            }
        );
    }

    #[tokio::test]
    async fn test_favicon_skipped() {
        let item = make_item("https://example.com/favicon.ico", 200, "binary");
        let result = ValidateStage.process(item).await;
        assert_eq!(
            result,
            StageOutcome::Filtered {
                reason: FilterReason::NonContentPath
            }
        );
    }

    #[test]
    fn test_stage_name() {
        assert_eq!(ValidateStage.name(), "validate");
    }
}

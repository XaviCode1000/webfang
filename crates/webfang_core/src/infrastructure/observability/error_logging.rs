//! Structured error logging helpers (issue #356, Fase 3).
//!
//! Provides consistent, structured error logging with full operational
//! context (url, stage, trace_id) so errors captured in a trace file can be
//! correlated with the operation that produced them.

use crate::domain::CorrelationId;

/// Log a structured error with full operational context.
///
/// Emits a `tracing::error!` event carrying the error, the URL being
/// processed, the stage where it occurred, and the `trace_id` (when a
/// correlation ID is available) so the error can be correlated with the rest
/// of the operation in a trace file.
///
/// # Arguments
/// * `error` - The error to log (any `Display` type).
/// * `url` - The URL being processed when the error occurred.
/// * `stage` - The stage/phase where the error occurred (e.g. `"fetch"`, `"extract"`).
/// * `correlation_id` - Optional correlation ID; its `trace_id` is logged when present.
/// * `context` - Human-readable context message.
pub fn log_scrape_error<E: std::fmt::Display + ?Sized>(
    error: &E,
    url: &str,
    stage: &str,
    correlation_id: Option<&CorrelationId>,
    context: &str,
) {
    // `Option<String>` as a tracing field records the value when `Some` and is
    // omitted when `None`, so a single call handles both cases.
    tracing::error!(
        error = %error,
        url = %url,
        stage = %stage,
        trace_id = correlation_id.map(|c| c.trace_id().to_string()),
        "{context}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = Guard;
        fn make_writer(&'a self) -> Self::Writer {
            Guard(self.0.clone())
        }
    }

    struct Guard(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Guard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_subscriber(buf: Arc<Mutex<Vec<u8>>>) -> impl tracing::Subscriber {
        tracing_subscriber::fmt()
            .with_writer(SharedWriter(buf))
            .with_ansi(false)
            .finish()
    }

    #[test]
    fn test_log_scrape_error_emits_structured_fields() {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let _guard = tracing::subscriber::set_default(capture_subscriber(buf.clone()));

        let corr = CorrelationId::new();
        let err = std::io::Error::other("connection reset");
        log_scrape_error(
            &err,
            "https://example.com/page",
            "fetch",
            Some(&corr),
            "page fetch failed",
        );

        let out = String::from_utf8_lossy(&buf.lock().unwrap()).to_string();
        assert!(out.contains("ERROR"), "should be ERROR level: {out}");
        assert!(out.contains("page fetch failed"), "context message: {out}");
        assert!(out.contains("https://example.com/page"), "url field: {out}");
        assert!(out.contains("fetch"), "stage field: {out}");
        assert!(out.contains("connection reset"), "error display: {out}");
        assert!(
            out.contains(&corr.trace_id().to_string()),
            "trace_id field: {out}"
        );
    }

    #[test]
    fn test_log_scrape_error_without_correlation_id() {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let _guard = tracing::subscriber::set_default(capture_subscriber(buf.clone()));

        log_scrape_error(
            &"boom",
            "https://x.com",
            "extract",
            None,
            "extraction failed",
        );

        let out = String::from_utf8_lossy(&buf.lock().unwrap()).to_string();
        assert!(out.contains("extraction failed"), "context: {out}");
        assert!(out.contains("https://x.com"), "url: {out}");
        assert!(out.contains("extract"), "stage: {out}");
    }
}

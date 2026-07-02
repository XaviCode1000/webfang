//! OpenTelemetry Metric Instruments
//!
//! Lazy-initialized static metric instruments for HTTP and crawler operations.
//! All instruments are feature-gated behind `otel-metrics`.
//!
//! # Instruments
//!
//! | Name | Type | Description |
//! |------|------|-------------|
//! | `HTTP_DURATION` | Histogram | HTTP request latency in seconds |
//! | `HTTP_ERRORS` | Counter | Total HTTP errors (4xx/5xx) |
//! | `HTTP_IN_FLIGHT` | Observable gauge | Currently in-flight HTTP requests |
//! | `CRAWLER_PAGES` | Counter | Total pages scraped |
//! | `CRAWLER_URLS` | Counter | Total URLs discovered |
//! | `CRAWLER_BANDWIDTH` | Counter | Total bytes downloaded |

use std::sync::atomic::{AtomicU64, Ordering};

use once_cell::sync::Lazy;
use opentelemetry::global;
use opentelemetry::metrics::Histogram;
use opentelemetry::metrics::ObservableGauge;

/// Global in-flight request counter for the observable gauge.
static IN_FLIGHT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn meter() -> opentelemetry::metrics::Meter {
    global::meter_with_scope(
        opentelemetry::InstrumentationScope::builder("rust_scraper")
            .with_version(env!("CARGO_PKG_VERSION"))
            .build(),
    )
}

/// HTTP request duration histogram (seconds).
pub static HTTP_DURATION: Lazy<Histogram<f64>> = Lazy::new(|| {
    meter()
        .f64_histogram("http.client.duration")
        .with_description("HTTP request latency in seconds")
        .with_unit("s")
        .build()
});

/// HTTP error counter (4xx/5xx responses).
pub static HTTP_ERRORS: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("http.client.errors")
        .with_description("Total HTTP client errors (4xx/5xx)")
        .build()
});

/// In-flight HTTP requests observable gauge.
pub static HTTP_IN_FLIGHT: Lazy<ObservableGauge<u64>> = Lazy::new(|| {
    meter()
        .u64_observable_gauge("http.client.inflight")
        .with_description("Number of in-flight HTTP requests")
        .with_callback(|observer| {
            observer.observe(
                IN_FLIGHT_COUNTER.load(Ordering::Relaxed),
                &[opentelemetry::KeyValue::new("component", "http_client")],
            );
        })
        .build()
});

/// Pages scraped counter.
pub static CRAWLER_PAGES: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.pages.total")
        .with_description("Total pages scraped successfully")
        .build()
});

/// URLs discovered counter.
pub static CRAWLER_URLS: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.urls.total")
        .with_description("Total URLs discovered")
        .build()
});

/// Bandwidth downloaded counter (bytes).
pub static CRAWLER_BANDWIDTH: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.bandwidth.bytes")
        .with_description("Total bytes downloaded")
        .with_unit("By")
        .build()
});

/// Increment the in-flight requests counter.
pub fn in_flight_inc() {
    IN_FLIGHT_COUNTER.fetch_add(1, Ordering::Relaxed);
}

/// Decrement the in-flight requests counter.
pub fn in_flight_dec() {
    IN_FLIGHT_COUNTER.fetch_sub(1, Ordering::Relaxed);
}

/// Read the current in-flight count.
pub fn in_flight_count() -> u64 {
    IN_FLIGHT_COUNTER.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lazy_histogram_initializes() {
        // Accessing the Lazy triggers initialization — should not panic
        let _ = &*HTTP_DURATION;
    }

    #[test]
    fn test_lazy_counter_initializes() {
        let _ = &*HTTP_ERRORS;
    }

    #[test]
    fn test_lazy_gauge_initializes() {
        let _ = &*HTTP_IN_FLIGHT;
    }

    #[test]
    fn test_lazy_crawler_counters_initialize() {
        let _ = &*CRAWLER_PAGES;
        let _ = &*CRAWLER_URLS;
        let _ = &*CRAWLER_BANDWIDTH;
    }

    #[test]
    fn test_in_flight_inc_dec() {
        let before = in_flight_count();
        in_flight_inc();
        in_flight_inc();
        assert_eq!(in_flight_count(), before + 2);
        in_flight_dec();
        assert_eq!(in_flight_count(), before + 1);
        in_flight_dec();
        assert_eq!(in_flight_count(), before);
    }
}

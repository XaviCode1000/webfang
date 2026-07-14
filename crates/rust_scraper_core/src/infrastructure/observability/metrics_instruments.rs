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

// ---------------------------------------------------------------------------
// AtomicU64 backing counters for observable gauges
// ---------------------------------------------------------------------------

/// Global in-flight request counter for the observable gauge.
static IN_FLIGHT_COUNTER: AtomicU64 = AtomicU64::new(0);

static RAM_USAGE_BACKING: AtomicU64 = AtomicU64::new(0);
static CHROME_INSTANCES_BACKING: AtomicU64 = AtomicU64::new(0);
static BATCH_CONCURRENCY_BACKING: AtomicU64 = AtomicU64::new(0);
static ENGINE_CONCURRENCY_BACKING: AtomicU64 = AtomicU64::new(0);
static SESSION_POOL_HEALTHY_BACKING: AtomicU64 = AtomicU64::new(0);

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

// ---------------------------------------------------------------------------
// Downloader counters
// ---------------------------------------------------------------------------

/// Layer escalations counter.
pub static DOWNLOADER_ESCALATIONS: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("downloader.escalations.total")
        .with_description("Total layer escalations")
        .build()
});

/// WAF block counter.
pub static DOWNLOADER_WAF_BLOCKS: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("downloader.waf.blocks.total")
        .with_description("Total WAF blocks")
        .build()
});

/// Resource denied counter.
pub static RESOURCE_DENIED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("downloader.resource.denied.total")
        .with_description("Total resource-denied events")
        .build()
});

/// Obscura timeout counter.
pub static DOWNLOAD_OBSCURA_TIMEOUT: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("downloader.obscura.timeout.total")
        .with_description("Total Obscura download timeouts")
        .build()
});

// ---------------------------------------------------------------------------
// Pipeline counters
// ---------------------------------------------------------------------------

/// Pipeline items processed counter.
pub static PIPELINE_ITEMS_TOTAL: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("pipeline.items.total")
        .with_description("Total pipeline items processed")
        .build()
});

/// Pipeline items rejected counter.
pub static PIPELINE_ITEMS_REJECTED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("pipeline.items.rejected.total")
        .with_description("Total pipeline items rejected")
        .build()
});

/// Pipeline items skipped counter.
pub static PIPELINE_ITEMS_SKIPPED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("pipeline.items.skipped.total")
        .with_description("Total pipeline items skipped")
        .build()
});

/// Validation rejects counter.
pub static VALIDATE_REJECTS: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("pipeline.validate.rejects.total")
        .with_description("Total validation rejects")
        .build()
});

/// SPA detected counter.
pub static CLEAN_SPA_DETECTED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("pipeline.clean.spa.detected.total")
        .with_description("Total SPA detections during cleaning")
        .build()
});

/// Output sink errors counter.
pub static OUTPUT_SINK_ERRORS: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("pipeline.output.sink.errors.total")
        .with_description("Total output sink errors")
        .build()
});

// ---------------------------------------------------------------------------
// Batch counters
// ---------------------------------------------------------------------------

/// Batch jobs total counter.
pub static BATCH_JOBS_TOTAL: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("batch.jobs.total")
        .with_description("Total batch jobs started")
        .build()
});

/// Batch jobs completed counter.
pub static BATCH_JOBS_COMPLETED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("batch.jobs.completed.total")
        .with_description("Total batch jobs completed successfully")
        .build()
});

/// Batch jobs failed counter.
pub static BATCH_JOBS_FAILED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("batch.jobs.failed.total")
        .with_description("Total batch jobs that failed")
        .build()
});

/// Batch URLs processed counter.
pub static BATCH_URLS_PROCESSED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("batch.urls.processed.total")
        .with_description("Total URLs processed in batches")
        .build()
});

// ---------------------------------------------------------------------------
// Crawler counters
// ---------------------------------------------------------------------------

/// Engine pages crawled counter.
pub static ENGINE_PAGES_CRAWLED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.engine.pages.crawled.total")
        .with_description("Total pages crawled by engine")
        .build()
});

/// Engine checkpoint saves counter.
pub static ENGINE_CHECKPOINT_SAVES: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.engine.checkpoint.saves.total")
        .with_description("Total engine checkpoint saves")
        .build()
});

// ---------------------------------------------------------------------------
// Session pool counter
// ---------------------------------------------------------------------------

/// Session pool banned counter.
pub static SESSION_POOL_BANNED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.session_pool.banned.total")
        .with_description("Total sessions banned")
        .build()
});

// ---------------------------------------------------------------------------
// Concurrency & checkpoint counters
// ---------------------------------------------------------------------------

/// Autoscale level transitions counter.
pub static AUTOSCALE_LEVEL_TRANSITIONS: Lazy<opentelemetry::metrics::Counter<u64>> =
    Lazy::new(|| {
        meter()
            .u64_counter("crawler.autoscale.transitions.total")
            .with_description("Total autoscale level transitions")
            .build()
    });

/// Checkpoint saves counter.
pub static CHECKPOINT_SAVES: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.checkpoint.saves.total")
        .with_description("Total checkpoint saves")
        .build()
});

/// Checkpoint loads counter.
pub static CHECKPOINT_LOADS: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.checkpoint.loads.total")
        .with_description("Total checkpoint loads")
        .build()
});

/// Checkpoint corrupted counter.
pub static CHECKPOINT_CORRUPTED: Lazy<opentelemetry::metrics::Counter<u64>> = Lazy::new(|| {
    meter()
        .u64_counter("crawler.checkpoint.corrupted.total")
        .with_description("Total corrupted checkpoints detected")
        .build()
});

// ---------------------------------------------------------------------------
// Downloader histograms
// ---------------------------------------------------------------------------

/// Layer latency histogram (seconds).
pub static DOWNLOADER_LAYER_LATENCY: Lazy<Histogram<f64>> = Lazy::new(|| {
    meter()
        .f64_histogram("downloader.layer.latency")
        .with_description("Layer fetch latency in seconds")
        .with_unit("s")
        .build()
});

/// Obscura download latency histogram (seconds).
pub static DOWNLOAD_OBSCURA_LATENCY: Lazy<Histogram<f64>> = Lazy::new(|| {
    meter()
        .f64_histogram("downloader.obscura.latency")
        .with_description("Obscura download latency in seconds")
        .with_unit("s")
        .build()
});

/// Wreq download latency histogram (seconds).
pub static DOWNLOAD_WREQ_LATENCY: Lazy<Histogram<f64>> = Lazy::new(|| {
    meter()
        .f64_histogram("downloader.wreq.latency")
        .with_description("Wreq download latency in seconds")
        .with_unit("s")
        .build()
});

/// HTML reduction percentage histogram.
pub static CLEAN_REDUCTION_PCT: Lazy<Histogram<f64>> = Lazy::new(|| {
    meter()
        .f64_histogram("pipeline.clean.reduction_pct")
        .with_description("HTML reduction percentage after cleaning")
        .with_unit("%")
        .build()
});

/// Session pool backoff histogram (seconds).
pub static SESSION_POOL_BACKOFF: Lazy<Histogram<f64>> = Lazy::new(|| {
    meter()
        .f64_histogram("crawler.session_pool.backoff")
        .with_description("Session pool backoff delay in seconds")
        .with_unit("s")
        .build()
});

// ---------------------------------------------------------------------------
// Observable gauges (backed by AtomicU64)
// ---------------------------------------------------------------------------

/// RAM usage percentage gauge.
pub static RAM_USAGE_PERCENT: Lazy<ObservableGauge<u64>> = Lazy::new(|| {
    meter()
        .u64_observable_gauge("downloader.resource.ram_usage_percent")
        .with_description("Current RAM usage percentage")
        .with_unit("%")
        .with_callback(|observer| {
            observer.observe(RAM_USAGE_BACKING.load(Ordering::Relaxed), &[]);
        })
        .build()
});

/// Active Chrome instances gauge.
pub static CHROME_INSTANCES_ACTIVE: Lazy<ObservableGauge<u64>> = Lazy::new(|| {
    meter()
        .u64_observable_gauge("downloader.resource.chrome_instances_active")
        .with_description("Number of active Chrome instances")
        .with_callback(|observer| {
            observer.observe(CHROME_INSTANCES_BACKING.load(Ordering::Relaxed), &[]);
        })
        .build()
});

/// Batch concurrency level gauge.
pub static BATCH_CONCURRENCY_CURRENT: Lazy<ObservableGauge<u64>> = Lazy::new(|| {
    meter()
        .u64_observable_gauge("batch.concurrency.current")
        .with_description("Current batch processing concurrency")
        .with_callback(|observer| {
            observer.observe(BATCH_CONCURRENCY_BACKING.load(Ordering::Relaxed), &[]);
        })
        .build()
});

/// Engine concurrency level gauge.
pub static ENGINE_CONCURRENCY_LEVEL: Lazy<ObservableGauge<u64>> = Lazy::new(|| {
    meter()
        .u64_observable_gauge("crawler.engine.concurrency_level")
        .with_description("Current engine concurrency level")
        .with_callback(|observer| {
            observer.observe(ENGINE_CONCURRENCY_BACKING.load(Ordering::Relaxed), &[]);
        })
        .build()
});

/// Session pool healthy sessions gauge.
pub static SESSION_POOL_HEALTHY: Lazy<ObservableGauge<u64>> = Lazy::new(|| {
    meter()
        .u64_observable_gauge("crawler.session_pool.healthy")
        .with_description("Number of healthy sessions in pool")
        .with_callback(|observer| {
            observer.observe(SESSION_POOL_HEALTHY_BACKING.load(Ordering::Relaxed), &[]);
        })
        .build()
});

// ---------------------------------------------------------------------------
// Gauge helper functions
// ---------------------------------------------------------------------------

/// Set RAM usage percentage.
pub fn update_ram_usage(value: u64) {
    RAM_USAGE_BACKING.store(value, Ordering::Relaxed);
}

/// Read RAM usage percentage.
pub fn ram_usage_get() -> u64 {
    RAM_USAGE_BACKING.load(Ordering::Relaxed)
}

/// Set active Chrome instances count.
pub fn update_chrome_instances(value: u64) {
    CHROME_INSTANCES_BACKING.store(value, Ordering::Relaxed);
}

/// Read active Chrome instances count.
pub fn chrome_instances_get() -> u64 {
    CHROME_INSTANCES_BACKING.load(Ordering::Relaxed)
}

/// Set batch concurrency level.
pub fn update_batch_concurrency(value: u64) {
    BATCH_CONCURRENCY_BACKING.store(value, Ordering::Relaxed);
}

/// Read batch concurrency level.
pub fn batch_concurrency_get() -> u64 {
    BATCH_CONCURRENCY_BACKING.load(Ordering::Relaxed)
}

/// Set engine concurrency level.
pub fn update_engine_concurrency(value: u64) {
    ENGINE_CONCURRENCY_BACKING.store(value, Ordering::Relaxed);
}

/// Read engine concurrency level.
pub fn engine_concurrency_get() -> u64 {
    ENGINE_CONCURRENCY_BACKING.load(Ordering::Relaxed)
}

/// Set healthy session count.
pub fn update_session_pool_healthy(value: u64) {
    SESSION_POOL_HEALTHY_BACKING.store(value, Ordering::Relaxed);
}

/// Read healthy session count.
pub fn session_pool_healthy_get() -> u64 {
    SESSION_POOL_HEALTHY_BACKING.load(Ordering::Relaxed)
}

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

    #[test]
    fn test_new_counters_init() {
        let _ = &*DOWNLOADER_ESCALATIONS;
        let _ = &*DOWNLOADER_WAF_BLOCKS;
        let _ = &*RESOURCE_DENIED;
        let _ = &*DOWNLOAD_OBSCURA_TIMEOUT;
        let _ = &*PIPELINE_ITEMS_TOTAL;
        let _ = &*PIPELINE_ITEMS_REJECTED;
        let _ = &*PIPELINE_ITEMS_SKIPPED;
        let _ = &*VALIDATE_REJECTS;
        let _ = &*CLEAN_SPA_DETECTED;
        let _ = &*OUTPUT_SINK_ERRORS;
        let _ = &*BATCH_JOBS_TOTAL;
        let _ = &*BATCH_JOBS_COMPLETED;
        let _ = &*BATCH_JOBS_FAILED;
        let _ = &*BATCH_URLS_PROCESSED;
        let _ = &*ENGINE_PAGES_CRAWLED;
        let _ = &*ENGINE_CHECKPOINT_SAVES;
        let _ = &*SESSION_POOL_BANNED;
        let _ = &*AUTOSCALE_LEVEL_TRANSITIONS;
        let _ = &*CHECKPOINT_SAVES;
        let _ = &*CHECKPOINT_LOADS;
        let _ = &*CHECKPOINT_CORRUPTED;
    }

    #[test]
    fn test_new_histograms_init() {
        let _ = &*DOWNLOADER_LAYER_LATENCY;
        let _ = &*DOWNLOAD_OBSCURA_LATENCY;
        let _ = &*DOWNLOAD_WREQ_LATENCY;
        let _ = &*CLEAN_REDUCTION_PCT;
        let _ = &*SESSION_POOL_BACKOFF;
    }

    #[test]
    fn test_new_gauges_init() {
        let _ = &*RAM_USAGE_PERCENT;
        let _ = &*CHROME_INSTANCES_ACTIVE;
        let _ = &*BATCH_CONCURRENCY_CURRENT;
        let _ = &*ENGINE_CONCURRENCY_LEVEL;
        let _ = &*SESSION_POOL_HEALTHY;
    }

    #[test]
    fn test_gauge_backing_roundtrip() {
        update_ram_usage(42);
        assert_eq!(ram_usage_get(), 42);
        update_ram_usage(0);
        assert_eq!(ram_usage_get(), 0);

        update_chrome_instances(3);
        assert_eq!(chrome_instances_get(), 3);

        update_batch_concurrency(5);
        assert_eq!(batch_concurrency_get(), 5);

        update_engine_concurrency(2);
        assert_eq!(engine_concurrency_get(), 2);

        update_session_pool_healthy(10);
        assert_eq!(session_pool_healthy_get(), 10);
    }
}

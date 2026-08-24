//! Trace JSONL parsing + metric computation (FR-3).
//!
//! Normative shape contract: the records `scripts/analyze-trace.sh` queries —
//! engine summary (`message == "crawl completed"` with numeric fields under
//! `fields`), `span_close` records with top-level `span_duration_ms`, ERROR
//! events carrying `fields.url`, and WAF challenge events. Pinned by golden
//! fixtures (ADR-B6); any drift breaks `golden_parser_test`.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{BenchmarkError, Result};

/// Lenient stage-1 model: every FileTraceLayer line deserializes into this.
#[derive(Debug, Deserialize)]
pub struct TraceLine {
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// `"span_close"` on span-close records (top-level, per FileTraceLayer).
    #[serde(default)]
    pub record: Option<String>,
    /// Top-level wall-clock duration on span_close records.
    #[serde(default)]
    pub span_duration_ms: Option<f64>,
    /// Event fields (numeric summary fields land here via `record_u64`).
    #[serde(default)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

/// Strict stage-2 classification with line provenance.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceRecord {
    /// Engine summary line (`message == "crawl completed"`), fields lifted.
    Summary(CrawlSummary),
    /// Span close with its wall-clock duration in milliseconds.
    SpanClose { duration_ms: f64 },
    /// ERROR event naming a failed URL (`fields.url`, per analyze-trace.sh
    /// urls-failed query).
    UrlsFailed { count: u64 },
    /// WAF challenge / banned-domain event (analyze-trace.sh waf query).
    WafEvent,
    /// Any benign line that carries no metric we consume.
    Ignored,
}

/// The engine's `crawl completed` payload (11 numeric fields + duration +
/// throughput + optional trace id). Exact-equality pinned by goldens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrawlSummary {
    pub total_pages: u64,
    pub succeeded: u64,
    pub errors: u64,
    pub errors_waf: u64,
    pub errors_http: u64,
    pub errors_timeout: u64,
    pub errors_network: u64,
    pub errors_rate_limit: u64,
    pub errors_extraction: u64,
    pub errors_internal: u64,
    pub errors_panic: u64,
    pub duration_secs: f64,
    pub pages_per_sec: f64,
    #[serde(default)]
    pub trace_id: Option<String>,
}

/// The 8 error-bucket field names the engine emits (issue #374).
const BUCKET_KEYS: [&str; 8] = [
    "errors_waf",
    "errors_http",
    "errors_timeout",
    "errors_network",
    "errors_rate_limit",
    "errors_extraction",
    "errors_internal",
    "errors_panic",
];

/// Parse a trace JSONL file into classified records (design §3 two-stage flow).
///
/// Lenient by design: partial traces parse fine ([`TraceRecord::Ignored`] for
/// benign lines). Required-shape enforcement happens in [`summary_of`] and
/// [`compute`] so partial-shape acceptance and loud pipeline failure coexist.
///
/// # Errors
///
/// - [`BenchmarkError::Io`] if the file cannot be opened/read.
/// - [`BenchmarkError::Jsonl`] with the 1-based line number for invalid JSON.
/// - [`BenchmarkError::Shape`] with the 1-based line number for a summary line
///   carrying an unknown error-bucket key or non-numeric required field.
pub fn parse_file(path: &Path) -> Result<Vec<TraceRecord>> {
    let reader = BufReader::new(File::open(path)?);
    let mut records = Vec::new();
    for (idx, line) in reader.lines().enumerate() {
        let line_no = idx + 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed: TraceLine = serde_json::from_str(&line)
            .map_err(|source| BenchmarkError::Jsonl { line: line_no, source })?;
        classify(parsed, line_no, &mut records)?;
    }
    Ok(records)
}

/// Extract the engine summary from classified records.
///
/// # Errors
///
/// [`BenchmarkError::MissingSummary`] when no summary record is present.
pub fn summary_of<'a>(records: &'a [TraceRecord], path: &str) -> Result<&'a CrawlSummary> {
    for record in records {
        if let TraceRecord::Summary(summary) = record {
            return Ok(summary);
        }
    }
    Err(BenchmarkError::MissingSummary {
        path: path.to_string(),
    })
}

fn classify(line: TraceLine, line_no: usize, out: &mut Vec<TraceRecord>) -> Result<()> {
    if line.record.as_deref() == Some("span_close") {
        match line.span_duration_ms {
            Some(duration_ms) => out.push(TraceRecord::SpanClose { duration_ms }),
            None => {
                return Err(BenchmarkError::Shape {
                    line: line_no,
                    detail: "span_close record without span_duration_ms".to_string(),
                });
            }
        }
        return Ok(());
    }

    if line.message.as_deref() == Some("crawl completed") {
        let summary = lift_summary(&line.fields, line_no)?;
        out.push(TraceRecord::Summary(summary));
        return Ok(());
    }

    // urls-failed: ERROR event carrying fields.url (analyze-trace.sh query).
    if line.level.as_deref() == Some("ERROR") && line.fields.contains_key("url") {
        out.push(TraceRecord::UrlsFailed { count: 1 });
        return Ok(());
    }

    // WAF events: message matches the analyze-trace.sh wad... waf query shape.
    if let Some(message) = line.message.as_deref() {
        if message.contains("WAF") || message.contains("Banned domain") {
            out.push(TraceRecord::WafEvent);
            return Ok(());
        }
    }

    out.push(TraceRecord::Ignored);
    Ok(())
}

fn lift_summary(
    fields: &serde_json::Map<String, serde_json::Value>,
    line_no: usize,
) -> Result<CrawlSummary> {
    // Unknown error buckets are shape violations: they mean the engine's
    // breakdown contract changed under us (AC-3.3 drift detection at parse time).
    for key in fields.keys() {
        if key.starts_with("errors_") && !BUCKET_KEYS.contains(&key.as_str()) && key != "errors" {
            return Err(BenchmarkError::Shape {
                line: line_no,
                detail: format!("unknown error bucket `{key}`"),
            });
        }
    }

    let get_u64 = |name: &str| -> Result<u64> {
        fields.get(name).and_then(serde_json::Value::as_u64).ok_or_else(|| {
            BenchmarkError::Shape {
                line: line_no,
                detail: format!("summary field `{name}` missing or not an integer"),
            }
        })
    };
    let get_f64 = |name: &str| -> Result<f64> {
        fields.get(name).and_then(serde_json::Value::as_f64).ok_or_else(|| {
            BenchmarkError::Shape {
                line: line_no,
                detail: format!("summary field `{name}` missing or not a number"),
            }
        })
    };

    Ok(CrawlSummary {
        total_pages: get_u64("total_pages")?,
        succeeded: get_u64("succeeded")?,
        errors: get_u64("errors")?,
        errors_waf: get_u64("errors_waf")?,
        errors_http: get_u64("errors_http")?,
        errors_timeout: get_u64("errors_timeout")?,
        errors_network: get_u64("errors_network")?,
        errors_rate_limit: get_u64("errors_rate_limit")?,
        errors_extraction: get_u64("errors_extraction")?,
        errors_internal: get_u64("errors_internal")?,
        errors_panic: get_u64("errors_panic")?,
        duration_secs: get_f64("duration_secs")?,
        pages_per_sec: get_f64("pages_per_sec")?,
        trace_id: fields
            .get("trace_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

/// Extract the engine summary from classified records.

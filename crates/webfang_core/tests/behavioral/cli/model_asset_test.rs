//! Per-tier ONNX model RSS budgets (#1315) and cold-pull observability (#1316).
//!
//! All three tests are ignored: `tier_*` requires the real Granite ONNX blobs
//! in the native HF cache, and `cold_pull_*` performs a real network pull.
//! Run them explicitly with:
//!
//! ```bash
//! cargo nextest run -p webfang_core --features ai --test behavioral --run-ignored only tier_
//! cargo nextest run -p webfang_core --features ai --test behavioral --run-ignored only cold_pull
//! ```

use std::path::{Path, PathBuf};

use crate::BehavioralTest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Peak-RSS budgets are PER-TIER (#1315). The fix removes the application-side
/// full in-RAM model copy (streamed SHA-256 + `commit_from_file`); each tier
/// then gets its own ceiling instead of one shared "sane (<~2 GB)" number
/// (`docs/manual/PLAN_PRUEBAS_REALES.md` row 7.12).
///
/// Values are in KiB (`/usr/bin/time -v` reports "Maximum resident set size
/// (kbytes)") and are set from MEASURED evidence on this host class, sized to
/// stay under the pre-fix regression while leaving noise headroom:
///
/// - 311m measured (post-fix, warm): ~2,147,000 KiB across repeated runs
///   (variance < 0.1%); pre-fix 3,360,932 KiB (#1315). Budget 2.5 GiB.
/// - 97m measured (post-fix, warm): ~770,500 KiB; pre-fix 1,152,544 KiB.
///   Budget 1 GiB — the gap between measured and pre-fix is only ~382 MiB,
///   so a looser ceiling would stop catching the double-copy regression.
///
/// NOTE on row 7.12's "mmap used not loaded": the vendored ONNX Runtime build
/// has no mmap model-loading (zero `external_mmap` strings in
/// `libonnxruntime.a`), and a no-inference run over a zero-chunk page measures
/// the SAME ~2,147,000 KiB — ORT materializes the inline weights at session
/// creation. The ~1.2 GiB single weight copy is therefore the tier's honest
/// floor; the budgets assert "loaded once", not "never loaded".
/// Granite-311M tier: 2560 MiB (measured floor ~2097 MiB + ~463 MiB headroom).
const TIER_311M_MAX_RSS_KIB: u64 = 2_621_440;
/// Granite-97M tier: 1024 MiB (measured floor ~752 MiB + ~272 MiB headroom,
/// still below the pre-fix 1125 MiB so the budget stays sensitive).
const TIER_97M_MAX_RSS_KIB: u64 = 1_048_576;

/// HF hub cache directory names (under `<cache>/hub/`) for the two tiers.
const GRANITE_97M_REPO_DIR: &str = "models--ibm-granite--granite-embedding-97m-multilingual-r2";
const GRANITE_311M_REPO_DIR: &str = "models--ibm-granite--granite-embedding-311m-multilingual-r2";

const PAGE_HTML: &str = r#"
<html><head><title>Model Asset Test</title></head>
<body><main><article>
<h1>Model Asset Budget Target</h1>
<p>This paragraph contains enough meaningful content for the semantic cleaner
to produce a real document chunk with embeddings. The extractor needs
sufficient text to trigger readability extraction and subsequent AI cleaning.</p>
<p>A second paragraph provides additional context for the chunker so the AI
pipeline has material to tokenize, embed, and score for relevance filtering.</p>
</article></main></body></html>
"#;

/// Resolve the native HF cache directory, honoring a pre-existing `HF_HOME`
/// (same precedence hf-hub uses), else `$HOME/.cache/huggingface`.
fn native_hf_cache_dir() -> PathBuf {
    if let Ok(hf_home) = std::env::var("HF_HOME") {
        return PathBuf::from(hf_home);
    }
    let home = std::env::var("HOME").expect("HOME must be set to locate the native HF cache");
    PathBuf::from(home).join(".cache").join("huggingface")
}

/// Locate the `onnx/model.onnx` snapshot file for a tier inside the native
/// cache: `<cache>/hub/<repo_dir>/snapshots/<rev>/onnx/model.onnx`.
/// Returns `None` when the tier is not cached (the caller panics with an
/// instructive message — running these tests is a deliberate manual act).
fn model_snapshot_file(cache: &Path, repo_dir: &str) -> Option<PathBuf> {
    let snapshots = cache.join("hub").join(repo_dir).join("snapshots");
    let entries = std::fs::read_dir(&snapshots).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let candidate = entry.path().join("onnx").join("model.onnx");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Run `webfang --single-page <mock> --clean-ai --ai-model <flag> --quiet`
/// wrapped in `/usr/bin/time -v`, assert success, and return the measured
/// peak RSS in KiB.
///
/// The HF cache is pinned via `HF_HOME` so the run resolves the model from
/// the native cache (warm run — cold pulls are `cold_pull_*`'s job), and the
/// webfang state cache gets its own `XDG_CACHE_HOME` (mirroring the harness)
/// so wiremock port reuse cannot leak resume state across tests.
fn run_and_measure_peak_rss_kib(t: &BehavioralTest, model_flag: &str, hf_cache: &Path) -> u64 {
    const GNU_TIME: &str = "/usr/bin/time";
    assert!(
        Path::new(GNU_TIME).exists(),
        "GNU time is required at {GNU_TIME}: install the 'time' package \
         (Debian/Ubuntu: apt install time) to run this RSS-budget test"
    );

    let time_report = t.out.path().join("time-report.txt");
    let mut cmd = std::process::Command::new(GNU_TIME);
    cmd.args(["-v", "-o"])
        .arg(&time_report)
        .arg(crate::common::webfang_path())
        .arg("--url")
        .arg(t.server.uri())
        .arg("--output")
        .arg(t.out.path())
        .arg("--single-page")
        .arg("--clean-ai")
        .arg("--ai-model")
        .arg(model_flag)
        .arg("--quiet")
        .env("HF_HOME", hf_cache)
        // Fixed locale so the `-v` report format is parseable regardless of
        // the host's environment.
        .env("LC_ALL", "C")
        .env(
            webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
            "1",
        );
    let xdg_cache = t.out.path().join("hermetic-cache");
    let _ = std::fs::create_dir_all(&xdg_cache);
    cmd.env("XDG_CACHE_HOME", &xdg_cache);

    let output = cmd
        .output()
        .expect("spawn /usr/bin/time -v wrapping webfang");
    assert!(
        output.status.success(),
        "scrape must succeed on a warm cache: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report = std::fs::read_to_string(&time_report)
        .unwrap_or_else(|e| panic!("read /usr/bin/time report {}: {e}", time_report.display()));
    parse_max_rss_kib(&report)
}

/// Parse "Maximum resident set size (kbytes): <N>" from a GNU time -v report.
fn parse_max_rss_kib(report: &str) -> u64 {
    for line in report.lines() {
        if let Some(rest) = line
            .trim()
            .strip_prefix("Maximum resident set size (kbytes):")
        {
            return rest
                .trim()
                .parse::<u64>()
                .unwrap_or_else(|e| panic!("unparseable RSS value in {line:?}: {e}"));
        }
    }
    panic!(
        "GNU time -v report has no 'Maximum resident set size (kbytes):' line.\n\
         Report contents:\n{report}"
    )
}

// ============================================================================
// 1. Per-tier RSS budgets (#1315)
// ============================================================================

/// The Granite-311m tier must stay under its per-tier peak-RSS budget.
///
/// Pre-fix, the ~1.2 GB blob was materialized in RAM twice (hash buffer +
/// ORT in-memory copy), pushing measured warm RSS to 3,360,932 KiB (~3.2 GiB).
/// Post-fix, the hash streams in 1 MiB chunks and the buffer never exists:
/// ORT holds the single copy of the inline weights (~2,147,000 KiB measured,
/// including at session creation — see the budget comment for why this build
/// cannot defer-load them), so the tier sits ~1.2 GiB below the pre-fix peak.
#[tokio::test]
#[ignore = "requires the granite-311m ONNX model (~1.2 GB) in the native HF cache"]
async fn tier_311m_peak_rss_under_budget() {
    let cache = native_hf_cache_dir();
    if model_snapshot_file(&cache, GRANITE_311M_REPO_DIR).is_none() {
        panic!(
            "the granite-311m ONNX blob was not found under {}.\n\
             Running this test is a deliberate manual act: pre-cache the model \
             (e.g. `huggingface-cli download ibm-granite/granite-embedding-311m-multilingual-r2` \
             or one `--clean-ai --ai-model granite-311m` run) and retry.",
            cache.join("hub").join(GRANITE_311M_REPO_DIR).display()
        );
    }

    let t = BehavioralTest::new().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_HTML))
        .mount(&t.server)
        .await;

    let peak_kib = run_and_measure_peak_rss_kib(&t, "granite-311m", &cache);

    assert!(
        peak_kib < TIER_311M_MAX_RSS_KIB,
        "granite-311m peak RSS {peak_kib} KiB must stay under the per-tier \
         budget {TIER_311M_MAX_RSS_KIB} KiB (2560 MiB) — a full in-RAM model \
         copy is back (#1315)"
    );
}

/// The Granite-97m tier must stay under its per-tier peak-RSS budget.
#[tokio::test]
#[ignore = "requires the granite-97m model in the native HF cache"]
async fn tier_97m_peak_rss_under_budget() {
    let cache = native_hf_cache_dir();
    if model_snapshot_file(&cache, GRANITE_97M_REPO_DIR).is_none() {
        panic!(
            "the granite-97m ONNX blob was not found under {}.\n\
             Running this test is a deliberate manual act: pre-cache the model \
             (one `--clean-ai` run downloads it) and retry.",
            cache.join("hub").join(GRANITE_97M_REPO_DIR).display()
        );
    }

    let t = BehavioralTest::new().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_HTML))
        .mount(&t.server)
        .await;

    let peak_kib = run_and_measure_peak_rss_kib(&t, "granite-97m", &cache);

    assert!(
        peak_kib < TIER_97M_MAX_RSS_KIB,
        "granite-97m peak RSS {peak_kib} KiB must stay under the per-tier \
         budget {TIER_97M_MAX_RSS_KIB} KiB (1024 MiB) — a full in-RAM model \
         copy is back (#1315)"
    );
}

// ============================================================================
// 2. Cold-pull observability (#1316)
// ============================================================================

/// A cold first download (fresh `HF_HOME`) with fully piped output (no TTY)
/// must (a) print the Spanish human hint naming the approximate blob size,
/// and (b) emit both structured `resolve_model_assets` events into the
/// `--trace-file` JSONL, parsed structurally — never by substring-guessing.
#[tokio::test]
#[ignore = "requires network: performs a real cold pull (~390 MB) into an isolated HF_HOME"]
async fn cold_pull_emits_structured_events_without_tty() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    // Fresh HF_HOME guarantees a cold pull; `.output()` pipes stdout AND
    // stderr, so `is_terminal()` is false by construction.
    let cold_hf_home = tempfile::tempdir().expect("create cold HF_HOME tempdir");
    let trace_path = t.out.path().join("trace.jsonl");

    let mut cmd = std::process::Command::new(crate::common::webfang_path());
    cmd.arg("--url")
        .arg(t.server.uri())
        .arg("--output")
        .arg(t.out.path())
        .arg("--single-page")
        .arg("--clean-ai")
        .arg("--trace-file")
        .arg(&trace_path)
        .arg("--quiet")
        .env("HF_HOME", cold_hf_home.path())
        .env(
            webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
            "1",
        );
    let xdg_cache = t.out.path().join("hermetic-cache");
    let _ = std::fs::create_dir_all(&xdg_cache);
    cmd.env("XDG_CACHE_HOME", &xdg_cache);

    let output = cmd.output().expect("spawn webfang for cold pull");
    assert!(
        output.status.success(),
        "cold-pull run must succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Human hint: fires exactly because stderr is NOT a terminal here
    // (a TTY run is covered by hf_hub's own progress bar).
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("primera vez"),
        "the Spanish cold-download hint must fire on the non-TTY path: stderr={stderr}"
    );

    // Snapshot rule: behavioral stderr is asserted via a redacted snapshot,
    // not bare `contains` (the hint line survives redaction).
    let stderr_owned = stderr.replace(cold_hf_home.path().to_string_lossy().as_ref(), "<HF_HOME>");
    crate::assert_snapshot_redacted("cold_pull_stderr", t.out.path(), stderr_owned);

    // Structural trace assertions on the JSONL (see the helper for the
    // exact contract it pins: repo/offline_mode on the entry event,
    // repo/bytes/elapsed_ms/cached on the summary event).
    assert_resolve_events_in_trace(&trace_path);
}

/// Assert the `--trace-file` JSONL contains both #1316 structured events
/// with their fields, parsed structurally (never by substring-guessing).
///
/// Events carry `message` at the top level and structured fields nested
/// under `fields`. `span_close` records share span names but never carry a
/// `message`, so matching on `message` alone already excludes them; the
/// explicit guard documents the intent.
fn assert_resolve_events_in_trace(trace_path: &Path) {
    assert!(
        trace_path.exists(),
        "trace.jsonl must be created at {}",
        trace_path.display()
    );
    let body = std::fs::read_to_string(trace_path).expect("read trace.jsonl");
    assert!(!body.trim().is_empty(), "trace.jsonl must not be empty");

    let mut found_resolve = false;
    let mut found_resolved = false;
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let record: serde_json::Value =
            serde_json::from_str(line).expect("each trace line must be valid JSON");
        if record.get("record").and_then(|v| v.as_str()) == Some("span_close") {
            continue;
        }
        let message = record.get("message").and_then(|v| v.as_str());
        match message {
            Some("resolving AI model assets") => {
                let fields = record.get("fields").unwrap_or(&serde_json::Value::Null);
                assert!(
                    fields.get("repo").and_then(|v| v.as_str()).is_some(),
                    "entry event must carry the repo field: {record}"
                );
                assert_eq!(
                    fields.get("offline_mode").and_then(|v| v.as_bool()),
                    Some(false),
                    "cold-pull entry event must record offline_mode=false: {record}"
                );
                found_resolve = true;
            },
            Some("AI model assets resolved") => {
                let fields = record.get("fields").unwrap_or(&serde_json::Value::Null);
                assert!(
                    fields.get("repo").and_then(|v| v.as_str()).is_some(),
                    "summary event must carry the repo field: {record}"
                );
                let bytes = fields
                    .get("bytes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or_default();
                assert!(
                    bytes > 0,
                    "summary event must carry a positive model blob size: {record}"
                );
                let elapsed_ms = fields
                    .get("elapsed_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or_default();
                assert!(
                    elapsed_ms > 0,
                    "summary event must carry a positive elapsed_ms: {record}"
                );
                assert_eq!(
                    fields.get("cached").and_then(|v| v.as_bool()),
                    Some(false),
                    "cold pull must be reported as cached=false: {record}"
                );
                found_resolved = true;
            },
            _ => {},
        }
    }
    assert!(
        found_resolve,
        "trace.jsonl must contain the 'resolving AI model assets' event"
    );
    assert!(
        found_resolved,
        "trace.jsonl must contain the 'AI model assets resolved' event"
    );
}

//! `--retention-days` end-of-run pruning (#1827 slice B).
//!
//! Behavioral contract, pinned through the CLI boundary:
//! - default (`0`) is a no-op: nothing is ever pruned unless the operator
//!   opts in,
//! - with `N > 0` a successful run prunes exports older than N days from
//!   the output root while keeping everything written by this run,
//! - the batch flow (`--batch-file`) runs the same retention pass,
//! - the summary line is user-facing and deterministic (counts + window).

#[path = "common/mod.rs"]
mod common;

use std::time::{Duration, SystemTime};

use common::cli_harness::BehavioralTest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const PAGE_BODY: &str = "<html><body><article>\
     <h1>Retention Fixture</h1>\
     <p>This body carries enough substantive text to pass the minimum-content guard.</p>\
     </article></body></html>";

/// Age a file's mtime into the past via std (`File::set_times`, stable
/// since 1.75) — no extra dev-dependency needed.
fn age_file(path: &std::path::Path, days_ago: u64) {
    let f = std::fs::File::options()
        .append(true)
        .open(path)
        .expect("open planted file");
    let past = SystemTime::now() - Duration::from_secs(days_ago * 86_400);
    f.set_times(std::fs::FileTimes::new().set_modified(past))
        .expect("set aged mtime");
}

fn plant_aged_export(t: &BehavioralTest, days_ago: u64) -> std::path::PathBuf {
    let aged = t.out.path().join("aged-export.md");
    std::fs::write(&aged, "stale run output").expect("plant aged file");
    age_file(&aged, days_ago);
    aged
}

async fn healthy_page(t: &BehavioralTest) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_BODY))
        .mount(&t.server)
        .await;
}

/// The deterministic `Retention:` summary line the run prints on stdout.
fn retention_line(stdout: &str) -> String {
    stdout
        .lines()
        .find(|l| l.starts_with("Retention:"))
        .expect("run must print the retention summary line")
        .to_owned()
}

#[tokio::test]
async fn opt_in_retention_prunes_aged_exports_and_keeps_this_run() {
    let t = BehavioralTest::new().await;
    healthy_page(&t).await;
    let aged = plant_aged_export(&t, 30);

    let output = t
        .scraper_cmd()
        .arg("--retention-days")
        .arg("7")
        .output()
        .expect("run webfang");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(
        !aged.exists(),
        "the 30-day-old file must be pruned by --retention-days 7"
    );
    assert!(
        t.find_files("md").len() >= 1,
        "this run's fresh export must survive retention"
    );
    insta::assert_snapshot!(
        retention_line(&stdout),
        @"Retention: 1 archivo(s) y 0 fila(s) de la vault DB purgados (> 7 días)."
    );
}

#[tokio::test]
async fn default_retention_is_disabled_and_never_prunes() {
    let t = BehavioralTest::new().await;
    healthy_page(&t).await;
    let aged = plant_aged_export(&t, 30);

    let output = t.scraper_cmd().output().expect("run webfang");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(
        aged.exists(),
        "without --retention-days nothing may ever be pruned"
    );
    assert!(
        !stdout.contains("Retention:"),
        "disabled retention must print no summary"
    );
}

#[tokio::test]
async fn batch_flow_applies_the_same_retention_pass() {
    let t = BehavioralTest::new().await;
    healthy_page(&t).await;
    let aged = plant_aged_export(&t, 30);
    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", t.server.uri())).expect("write batch file");

    let output = t
        .scraper_cmd()
        .arg("--batch-file")
        .arg(&batch_file)
        .arg("--retention-days")
        .arg("7")
        .output()
        .expect("run webfang batch");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(
        !aged.exists(),
        "the batch flow must run the same retention pass"
    );
    insta::assert_snapshot!(
        retention_line(&stdout),
        @"Retention: 1 archivo(s) y 0 fila(s) de la vault DB purgados (> 7 días)."
    );
}

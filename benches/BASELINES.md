# Benchmark Baselines

Reference numbers for the 9 criterion suites (`benches/*.rs`, wired via
`[[bench]]` in `crates/webfang_core/Cargo.toml`).

**Provenance:** GitHub Actions `ubuntu-latest` runner, produced by the nightly
[`benches`](../.github/workflows/benches.yml) workflow (issue #958). Numbers are
for **trend/regression detection between runs on the same runner image** — not
absolute microarchitecture claims. Refresh by dispatching the workflow manually
and copying the summary below.

| Field | Value |
| --- | --- |
| Baseline run | _pending first nightly / manual dispatch_ |
| Workflow run | _pending_ |
| rustc | _pending_ |
| Runner CPU | _pending_ |

<!-- Fill per-bench tables from the workflow's criterion-reports artifact.
     Keep one table per bench file; rows are benchmark groups/ids as reported
     by criterion (mean estimate). -->

## cosine_similarity

_Pending baseline run._

## export

_Pending baseline run._

## html_conversion

_Pending baseline run._

## link_extraction

_Pending baseline run._

## readability

_Pending baseline run._

## sitemap_parsing

_Pending baseline run._

## url_parsing

_Pending baseline run._

## waf_detection

_Pending baseline run._

## tracing_overhead

_Pending baseline run._

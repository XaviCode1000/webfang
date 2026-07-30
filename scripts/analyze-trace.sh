#!/usr/bin/env bash
#
# analyze-trace.sh — query cookbook for WebFang FileTraceLayer JSONL output.
#
# Usage:
#   scripts/analyze-trace.sh <trace.jsonl> <command> [args]
#
# Generate a trace first:
#   webfang --url https://example.com --trace-file debug.jsonl -vvv
#
# Commands:
#   errors                 All ERROR events with url/stage/error context
#   slow [N]               Top N slowest spans (default 20)
#   stages                 Time/count distribution per pipeline stage
#   progress               Crawl progress events over time
#   summary                Final "crawl completed" summary
#   counts                 Operation counts by span type
#   urls-failed            Unique URLs that produced an ERROR
#   trace <trace_id>       Reconstruct one operation by trace_id
#   waf                    WAF challenges and banned-domain events
#
# Requires: jq
set -euo pipefail

if [[ $# -lt 2 ]]; then
  grep '^#' "$0" | sed 's/^# \{0,1\}//'
  exit 1
fi

FILE="$1"
CMD="$2"
shift 2

if [[ ! -f "$FILE" ]]; then
  echo "error: trace file not found: $FILE" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq is required (https://jqlang.github.io/jq/)" >&2
  exit 1
fi

case "$CMD" in
  errors)
    jq -c 'select(.level == "ERROR") | {target, url: .fields.url, stage: .fields.stage, error: .fields.error, msg: .fields.message}' "$FILE"
    ;;
  slow)
    N="${1:-20}"
    jq -r 'select(.span_duration_ms != null) | [.span_duration_ms, .span] | @tsv' "$FILE" | sort -rn | head -n "$N"
    ;;
  stages)
    jq -r 'select(.span == "pipeline_stage") | .fields.stage' "$FILE" | sort | uniq -c | sort -rn
    ;;
  progress)
    jq -c 'select(.fields.message? == "crawl progress") | {pages: .fields.pages_crawled, pct: .fields.progress_pct, eta_s: .fields.eta_secs}' "$FILE"
    ;;
  summary)
    jq -c 'select(.fields.message? == "crawl completed")' "$FILE"
    ;;
  counts)
    jq -r '.span // "event"' "$FILE" | sort | uniq -c | sort -rn
    ;;
  urls-failed)
    jq -r 'select(.level == "ERROR") | .fields.url // empty' "$FILE" | sort -u
    ;;
  trace)
    TRACE="${1:-}"
    if [[ -z "$TRACE" ]]; then
      echo "error: 'trace' requires a <trace_id> argument" >&2
      exit 1
    fi
    jq -c "select(.trace_id == \"$TRACE\" or ((.fields.trace_id? // \"\") | contains(\"$TRACE\")))" "$FILE"
    ;;
  waf)
    jq -c 'select((.fields.message? // "") | test("WAF|Banned domain"; "i"))' "$FILE"
    ;;
  *)
    echo "error: unknown command: $CMD" >&2
    grep '^#   ' "$0" | sed 's/^#   /  /'
    exit 1
    ;;
esac

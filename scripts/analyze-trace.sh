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
#   pacing                 Rate-limit wait cost per pacing scope (issue #1610)
#   pacing-hot [N]         Top N URLs by rate-limit wait time (default 10)
#   admission              Admission/dispatch wait cost per operation (#1610)
#   percentiles            p50/p95/p99 of scrape durations, from the JSONL (#1610)
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
    jq -c 'select(.level == "ERROR") | {target, url: .fields.url, stage: .fields.stage, error: .fields.error, msg: .message}' "$FILE"
    ;;
  slow)
    N="${1:-20}"
    jq -r 'select(.span_duration_ms != null) | [.span_duration_ms, .span] | @tsv' "$FILE" | sort -rn | head -n "$N"
    ;;
  stages)
    jq -r 'select(.span != null and .record != "span_close") | .span' "$FILE" | sort | uniq -c | sort -rn
    ;;
  progress)
    jq -c 'select(.message == "crawl progress") | {pages: .fields.pages_crawled, pct: .fields.progress_pct, eta_s: .fields.eta_secs}' "$FILE"
    ;;
  summary)
    jq -c 'select(.message == "crawl completed")' "$FILE"
    ;;
  counts)
    jq -r 'select(.record != "span_close") | .span // "event"' "$FILE" | sort | uniq -c | sort -rn
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
    jq -c "select(.trace_id == \"$TRACE\")" "$FILE"
    ;;
  waf)
    jq -c 'select((.fields.message? // "") | test("WAF|Banned domain"; "i"))' "$FILE"
    ;;
  # ------------------------------------------------------------------
  # Waiting and admission surfaces (issue #1610)
  #
  # These events are DEBUG level, so `-vvv` is NOT required: the
  # FileTraceLayer always runs at TRACE, so they are in the JSONL
  # whenever --trace-file was passed.
  # ------------------------------------------------------------------
  pacing)
    # "Is this run slow because of the network, or because we paced it?"
    # One row per pacing scope, with the share of waits that were shed.
    jq -s 'map(select(.message == "rate limit wait"))
      | group_by(.fields.scope)
      | map({
          scope: .[0].fields.scope,
          waits: length,
          total_waited_ms: (map(.fields.waited_ms // 0) | add),
          max_waited_ms: (map(.fields.waited_ms // 0) | max),
          cancelled: (map(select(.fields.outcome == "cancelled")) | length)
        })' "$FILE"
    ;;
  pacing-hot)
    N="${1:-10}"
    # Which targets absorbed the most pacing time (OBS-H2: the wait is
    # attributed to the URL whose permit it was waiting for).
    jq -r 'select(.message == "rate limit wait") | [(.fields.waited_ms // 0), .fields.scope, (.fields.url // "unknown")] | @tsv' "$FILE" \
      | awk -F'\t' '{t[$2 FS $3] += $1} END {for (k in t) print t[k] "\t" k}' \
      | sort -rn | head -n "$N"
    ;;
  percentiles)
    # The reconstruction query for OBS-M4: the same nearest-rank percentiles
    # the in-process reservoir publishes, computed over the whole trace file.
    # `message` is TOP-LEVEL in this JSONL - selecting `.fields.message`
    # returns nothing and reports `samples: 0`, which reads like "no scrapes"
    # rather than like a broken query.
    jq -s '[ .[] | select(.message? == "scrape recorded")
             | .fields.duration_ms ] | sort
       | . as $s | ($s|length) as $n
       | def nr($p): if $n == 0 then null
                     else $s[((((($p*$n)+99)/100)|floor) - 1)] end;
         { samples: $n, p50_ms: nr(50), p95_ms: nr(95), p99_ms: nr(99) }' "$FILE"
    ;;
  admission)
    # Every semaphore/pool admission wait across the three transports, one
    # row per `operation`: the downloader governor, the AI inference pool
    # permit and the AI pool dispatch queue.
    jq -s 'map(select(.message == "semaphore acquire"
                       or .message == "inference pool permit"
                       or .message == "inference request queued for a worker"
                       or .message == "inference worker took a request"))
      | group_by(.fields.operation)
      | map({
          operation: .[0].fields.operation,
          events: length,
          total_waited_ms: (map(.fields.waited_ms // 0) | add),
          max_waited_ms: (map(.fields.waited_ms // 0) | max),
          max_in_flight: (map(.fields.in_flight // 0) | max)
        })' "$FILE"
    ;;
  *)
    echo "error: unknown command: $CMD" >&2
    grep '^#   ' "$0" | sed 's/^#   /  /'
    exit 1
    ;;
esac

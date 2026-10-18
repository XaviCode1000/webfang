jq -r 'select(.span == "crawl_page" and .record == "span_close")
       | [.span_fields.outcome, .span_fields.rate_limit_wait_ms, .span_duration_ms] | @tsv' \
  debug.jsonl | sort -k2 -rn | head

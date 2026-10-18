# 1. By identity — the wait carries the same correlation_id the page span does.
CID=00-01949e0e8b8e70008000000000000001-0000000000000042-01
jq -c "select(.fields.correlation_id? == \"$CID\")" debug.jsonl

# 2. By span field — crawl_page records the wait it absorbed.
jq -c 'select(.span == "crawl_page" and .record == "span_close")
       | {url: .span_fields.url, outcome: .span_fields.outcome,
          paced_ms: .span_fields.rate_limit_wait_ms,
          total_ms: .span_duration_ms}' debug.jsonl

jq -r 'select(.span_duration_ms != null) | [.span_duration_ms, .span] | @tsv' debug.jsonl | sort -rn | head -20

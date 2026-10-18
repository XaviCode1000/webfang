jq -r 'select(.record != "span_close") | .span // "event"' debug.jsonl | sort | uniq -c | sort -rn

jq -r 'select(.level == "ERROR") | .fields.url // empty' debug.jsonl | sort -u

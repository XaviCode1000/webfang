ROOT=0000000000000001
jq -c "select(.trace_id == \"$ROOT\")" debug.jsonl

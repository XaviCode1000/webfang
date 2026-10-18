CID=00-01949e0e8b8e70008000000000000001-0000000000000042-01
jq -c "select(.fields.correlation_id? == \"$CID\" and .message? == \"rate limit wait\")" debug.jsonl

jq -r 'select(.span == "pipeline_stage") | .fields.stage' debug.jsonl | sort | uniq -c | sort -rn

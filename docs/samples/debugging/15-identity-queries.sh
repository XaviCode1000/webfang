# Reconstruct an entire run by the single top-level trace_id (root span Id)
ROOT=0000000000000001
jq -c "select(.trace_id == \"$ROOT\")" debug.jsonl

# Same run by the durable run-root UUID (CorrelationId in span_fields)
CUUID=01949e0e-8b8e-7000-8000-000000000001
jq -c "select(.span_fields.trace_id == \"$CUUID\")" debug.jsonl

# The run-root identity (the `run identity` event carries it in .fields)
jq -c 'select(.message? == "run identity") | .fields' debug.jsonl

# Every page identity present in the trace
jq -r '.span_fields.correlation_id // empty' debug.jsonl | sort -u

# Reconstruct one page's scrape by its correlation_id
CID=00-01949e0e8b8e70008000000000000001-0000000000000042-01
jq -c "select(.span_fields.correlation_id? == \"$CID\")" debug.jsonl

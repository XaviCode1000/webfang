jq -c 'select(.level == "ERROR") | {target, url: .fields.url, stage: .fields.stage, error: .fields.error, msg: .fields.message}' debug.jsonl

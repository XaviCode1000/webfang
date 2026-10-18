jq -c 'select(.fields.message? == "crawl progress") | {pages: .fields.pages_crawled, pct: .fields.progress_pct, eta_s: .fields.eta_secs}' debug.jsonl

# Full trace + verbose logging
webfang --url https://example.com --trace-file debug.jsonl -vvv

# Batch / crawl
webfang --url https://example.com --max-pages 100 --trace-file crawl.jsonl -v

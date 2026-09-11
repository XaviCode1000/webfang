#!/usr/bin/env python3
"""Mode-D MCP probe: raw tool-call dump."""
import json
import sys
import urllib.request

URL = "http://127.0.0.1:18777/mcp"


def post(payload, session=None):
    req = urllib.request.Request(
        URL,
        data=json.dumps(payload).encode(),
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            **({"mcp-session-id": session} if session else {}),
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=180) as resp:
        sid = resp.headers.get("mcp-session-id")
        body = resp.read().decode()
    obj = None
    for line in body.splitlines():
        if line.startswith("data:") and line[5:].strip().startswith("{"):
            obj = json.loads(line[5:].strip())
    if obj is None and body.strip():
        obj = json.loads(body)
    return obj, sid


_, sid = post(
    {
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "moded", "version": "0.1"},
        },
    }
)
try:
    post({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}, sid)
except Exception:  # noqa: BLE001
    pass

name = sys.argv[1]
args = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
out, _ = post(
    {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}},
    sid,
)
res = out["result"]
txt = "".join(c.get("text", "") for c in res.get("content", []))
print("isError:", res.get("isError"))
try:
    p = json.loads(txt)
except Exception:  # noqa: BLE001
    print("text:", txt[:400])
    sys.exit(0)
if isinstance(p, dict) and "documents" in p:
    print("summary:", {k: v for k, v in p.items() if k != "documents"})
    d0 = p["documents"][0]
    print("doc keys:", list(d0.keys()))
    lens = sorted({len(d.get("embeddings") or []) for d in p["documents"]})
    print("embedding lens per chunk:", lens)
    for i, d in enumerate(p["documents"][:2]):
        print(f"  [{i}] meta={ {k: d.get(k) for k in ('chunk_index','total_chunks','relevance_score','metadata') if k in d} } text={ (d.get('content') or '')[:70]!r}")
else:
    print(json.dumps(p, indent=2)[:600])

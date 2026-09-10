#!/usr/bin/env python3
"""Mode-D cancellation probe: deliver SIGINT/SIGTERM to webfang without the
bash-background SIG_IGN artifact, and report what the process actually did."""
import json
import os
import re
import signal
import subprocess
import sys
import time

BIN = "/tmp/moded/bin/webfang-ai"
ENV = {
    **os.environ,
    "WEBFANG_DISABLE_SSRF_ENTRY_GUARD": "1",
    "WEBFANG_DISABLE_SSRF_RESOLVER": "1",
    "WEBFANG_DISABLE_SSRF_REDIRECT_GUARD": "1",
}
SIG = getattr(signal, sys.argv[1] if len(sys.argv) > 1 else "SIGINT")
MODE = sys.argv[2] if len(sys.argv) > 2 else "ai"  # ai | noai
DELAY = int(sys.argv[3]) if len(sys.argv) > 3 else 4000
OUT = f"/tmp/moded/out/CANCEL-{SIG.name}-{MODE}-{int(time.time())}"

args = [BIN, "--url", "http://127.0.0.1:18991/many.html", "--max-pages", "12",
        "--delay-ms", str(DELAY), "--rate-limit-burst", "1", "-vv", "-o", OUT]
if MODE == "ai":
    args.insert(1, "--clean-ai")

os.makedirs(OUT, exist_ok=True)
with open(f"{OUT}/stdout.txt", "w") as so, open(f"{OUT}/stderr.txt", "w") as se:
    t0 = time.time()
    p = subprocess.Popen(args, stdout=so, stderr=se, env=ENV,
                         # new session -> no inherited SIG_IGN for INT
                         start_new_session=True)
    time.sleep(12)
    send_t = time.time()
    print(f"delivering {SIG.name} at t={send_t - t0:.1f}s pid={p.pid}")
    p.send_signal(SIG)
    try:
        rc = p.wait(timeout=180)
        print(f"process ended: rc={rc} (raw wait status) latency={time.time() - send_t:.1f}s")
    except subprocess.TimeoutExpired:
        p.kill()
        print(f"STILL RUNNING after 180s -> killed; latency={time.time() - send_t:.1f}s")

body = open(f"{OUT}/stderr.txt", errors="replace").read()
print("--- signal-related log lines ---")
for pat in ("received SIGTERM", "Received SIGTERM", "received SIGINT", "Received SIGINT",
            "graceful shutdown", "draining", "cancelling", "skipped"):
    for m in re.finditer(pat, body, re.I):
        line = body[max(0, body.rfind("\n", 0, m.start()) + 1):body.find("\n", m.start())]
        print("  LOG:", line[:150])
        break
try:
    with open(f"{OUT}/export.jsonl") as fh:
        n = sum(1 for _ in fh)
except OSError:
    n = None
print(f"export records: {n}")
print(f"panics: {body.lower().count('panic')}")
done = re.findall(r"Finished: (\d+) total, (\d+) succeeded, (\d+) failed", body)
print("final tally:", done[-1] if done else "none")

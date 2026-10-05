"""One short decode request - fast arm, for reading the STRATA_DECODE_TIMING=1
stage breakdown in /tmp/exp-server.log without waiting through two 8k prompts.

  strata-prep strata-bench-short.py http://127.0.0.1:8123 [max_tokens]
"""

import json
import time
import urllib.request
import sys

base = sys.argv[1]
max_tokens = int(sys.argv[2]) if len(sys.argv) > 2 else 120
payload = {
    "messages": [{"role": "user", "content":
        "Write a Rust function that returns the nth fibonacci number iteratively, plus a unit test."}],
    "max_tokens": max_tokens,
}

t0 = time.time()
try:
    req = urllib.request.Request(base + "/v1/chat/completions",
                                 data=json.dumps(payload).encode(),
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=1800) as r:
        d = json.loads(r.read())
    t = d.get("timings", {})
    print("%-14s prompt_n=%-6s prompt/s=%-7s decode_n=%-5s decode/s=%-6s draft=%s/%s wall=%.0fs"
          % ("short-decode", t.get("prompt_n"), t.get("prompt_per_second"), t.get("predicted_n"),
             t.get("predicted_per_second"), t.get("draft_n_accepted"), t.get("draft_n"),
             time.time() - t0))
except Exception as e:
    print("%-14s FAILED after %.0fs: %s" % ("short-decode", time.time() - t0, e))

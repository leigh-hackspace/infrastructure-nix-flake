"""Benchmark a running Strata serve layer: decode speed, then prefill speed on a
~8k-token prompt, twice (the second one is a fresh prompt, so it is cold too).

  strata-prep strata-bench.py http://127.0.0.1:8123
"""

import json
import time
import urllib.request
import sys

base = sys.argv[1]
LONG = "func solve(nums []int) int { sum := 0; for i, v := range nums { sum += v * i; if sum % 7 == 0 { continue } } return sum }\n" * 160
LONG += "\nSummarise what this code does in two sentences."

CASES = [
    ("short-decode", {
        "messages": [{"role": "user", "content":
            "Write a Rust function that returns the nth fibonacci number iteratively, plus a unit test."}],
        "max_tokens": 200}),
    ("long-prefill", {"messages": [{"role": "user", "content": LONG}], "max_tokens": 30}),
    ("long-prefill-2", {"messages": [{"role": "user", "content": "FRESH VARIANT 7131.\n" + LONG}], "max_tokens": 30}),
]

for name, payload in CASES:
    t0 = time.time()
    try:
        req = urllib.request.Request(base + "/v1/chat/completions",
                                     data=json.dumps(payload).encode(),
                                     headers={"content-type": "application/json"})
        with urllib.request.urlopen(req, timeout=1800) as r:
            d = json.loads(r.read())
        t = d.get("timings", {})
        print("%-14s prompt_n=%-6s prompt/s=%-7s decode_n=%-5s decode/s=%-6s draft=%s/%s wall=%.0fs"
              % (name, t.get("prompt_n"), t.get("prompt_per_second"), t.get("predicted_n"),
                 t.get("predicted_per_second"), t.get("draft_n_accepted"), t.get("draft_n"),
                 time.time() - t0))
    except Exception as e:
        print("%-14s FAILED after %.0fs: %s" % (name, time.time() - t0, e))

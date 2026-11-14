"""Benchmark a running Strata serve layer: decode speed, then prefill speed on a
~8k-token prompt, twice (both prompts fresh, so both are cold reads).

  strata-prep strata-bench.py http://127.0.0.1:8123

The variant marker is RANDOM, and it has to be.  This box runs
--conversation-cache-mib 1536 --conversation-cache-min-tokens 2048, so a prompt it has
already seen is *parked* and the next identical one is answered from the parked identity:
measured 2026-11-14, the engine's own line for a repeat of this script's fixed 6,943-token
prompt was `prompt 6943 tokens = 6938 reused + 5 read in 294 ms (17.0 tok/s)`.  A second
run of the old script therefore reported 17.0 tok/s of "prefill" that was a cache hit, and
prompt_n told the truth about it in a column nobody reads.  `prompt_n` is the check: a
long-prefill arm that does not print ~6950 measured no read at all.
"""

import json
import random
import time
import urllib.request
import sys

base = sys.argv[1]
LONG = "func solve(nums []int) int { sum := 0; for i, v := range nums { sum += v * i; if sum % 7 == 0 { continue } } return sum }\n" * 160
LONG += "\nSummarise what this code does in two sentences."

# Distinct per process AND per case, so neither a repeat run nor case 2 vs case 1 hits the cache.
def fresh(tag):
    return "FRESH VARIANT %s %d.\n" % (tag, random.randrange(1 << 40))

CASES = [
    ("short-decode", {
        "messages": [{"role": "user", "content": fresh("decode") +
            "Write a Rust function that returns the nth fibonacci number iteratively, plus a unit test."}],
        "max_tokens": 200}),
    ("long-prefill", {"messages": [{"role": "user", "content": fresh("a") + LONG}], "max_tokens": 30}),
    ("long-prefill-2", {"messages": [{"role": "user", "content": fresh("b") + LONG}], "max_tokens": 30}),
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

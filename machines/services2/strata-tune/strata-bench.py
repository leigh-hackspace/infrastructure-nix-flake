#!/usr/bin/env python3
"""Benchmark a running Strata serve layer on services2 (the CUDA/sm_86 engine).

The same shape as machines/aibox/strata-tune/strata-bench.py, with the arm this
box actually needs: prefill measured at 8k AND at ~28k tokens, because a bare
`--prefill auto` took an 8192-token chunk ceiling on NVIDIA at the previous input
(0.1.56) and takes 32768 on any card of compute capability >= 8.0 at 0.1.59.1
(`include/strata/core/nv_chunk.hpp`) - so a 28K prompt is one chunk instead of
four, and the only way to see what that costs or buys here is to time it.

Every prompt is a fresh variant (the marker line is random per run). This box runs
`--conversation-cache-mib 0`, so a repeat would be re-read here anyway - but the same
script crossed to aibox, which does have the cache parked on, and answered a repeat with
`prompt 6943 tokens = 6938 reused + 5 read in 294 ms (17.0 tok/s)`: 17 "tok/s" of no read.
A bench must not depend on which cache dials the box under test has, so it asks in fresh
prompts and prints `prompt_n` first.

  # through the front door, which injects the engine's api_key
  python3 strata-bench.py https://strata.int.leighhack.org

  # or straight at the engine, with the key in the environment
  STRATA_BENCH_KEY=... python3 strata-bench.py http://10.3.1.21:8080

Decode is read off the engine's own `done:` line in /var/lib/strata/strata.log
too - `timings.predicted_per_second` and that line have agreed on this box.
"""

import json
import os
import random
import sys
import time
import urllib.request

base = (sys.argv[1] if len(sys.argv) > 1 else "https://strata.int.leighhack.org").rstrip("/")
key = os.environ.get("STRATA_BENCH_KEY")

# Each LINE tokenises to about 43 tokens, so ask in lines computed from that.
LINE = ("func solve(nums []int) int { sum := 0; for i, v := range nums { sum += v * i; "
        "if sum % 7 == 0 { continue } } return sum }\n")
TOK_PER_LINE = 43


def code_prompt(n_tokens):
    marker = "VARIANT %d %d.\n" % (random.randrange(10**9), random.randrange(10**9))
    return marker + LINE * max(1, n_tokens // TOK_PER_LINE) + \
        "\nHow many lines of Go are in this file? Answer with the number."


CASES = [
    # name, prompt tokens (0 = the fixed short prompt), max_tokens
    ("short-decode", 0, 200),
    ("prefill-8k", 8000, 8),
    ("prefill-8k-2", 8000, 8),
    ("prefill-28k", 28000, 8),
    ("prefill-28k-2", 28000, 8),
]

for name, n_tokens, max_tokens in CASES:
    content = ("Write a Rust function that returns the nth fibonacci number iteratively, "
               "plus a unit test." if n_tokens == 0 else code_prompt(n_tokens))
    payload = {"messages": [{"role": "user", "content": content}], "max_tokens": max_tokens}
    headers = {"content-type": "application/json"}
    if key:
        headers["authorization"] = "Bearer " + key
    t0 = time.time()
    try:
        req = urllib.request.Request(base + "/v1/chat/completions",
                                    data=json.dumps(payload).encode(), headers=headers)
        with urllib.request.urlopen(req, timeout=3600) as r:
            d = json.loads(r.read())
        t = d.get("timings") or {}
        print("%-14s prompt_n=%-6s prompt/s=%-9s decode_n=%-4s decode/s=%-7s draft=%s/%s wall=%.0fs"
              % (name, t.get("prompt_n"),
                 ("%.1f" % t["prompt_per_second"]) if t.get("prompt_per_second") else "-",
                 t.get("predicted_n"),
                 ("%.1f" % t["predicted_per_second"]) if t.get("predicted_per_second") else "-",
                 t.get("draft_n_accepted"), t.get("draft_n"), time.time() - t0), flush=True)
    except Exception as e:                                    # noqa: BLE001
        print("%-14s FAILED after %.0fs: %s" % (name, time.time() - t0, e), flush=True)

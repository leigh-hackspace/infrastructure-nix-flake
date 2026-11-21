#!/usr/bin/env python3
"""The fast arm bench for services2: three short decodes back to back and one
8k prefill, so an arm costs ~40 s of engine-down time instead of ~80 s, and the
prompt is built by the same code as strata-bench.py so arms and full passes are
comparable.

  strata-prep strata-bench-fast.py http://127.0.0.1:8123

Three decode reps, not one: this box's decode spread across *identical* arms is
about +-20% (measured 2026-11-14 on one build: 24.4 / 30.9 / 31.0 tok/s), so a
single reading cannot tell an arm from noise - the median of three can, and the
spread it prints is the thing to compare an arm's delta against.

Prompts carry a random marker per rep so no arm is a warm read regardless of
which cache dials the box under test has; `prompt_n` is the check, and a prefill
arm that does not print about 8000 measured no read at all (aibox's bench once
reported 17 "tok/s" of a 5-token read).
"""

import json
import random
import statistics
import sys
import time
import urllib.request

base = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8123").rstrip("/")
DECODE_REPS = int(sys.argv[2]) if len(sys.argv) > 2 else 3
PREFILL_TOKENS = int(sys.argv[3]) if len(sys.argv) > 3 else 8000

# Identical to strata-bench.py's, on purpose (that file's comment owns the number).
LINE = ("func solve(nums []int) int { sum := 0; for i, v := range nums { sum += v * i; "
        "if sum % 7 == 0 { continue } } return sum }\n")
TOK_PER_LINE = 43


def code_prompt(n_tokens):
    marker = "VARIANT %d %d.\n" % (random.randrange(10**9), random.randrange(10**9))
    return marker + LINE * max(1, n_tokens // TOK_PER_LINE) + \
        "\nHow many lines of Go are in this file? Answer with the number."


def ask(content, max_tokens):
    payload = {"messages": [{"role": "user", "content": content}], "max_tokens": max_tokens}
    req = urllib.request.Request(base + "/v1/chat/completions", data=json.dumps(payload).encode(),
                                 headers={"content-type": "application/json"})
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=3600) as r:
        d = json.loads(r.read())
    return d.get("timings") or {}, time.time() - t0


def line(name, t, wall):
    def f(x):
        return ("%.1f" % x) if isinstance(x, (int, float)) else "-"
    return ("%-13s prompt_n=%-6s prompt/s=%-8s decode_n=%-4s decode/s=%-6s draft=%s/%s wall=%.0fs"
            % (name, t.get("prompt_n"), f(t.get("prompt_per_second")), t.get("predicted_n"),
               f(t.get("predicted_per_second")), t.get("draft_n_accepted"), t.get("draft_n"), wall))


rates = []
for i in range(DECODE_REPS):
    t0 = time.time()
    try:
        t, wall = ask("Write a Rust function that returns the nth fibonacci number iteratively, "
                      "plus a unit test. VARIANT %d." % random.randrange(10**9), 200)
        if isinstance(t.get("predicted_per_second"), (int, float)):
            rates.append(t["predicted_per_second"])
        print(line("decode-%d" % (i + 1), t, wall), flush=True)
    except Exception as e:                                    # noqa: BLE001
        print("decode-%-7d FAILED after %.0fs: %s" % (i + 1, time.time() - t0, e), flush=True)
if rates:
    print(">>> decode all-reps median %.1f tok/s (n=%d, spread %.1f-%.1f)"
          % (statistics.median(rates), len(rates), min(rates), max(rates)), flush=True)
if len(rates) > 2:
    # The first request after a load is this box's warm-up arm: measured 2026-11-14,
    # 24.2 / 31.9 / 33.3 on the same build, first arm last.  The steady-state number is
    # the median of the rest; the gap to rep 1 is the per-request cold cost, not noise.
    rest = rates[1:]
    print(">>> decode steady median %.1f tok/s (reps 2-%d: %s)"
          % (statistics.median(rest), len(rates), " ".join("%.1f" % r for r in rest)), flush=True)

t0 = time.time()
try:
    t, wall = ask(code_prompt(PREFILL_TOKENS), 8)
    print(line("prefill-8k", t, wall), flush=True)
except Exception as e:                                        # noqa: BLE001
    print("prefill-8k    FAILED after %.0fs: %s" % (time.time() - t0, e), flush=True)

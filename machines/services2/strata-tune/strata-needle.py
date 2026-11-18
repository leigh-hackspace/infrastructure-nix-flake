#!/usr/bin/env python3
"""Does a long context on services2 actually WORK, or is it only advertised?

  python3 strata-needle.py https://strata.int.leighhack.org [target_tokens]

`--max-context` says how much the engine will *accept*.  It does not say whether
anything is still readable at the top of that range, and on this box raising it is
a real trade (the KV's bytes come out of the expert cache and the prompt chunk —
machines/services2/STRATA.md, "Context and the KV cache").  So the number that
matters is: bury two unique records in a long prompt, one shallow and one deep,
and can it give both back?

Records are ~28 tokens each and each one's `serial=` and `value=` are drawn from a
**fixed** seed, so two arms of a comparison send byte-identical haystacks and only
the config differs.  `target_tokens` is a target, not what you get: the tokeniser
decides (asking for 200000 arrives as `prompt_n=183987`).  Read `prompt_n` in the
output and compare arms at the same `prompt_n`, not the same argument.

What it is not:

- **not a quality score.** Two facts out of one prompt is a retrieval check, not a
  benchmark.  `strata-answer-score.py` is the answer-key gate for answers.
- **not a speed harness.** The `prompt/s` it prints is one cold read of one long
  prompt, which is the number worth having at long range, but the arm's shape is
  `strata-bench.py`'s job.
- **not immune to the thinking budget.** This model answers after reasoning, and
  `max_tokens` covers both.  If the budget runs out there is no `content`, so the
  search falls back to the reasoning tail and prints `finish=` and `reasoning=` —
  a miss with `finish=length` is the budget, not the context.  Raise the argument
  below rather than reporting 0/2.

Measured 2026-11-18, engine 0.1.61, the Coder IQ1_M pack, `--kv` and `--max-context`
as the only variables: 2/2 at every arm that ran it — 131072/int8 and 196608/q4_0 at
prompt_n=127,669, 262144 with int8, k8v4 and q4_0 at 138,016, and the deployed
262144/q4_0 again at **183,987**, which is past the range this box had before.
What moved was the read speed: 1118 / 1038 / 744 / 892 / 995 tok/s in that order.
"""

import json
import os
import random
import sys
import time
import urllib.request

base = (sys.argv[1] if len(sys.argv) > 1 else "https://strata.int.leighhack.org").rstrip("/")
target = int(sys.argv[2]) if len(sys.argv) > 2 else 150_000
key = os.environ.get("STRATA_BENCH_KEY")
MAX_TOKENS = 400                    # reasoning + the two numbers, or `content` comes back empty

rnd = random.Random(20261118)       # fixed record set, so arms are comparable


def record(i):
    return "RECORD %05d: serial=%04x-%04x value=%d;\n" % (
        i, rnd.randrange(1 << 16), rnd.randrange(1 << 16), rnd.randrange(100_000))


n_rec = max(10, target // 30)
depths = (0.02, 0.97)
targets = {int(n_rec * d): record(int(n_rec * d)) for d in depths}
body = "".join(targets[i] if i in targets else record(i) for i in range(n_rec))
ask = ("\nGive the `value=` of RECORD %05d and of RECORD %05d, each as the number "
       "alone on its own line." % (min(targets), max(targets)))

payload = {"messages": [{"role": "user", "content": body + ask}],
           "max_tokens": MAX_TOKENS, "temperature": 0}
headers = {"content-type": "application/json"}
if key:
    headers["authorization"] = "Bearer " + key

t0 = time.time()
try:
    req = urllib.request.Request(base + "/v1/chat/completions", data=json.dumps(payload).encode(),
                                 headers=headers)
    with urllib.request.urlopen(req, timeout=3600) as r:
        d = json.loads(r.read())
    choice = (d.get("choices") or [{}])[0]
    msg = choice.get("message") or {}
    got = (msg.get("content") or "").strip()
    reasoning = (msg.get("reasoning_content") or "").strip()
    where = "content"
    if not got:
        got, where = reasoning[-400:], "reasoning"      # the budget ran out before it answered
    t = d.get("timings") or {}
    want = [v.strip().split("value=")[1].split(";")[0] for v in targets.values()]
    found = [w for w in want if w in got.split()]
    print("needle prompt_n=%s prompt/s=%s  records at %s -> %d/%d  (%s)  want=%s" % (
        t.get("prompt_n"),
        ("%.0f" % t["prompt_per_second"]) if t.get("prompt_per_second") else "-",
        "/".join("%d%%" % (100 * x) for x in depths), len(found), len(want), where, want))
    if len(found) != len(want):
        print("  answer %r  finish=%s reasoning_tokens=%d" % (got[:200], choice.get("finish_reason"),
                                                              len(reasoning)))
except Exception as e:                                                    # noqa: BLE001
    print("needle FAILED after %.0fs: %s" % (time.time() - t0, e))

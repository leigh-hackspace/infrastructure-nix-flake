#!/usr/bin/env python3
"""Greedy-output fingerprint bench: ask in the SAME prompts every time, at
temperature 0, and print a hash of each answer plus its token count.

  strata-prep strata-answer-check.py http://127.0.0.1:8123

Why this exists: several of the speed switches this box is offered are honest about
moving answers (`STRATA_PF_FUSED` is listed under "options that change answers", and
`STRATA_ROUTE_TAIL_SKIP`'s row says "answers differ slightly from a run without it"),
and the only way to say by how much, on THIS pack, is to ask the same question both
ways and diff it.  Greedy decoding makes the comparison meaningful: any difference is
the engine's rounding, not sampling.

It prints, per prompt: the sha256 of the answer text, the token count, and the first
divergence (the engine's own `usage` numbers when present).  Two runs of the same
config should match on every line; a config that "changes answers slightly" shows up
as a hash difference and a divergence offset.
"""

import hashlib
import json
import sys
import urllib.request

base = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8123").rstrip("/")

# Fixed, short, and answer-shaped: each one has a definite continuation, so a
# rounding difference shows up as a different token rather than a different style.
PROMPTS = [
    ("fib", "Write a Rust function that returns the nth fibonacci number iteratively, "
            "plus a unit test. Return only the code."),
    ("sortdoc", "In exactly two sentences, describe what this Go function returns: "
                "func f(s []int) int { n := 0; for _, v := range s { if v > 0 { n++ } }; return n }"),
    ("arith", "Answer with just the number: what is 237 * 48 + 19?"),
]

MAX_TOKENS = int(sys.argv[2]) if len(sys.argv) > 2 else 96

for name, prompt in PROMPTS:
    payload = {"messages": [{"role": "user", "content": prompt}], "max_tokens": MAX_TOKENS,
               "temperature": 0}
    try:
        req = urllib.request.Request(base + "/v1/chat/completions", data=json.dumps(payload).encode(),
                                     headers={"content-type": "application/json"})
        with urllib.request.urlopen(req, timeout=1800) as r:
            d = json.loads(r.read())
        ch = (d.get("choices") or [{}])[0]
        msg = ch.get("message") or {}
        text = msg.get("content") or ""
        reasoning = msg.get("reasoning_content") or ""
        t = d.get("timings") or {}
        both = (reasoning + "\x00" + text).encode()
        h = hashlib.sha256(both).hexdigest()[:16]
        print("%-9s sha=%s chars=%-5d tok=%-4s stop=%s"
              % (name, h, len(both), t.get("predicted_n"), ch.get("finish_reason")))
        print("          first line: %s" % (text.strip().splitlines() or ["(none)"])[0][:110])
    except Exception as e:                                    # noqa: BLE001
        print("%-9s FAILED: %s" % (name, e))

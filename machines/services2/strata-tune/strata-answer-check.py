#!/usr/bin/env python3
"""Is this engine's greedy output REPRODUCIBLE - and if not, what may a fingerprint prove?

  strata-prep strata-answer-check.py http://127.0.0.1:8123 [max_tokens] [reps]
  python3 strata-answer-check.py https://strata.int.leighhack.org 96 3

It was written to answer "does this speed switch change answers?" by hashing a fixed
greedy completion, deployed once and compared.  **That premise is false on this box, and
this script is now the thing that measures it rather than one that assumes it.**  Measured
2026-11-14 on services2, same live unit, same config, same prompts, two runs:

    run A   fib=e46e9bbb  sortdoc=4b5d4b  arith=f8423c
    run B   fib=f26e20    sortdoc=84dc6b  arith=f8423c

Two of three items differ between two runs of ONE configuration.  So "the fingerprints
moved when I turned the switch on" attributed noise to the switch, and the earlier number
this script produced ("2 of 3 continuations change under STRATA_PF_FUSED=1") is not a
finding about that flag: with greedy decoding a single tie-break diverges a whole
continuation, and this engine does not even start two runs at the same tie-break.

What the output therefore is: **per item, STABLE or VARIABLE across `reps` runs, and
`stable=N/M`**.  Only a STABLE item can carry a before/after claim, and on 2026-11-14 the
only item that ever was stable (`arith`, 4 runs across two configs and two days) was
identical across both configs.  For "is it still *right*", do not use this script: use
`strata-answer-score.py`, which scores against an answer key and does not care whether the
prose is byte-identical.
"""

import hashlib
import json
import sys
import urllib.request

base = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8123").rstrip("/")
MAX_TOKENS = int(sys.argv[2]) if len(sys.argv) > 2 else 96
REPS = int(sys.argv[3]) if len(sys.argv) > 3 else 3

# Fixed, short, and answer-shaped: each has a definite continuation, so a divergence is
# visible.  They are NOT assumed reproducible - proving that is the run's other output.
PROMPTS = [
    ("fib", "Write a Rust function that returns the nth fibonacci number iteratively, "
            "plus a unit test. Return only the code."),
    ("sortdoc", "In exactly two sentences, describe what this Go function returns: "
                "func f(s []int) int { n := 0; for _, v := range s { if v > 0 { n++ } }; return n }"),
    ("arith", "Answer with just the number: what is 237 * 48 + 19?"),
]


def ask(prompt):
    payload = {"messages": [{"role": "user", "content": prompt}], "max_tokens": MAX_TOKENS,
               "temperature": 0}
    req = urllib.request.Request(base + "/v1/chat/completions", data=json.dumps(payload).encode(),
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=1800) as r:
        d = json.loads(r.read())
    msg = ((d.get("choices") or [{}])[0].get("message")) or {}
    both = ((msg.get("reasoning_content") or "") + "\x00" + (msg.get("content") or "")).encode()
    return hashlib.sha256(both).hexdigest()[:6]


stable = 0
for name, prompt in PROMPTS:
    seen = []
    for _ in range(REPS):
        try:
            seen.append(ask(prompt))
        except Exception as e:                                # noqa: BLE001
            print("%-9s ERROR %s" % (name, e))
            break
    uniq = sorted(set(seen))
    ok = len(uniq) == 1
    stable += ok
    print("%-9s %-8s %s   (%s of %d runs)" % (
        name, "STABLE" if ok else "VARIABLE", " ".join(uniq), len(seen) if ok else len(uniq),
        REPS))

print("stable %d/%d  -  only a STABLE item can carry a before/after claim" % (stable, len(PROMPTS)))
if stable < len(PROMPTS):
    print("verdict: greedy output is NOT reproducible here; a one-run-per-config fingerprint "
          "comparison attributes run noise to whatever you changed")

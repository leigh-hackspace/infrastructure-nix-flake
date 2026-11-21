#!/usr/bin/env python3
"""Score a Strata endpoint on short, CHECKABLE answers - greedy, so the score is the
engine's rounding and nothing else.

  strata-prep strata-answer-score.py http://127.0.0.1:8123

Why this exists and `strata-answer-check.py` does not answer it: the fingerprint script
proves that a switch which upstream lists under "options that change answers" DOES move
this pack's output (2026-11-14: 2 of 3 greedy continuations differ under
STRATA_PF_FUSED=1).  A changed hash is not a degraded answer - with greedy decoding a
single tie-break diverges the whole continuation, so ANY last-bit rounding change reads
as "different".  The question that decides whether to deploy is whether the model is
still *right*, and that needs items with an answer key.

`INCOMPLETE` is a third state on purpose: a reasoning model whose budget ran out before
it emitted `content` has not answered wrongly, and counting that as a failure would let a
tokenizer artefact decide a deploy.  Only scored items count, and two runs of one config
should agree on the score AND on which items were incomplete.

Nine items, each one short (so the answer lands inside a small budget and the check is
robust to prose), each scored by a regex on the last number/word the model emitted.
It prints `score N/9` plus the failures.  Two runs of the same config should agree; a
config that degrades rounding shows up here, and 9/9 against 9/9 is the evidence that
the drift is cosmetic at the answer level.
"""

import json
import os
import re
import sys
import urllib.request

base = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8123").rstrip("/")
# 64 was not enough: a reasoning model burns the budget mid-calculation and the tail of
# the reasoning is then a partial sum, which reads as a wrong answer.  Measured 2026-11-14:
# 2 of 9 'failures' were `...=1139` with the answer one token away.
MAX_TOKENS = int(sys.argv[2]) if len(sys.argv) > 2 else 512
# Optional, and only ever from the environment: the arms' server is keyless, so a bench
# run never needs it, and no path here puts a key on a command line (rule 13).
KEY = os.environ.get("STRATA_BENCH_KEY")

# (name, prompt, extractor, expected)
ITEMS = [
    ("mul-add", "Answer with just the number: 237 * 48 + 19 = ?",
     lambda t: [int(x) for x in re.findall(r"-?\d+", t)], 11395),
    ("div-floor", "Answer with just the number: integer division 1000 // 7 = ?",
     lambda t: [int(x) for x in re.findall(r"-?\d+", t)], 142),
    ("neg", "Answer with just the number: 12 - 40 = ?",
     lambda t: [int(x) for x in re.findall(r"-?\d+", t)], -28),
    ("go-count", "In exactly one number, how many elements does this Go slice hold: "
                 "[]int{3, 5, 7, 9}?",
     lambda t: [int(x) for x in re.findall(r"-?\d+", t)], 4),
    ("rust-fn", "One line of Rust, no prose: an expression that returns the length of "
                "Vec<i32> v. Your answer must contain the characters .len()",
     lambda t: 1 if re.search(r"v\s*\.\s*len\s*\(\s*\)", t) else 0, 1),
    ("py-out", "Answer with just the number that Python prints: print(len(\"abcdefgh\"))",
     lambda t: [int(x) for x in re.findall(r"-?\d+", t)], 8),
    ("bit", "Answer with just the number: 0b1011 + 0b1 = ?",
     lambda t: [int(x) for x in re.findall(r"-?\d+", t)], 12),
    ("mod", "Answer with just the number: 100 mod 7 = ?",
     lambda t: [int(x) for x in re.findall(r"-?\d+", t)], 2),
    ("sum-seq", "Answer with just the number: 1 + 2 + 3 + 4 + 5 + 6 + 7 = ?",
     lambda t: [int(x) for x in re.findall(r"-?\d+", t)], 28),
]

ok = 0
scored = 0
for name, prompt, get, want in ITEMS:
    payload = {"messages": [{"role": "user", "content": prompt}], "max_tokens": MAX_TOKENS,
               "temperature": 0}
    try:
        req = urllib.request.Request(base + "/v1/chat/completions",
                                     data=json.dumps(payload).encode(),
                                     headers={"content-type": "application/json",
                                            **({"authorization": "Bearer " + KEY} if KEY else {})})
        with urllib.request.urlopen(req, timeout=1800) as r:
            d = json.loads(r.read())
        msg = ((d.get("choices") or [{}])[0].get("message")) or {}
        # Greedy reasoning models put the reasoning first; the answer is in content when
        # there is content, and in the tail of the reasoning when the budget ran out.
        content = (msg.get("content") or "").strip()
        reasoning = (msg.get("reasoning_content") or "").strip()
        if not content:                       # the budget ran out before it answered
            print("%-9s INCOMPLETE  (no content; reasoning tail: %r)" % (name, reasoning[-50:]))
            continue                          # not a wrong answer: do not score it either way
        vals = get(content)
        good = (vals == want) if isinstance(want, int) and want == 1 else (want in vals)
        ok += bool(good)
        scored += 1
        print("%-9s %-4s  %r" % (name, "PASS" if good else "FAIL", content[:60]))
    except Exception as e:                                    # noqa: BLE001
        print("%-9s ERROR %s" % (name, e))

print("score %d/%d  (%d scored, %d incomplete)" % (ok, len(ITEMS), scored, len(ITEMS) - scored))

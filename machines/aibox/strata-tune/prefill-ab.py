#!/usr/bin/env python3
"""The 2026-10-28 prefill / conversation-cache A/B, as a script, so the numbers in
../STRATA.md ("Prefill 2.2x, and what the conversation cache actually parks") can be
re-run instead of re-believed.

    ./prefill-ab.py https://strata2.int.leighhack.org/v1 pair
    ./prefill-ab.py https://strata2.int.leighhack.org/v1 alt
    ./prefill-ab.py http://127.0.0.1:8080/v1 probe          # direct, needs a key
    ./prefill-ab.py https://strata2.int.leighhack.org/v1 ext

The door at `strata2.int` injects the engine key, so through it no secret is needed;
directly against the engine, pass `Authorization` yourself or run it there.

What each mode is for, because the whole point of the 2026-10-28 run was that the
obvious probe measures the wrong thing:

  pair   one ~13k-token prompt, sent cold and then again with two words added. The
         cold send is the prefill measurement (compare `strata serve: prompt N tokens =
         X reused + Y read in Z ms` line to line, cold against cold - the same box's
         second cold read was 26% faster than its first, so within-pair deltas under
         ~30% are noise). The second send measures exact-prefix reuse: it reported
         `reused 0` in both arms.
  alt    A, B, A, B, A over two different prompts. This is what parking actually does:
         repeats of a conversation restore in ~40 ms (`restored N tokens (checkpoint)`)
         where a cold read costs ~160 s. Run it with the cache on and off to see the
         difference, and note `bytes=` / `evictions=` to see the cap working.
  ext    A, then A extended by 30 tokens. The answer is `reused 0` and a full re-read:
         parking keys on prompt IDENTITY, so an agent turn - prompt_t plus new text -
         gets nothing from it. Anyone "fixing long agent prompts" with this flag should
         run this mode first.
  probe  a short, meaningful, temperature-0 generation, printed with its md5. Kept as a
         smoke test only: greedy output on this engine is NOT reproducible run to run
         (two identical runs gave `edf775d3...` and `e83db119...`), so a changed digest
         proves nothing about a changed default and an unchanged one proves less.

Reading the engine's own accounting needs the log, so run it where
/var/lib/strata/strata.log is readable (root, on aibox) or pass `--no-log`.
"""

import hashlib
import json
import random
import subprocess
import sys
import time
import urllib.request

LOG = "/var/lib/strata/strata.log"
MODEL = "qwen3.8-flash-next-iq2xs"
TAIL = "\n\nReply with exactly the single word: READY"

WORDS = (
    "switch lease prefix tunnel arena expert cache token window pinned memory budget slot "
    "layer query key value head attention router dnsmasq nftables backup borg snapshot "
    "volume export mount daemon service unit timer journal certificate issuer domain "
    "resolver http proxy upstream socket request header postgres collation database schema "
    "index secret sops runtime store path build network vlan subnet bridge"
).split()


def prompt(seed: int) -> str:
    """~13k tokens of pseudo-random prose: repetitive enough to be boring, random enough
    that the tokenizer cannot collapse it into something smaller than it measures."""
    rnd = random.Random(seed)
    lines = []
    n = 0
    while n < 8600:
        lines.append(f"{n:05d} " + " ".join(rnd.choice(WORDS) for _ in range(12)))
        n += 13
    return "\n".join(lines) + "\n"


def engine_lines(since: int) -> str:
    if "--no-log" in sys.argv:
        return "(log not read)"
    out = subprocess.run(
        ["tail", "-c", f"+{since + 1}", LOG], capture_output=True, text=True
    ).stdout
    # `strata serve: prompt ...` starts at column 1 of the line. Filtering for it with a
    # leading space - which is how this script was first written - silently reports
    # nothing, and "nothing" next to a valid timing looks like a result.
    keep = [
        l
        for l in out.splitlines()
        if "strata serve: prompt" in l or "conversation cache" in l
    ]
    return "\n".join("      " + l[:190] for l in keep[-6:]) or "      (no line)"


class Runner:
    def __init__(self, base: str):
        self.base = base.rstrip("/") + "/chat/completions"
        self.mark = 0

    def logsize(self) -> int:
        if "--no-log" in sys.argv:
            return 0
        return int(
            subprocess.run(["stat", "-c", "%s", LOG], capture_output=True, text=True).stdout
        )

    def send(self, tag: str, content: str, max_tokens: int = 8) -> None:
        body = json.dumps(
            {
                "model": MODEL,
                "messages": [{"role": "user", "content": content}],
                "max_tokens": max_tokens,
                "temperature": 0,
                "stream": False,
            }
        ).encode()
        self.mark = self.logsize()
        req = urllib.request.Request(
            self.base, data=body, headers={"Content-Type": "application/json"}, method="POST"
        )
        t0 = time.time()
        with urllib.request.urlopen(req, timeout=3600) as resp:
            payload = json.load(resp)
        el = time.time() - t0
        time.sleep(2)  # the engine writes its accounting line as the request ends
        msg = payload["choices"][0]["message"]
        usage = payload.get("usage", {})
        print(f"--- {tag}: client {el:.1f} s | prompt={usage.get('prompt_tokens')} "
              f"completion={usage.get('completion_tokens')} "
              f"cached={usage.get('prompt_tokens_details', {}).get('cached_tokens')}")
        if max_tokens > 64:
            text = (msg.get("content") or "") + "|" + (msg.get("reasoning_content") or "")
            print(f"    md5(reply) = {hashlib.md5(text.encode()).hexdigest()}")
        print(self.engine_lines(self.mark))
        sys.stdout.flush()


def main() -> None:
    if len(sys.argv) < 3:
        print(__doc__)
        raise SystemExit(2)
    r = Runner(sys.argv[1])
    mode = sys.argv[2]
    a = prompt(20261028) + TAIL
    b = prompt(777) + TAIL
    if mode == "pair":
        r.send("A cold", a)
        r.send("A + 2 words (tests exact-prefix reuse)", a + " now.")
    elif mode == "alt":
        for name, body in (("A", a), ("B", b), ("A", a), ("B", b), ("A", a)):
            r.send(name, body)
    elif mode == "ext":
        r.send("A (parks it)", a)
        r.send("A + 30 tokens (what an agent turn is)", a + " alpha beta gamma delta epsilon " * 6)
        r.send("A unchanged again (control)", a)
    elif mode == "probe":
        r.send(
            "short meaningful generation",
            "A router has 3 uplinks and 5 VLANs. VLAN 225's IPv4 leaves through the "
            "tunnel, every other VLAN's IPv4 leaves through the hub with a second NAT, "
            "and all IPv6 leaves through the tunnel. A client on VLAN 230 reports that "
            "IPv4 works and IPv6 does not, and a client on VLAN 225 reports the opposite. "
            "In at most 150 words, list the three most likely causes in order, and say "
            "one check per cause.",
            max_tokens=200,
        )
    else:
        raise SystemExit(f"unknown mode {mode!r} (pair|alt|ext|probe)")


main()

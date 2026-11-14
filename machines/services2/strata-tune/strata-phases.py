#!/usr/bin/env python3
"""strata-phases.py - labelled prefill/decode phases plus a GPU sampler in one process,
for the power and SM-clock claims in ../STRATA-TUNE-2026-11-14.md.

  sudo python3 machines/services2/strata-tune/strata-phases.py     # -> /tmp/strata-phases.csv

One process owns both the marks and the samples, because a phase is only identifiable if
its boundary lands in the same file as its readings; driving them from a second shell put
the marks in a file the sampler then overwrote (an append-mode handle writes at its own
offset, and every MARK line vanished - measured).  It runs as root because the api key
lives in the config the module renders; the key goes straight into the request header and
is never printed, and nothing here dumps the config (AGENTS.md rule 13).

The phases are three long prompts (a real prefill each - a *repeated* prompt is answered
from the engine's prompt prefix cache, which is how a "prefill" window once contained no
prefill) and one short-prompt/long-generation pass, with idles around them so the idle
baseline is in the same file.

The key goes from the rendered config straight into the header and is never printed
(rule 13); nothing here dumps the config.
"""

import json
import subprocess
import threading
import time
import urllib.request

CSV = "/tmp/strata-phases.csv"
CFG = subprocess.run(["bash", "-lc",
                      "systemctl show strata -p FragmentPath --value | "
                      "xargs grep -o '/nix/store/[a-z0-9]*-strata-coder-iq1m.json' | head -1"],
                     capture_output=True, text=True).stdout.strip()
cfg = json.load(open(CFG))
key = cfg.get("api_key") or ""
url = "http://10.3.1.21:8080/v1/chat/completions"

stop = threading.Event()


def sampler():
    """memory.used, power.draw, utilization.gpu, clocks.sm every 2 s."""
    import os
    with open(CSV, "w") as f:
        f.write("epoch,utc,mib,watts,util,sm_mhz\n")
    # append mode, and re-opened per line is not needed - but "w" on this handle would
    # write at its own offset and step on the MARK lines another handle appends (measured:
    # every mark vanished), so the sampler appends too.
    with open(CSV, "a") as f:
        while not stop.is_set():
            out = subprocess.run(["nvidia-smi", "--query-gpu=memory.used,power.draw,"
                                  "utilization.gpu,clocks.sm", "--format=csv,noheader,nounits"],
                                 capture_output=True, text=True).stdout.strip().replace(" ", "")
            f.write("%d,%s,%s\n" % (time.time(), time.strftime("%H:%M:%S", time.gmtime()), out))
            f.flush()
            stop.wait(2)


def mark(label):
    with open(CSV, "a") as f:
        f.write("%d,MARK,%s,,,\n" % (time.time(), label))
    print("mark " + label, flush=True)


SEQ = [0]


def ask(repeat, max_tokens):
    SEQ[0] += 1
    # a DIFFERENT filler per call: with the same text twice the engine answers the second
    # one from its prompt prefix cache (measured: prompt=5 of 63,052 tokens read), which
    # would leave the "prefill" window with no prefill in it.
    filler = "Repeat the digit %d exactly. " % SEQ[0]
    body = {"messages": [{"role": "user", "content": filler * repeat}],
            "max_tokens": max_tokens, "temperature": 0}
    req = urllib.request.Request(url, data=json.dumps(body).encode(),
                                 headers={"content-type": "application/json",
                                          "authorization": "Bearer " + key})
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=900) as r:
            d = json.loads(r.read())
    except Exception as e:                                    # noqa: BLE001
        print("  request failed: %s" % e, flush=True)
        return
    t = d.get("timings") or {}
    print("  prompt=%s gen=%s  prefill=%s tok/s  decode=%s tok/s  wall=%.0fs"
          % (t.get("prompt_n"), t.get("predicted_n"), t.get("prompt_per_second"),
             t.get("predicted_per_second"), time.time() - t0), flush=True)


th = threading.Thread(target=sampler, daemon=True)
th.start()
time.sleep(4)
print("config %s  api_key %s" % (CFG, "present" if key else "none"), flush=True)

mark("idle-1")
time.sleep(8)
mark("prefill-burst")                       # three ~9k-token prompts back to back
for _ in range(3):
    ask(9000, 8)
mark("idle-2")
time.sleep(8)
mark("decode-heavy")                        # short prompt, long generation
ask(40, 240)
mark("idle-3")
time.sleep(8)
mark("end")
stop.set()
time.sleep(2)
print("wrote " + CSV, flush=True)

#!/usr/bin/env python3
"""Turns /tmp/arm-*.log into the table for STRATA-TUNE-2026-11-14.md.

The arms' logs are the receipts; this keeps the table in the doc honest, because it
reads the same lines the harness printed rather than anything I retyped.  A field an
arm did not measure prints as "-".

usage: nix shell nixpkgs#python3 -c "python3 machines/services2/strata-tune/strata-tune-report.py '/tmp/arm-*.log'"
"""

import glob
import re
import sys

PAT = {
    "args": re.compile(r"^=== arm: (.*?) ===$", re.M),
    "steady": re.compile(r">>> decode steady median ([\d.]+) tok/s"),
    "median": re.compile(r">>> decode median ([\d.]+) tok/s"),
    "8k": re.compile(r"^prefill-8k\s+prompt_n=\d+\s+prompt/s=([\d.]+)", re.M),
    "28k": re.compile(r"^prefill-28k\s+prompt_n=\d+\s+prompt/s=([\d.]+)", re.M),
    "short": re.compile(r"^short-decode\s+prompt_n=\d+\s+prompt/s=[\d.]+\s+decode_n=\d+\s+decode/s=([\d.]+)", re.M),
    "draft": re.compile(r"^short-decode.*draft=(\d+/\d+)", re.M),
    "share": re.compile(r"PCIe share: auto = ([\d.]+)"),
    "cache": re.compile(r"expert cache: (\d+) slots"),
    "aux": re.compile(r"aux cpus: .*CPUs ([\d,-]+) \((\w+)"),
}

print("| arm | decode tok/s | 8k | 28k | short | spec accept | share | slots | answer sha | override |")
print("|---|---|---|---|---|---|---|---|---|---|")
for f in sorted(glob.glob(sys.argv[1] if len(sys.argv) > 1 else "/tmp/arm-*.log")):
    name = f.split("arm-")[-1].replace(".log", "")
    txt = open(f, errors="replace").read()
    g = {k: (m.group(1) if (m := p.search(txt)) else "-") for k, p in PAT.items()}
    dec = g["steady"] if g["steady"] != "-" else g["median"]
    ans = " ".join(re.findall(r"sha=([0-9a-f]{6})", txt)) or "-"
    print("| {:<15} | {:>6} | {:>6} | {:>6} | {:>6} | {:>7} | {:>5} | {:>5} | {:<20} | {} |".format(
        name, dec, g["8k"], g["28k"], g["short"], g["draft"], g["share"], g["cache"],
        ans[:20], (g["args"] or "-")[:90]))

# Strata on aibox — measured behaviour and what to try next

Qwen3.8-Flash-Next (125 B total / ~6 B active MoE) running as `strata.service`
on the Radeon 660M iGPU + system RAM + NVMe. Config: `strata.nix` (service) +
`strata-package.nix` (engine build). Endpoints: `http://10.3.1.32:8080/v1` and,
through nginx, `http://aibox.int.leighhack.org/llm/v1`.

It replaced `llama-server` for a reason: the dense 27B (Dirk-Qwen3.8-27B-UD-Q6_K)
decodes at **~2 tok/s** here — a 200-token completion timed out at 145 s.

## Measured (2026-10-05, IQ2_XS, `--max-context 131072 --kv int8 --kv-resident 32768 --spec 4 --spec-min-p 0.5`, llama-server stopped)

| `--expert-cache` | experts cached | `mem_info_gtt_used` | RAM left | cache hit | decode tok/s | prefill tok/s |
|---|---|---|---|---|---|---|
| 2048 (was deployed) | 2 132 (2.88 GiB) | 8.66 GB | 13 GiB | 60–66 % | 8.2 / 8.7 / 8.0 / 10.2 | ~40 (`auto`→4096 chunk) |
| 2048 + `--prefill 8192` | 2 132 | 8.66 GB | 13 GiB | 58–66 % | 8.7 / 8.0 / 10.2 | 44.6 / 44.7 |
| **4096 (deployed)** | 4 292 (5.76 GiB) | 11.75 GB | 10 GiB | 73–85 % | 9.7 / 7.8 / 8.9 | 46.1 / 46.0 |
| 8192 | 8 607 (11.52 GiB) | 17.94 GB | **4.7 GiB** | 86–91 % | 9.7 / 9.2 / 11.2 | 46.0 / 46.1 |
| 0 (engine auto-fills) | 12 167 (16.31 GiB) | 23.08 GB | **0.6 GiB** | 91 % | 10.1 | — |

Each expert blob is **1 382,400 B**, so slots ≈ GiB / 1.317. Model load is
"experts loaded: 33.02 GiB at ~4.9 GiB/s" ≈ 12 s, plus the cache fill.

**Deployed now: `--expert-cache 4096` and `--prefill 8192`** — measured 9.7 tok/s
decode, 46 tok/s prefill, 11.75 GB of GTT, 10 GiB RAM left. 8192 slots measured
the fastest decode but left 4.7 GiB on a box that also runs frigate, immich,
whisper and the containers — that is not a margin worth having.

## Why more GTT barely helps (the answer to "we only use 8 GB of the GTT")

On an iGPU there is **no separate VRAM**: every "VRAM" allocation is GTT, i.e.
pages of the same DDR5 the CPU expert pool already reads. The expert cache is a
**GPU-compute tier, not a memory tier** — a cached expert is still DRAM traffic.
Quadrupling the cache (hit 63 % → 91 %) moved work from the CPU pool to the GPU
and bought only **+11–17 % decode, +3 % prefill**, while eating the RAM the
arena needs. `--expert-cache auto` is worse still: it reads `hipMemGetInfo`'s
~52 GiB of "free" GTT and fills the machine.

## Where a decode token actually goes (`STRATA_DECODE_TIMING=1 STRATA_VERIFY_PROFILE=1`)

```
56 windows, avg T 2.62, 2.14 tokens accepted/window, 260.23 ms/window  (= 121 ms/token, 8.2 tok/s)
  verify 234.11 = GPU-reach wait 121.94 + per-layer host 95.71 [CPU experts 95.24] + stage 0.02
  + commit/emit 0.07 + draft (MTP) 26.03
  per layer-window: CPU experts 5.50 ms (7.36 entries), VRAM hits 12.83, PCIe 5.51

GPU stages (ms/window): GDN  q8+qkv/q-idx gemv 21.29 · conv 0.43 · ab 0.79 · z 12.16 · rec 5.45
  out-proj 10.29 · hc-read1+router 17.79 · shared+quant 7.38 · VRAM hits 24.23 · waitB 10.40
  PCIe grp 14.90 · waitCPU 27.65 | LM head 15.19
  QSA  q+q-idx 9.80 · attention 1.26 · out-proj 3.51 · hc-read1+router 5.94 · shared+quant 2.51
  VRAM hits 9.69 · PCIe grp 3.70 · waitCPU 3.29                                   total 234.33
```

Ranked cost: **CPU expert pool ~95 ms/window** (the engine logs *"this CPU has no
AVX-512: the expert kernels run on AVX-2"*, and the 6600H has 6 cores), dense
GDN/QSA work on a 6-CU GPU ~100 ms, expert-cache handling + PCIe grouping +
`waitCPU` ~83 ms, LM head 15 ms, MTP draft 26 ms.

## The real limit for coding use is prefill, not decode

**~45 tok/s, independent of chunk size** (`auto`→4096 = 44.6, explicit 8192 =
46.0). A 20 000-token prompt costs **~7 minutes** before the first token, and a
coding agent re-sends its context every turn. Decode at 8–11 tok/s is usable;
prefill is what will hurt. Nothing tried so far moves it — it is the 6-CU GPU's
grouped-GEMM throughput, not a cache or chunk-size problem.

## Suggestions, in the order I would try them

1. **`--pool-workers` (untested, cheapest).** The pool defaults to 5 (physical
   cores minus the host thread) but the 6600H has 12 SMT threads, and CPU experts
   are the single biggest line in the profile. Arms for 12 / 10 / 3 are in
   `strata-tune/run-arms.sh`; ~25 s each with the short bench. Upstream's RDNA2
   report saw *fewer* workers beat more on a 16-core box, so measure both ends.
2. **The Coder family (`ISTA-DASLab/Qwen3.8-Flash-Next-GSQ-RCO-Coder-GGUF/IQ1_M`).**
   It is the coding-tuned variant (256 of 512 experts kept) and its arena is
   **23.4 GiB instead of 35.5** — that frees ~12 GiB, which is exactly what a
   12 000-slot expert cache needs. Cost: a 58.4 GB re-download (~11 min at the
   ~89 MB/s this box gets from HF), a new pack, and `expert-profile-coder.bin`
   (already shipped in the package's `data/`). This is the change most likely to
   matter for coding quality *and* speed together.
3. **`--spec` / `--spec-min-p`.** The MTP draft costs 26 ms/window and accepts
   2.14 of 2.62 offered. Try `--spec 6` and `--spec-min-p 0.35/0.65` with the
   short bench; this is a pure trade of draft cost against acceptance.
4. **Do not chase the PLE table.** `--ple-io direct` (default) reads the 28.8 GB
   n-gram shard unbuffered from NVMe; `--ple-io mmap/ram` would need RAM the
   arena already owns. The stage list shows no I/O stall (`waitA` 0.24 ms).
5. **Add an `api_key` to the run config.** The unit logs *"WARNING: no API key —
   anyone on your network can use this model"*. Not added here because it breaks
   existing clients until they send it.
6. **The GPU is the ceiling.** Upstream `docs/AMD_HIP.md` measures the same
   engine on an RX 6900 XT (gfx1030, 16 GB): 38–42 tok/s decode and 330–339
   tok/s prefill. aibox has a USB4/Thunderbolt host router, so an eGPU is
   physically plausible — but a supported card in the *internal* M.2→USB3 slot is
   what killed the GTX 1060 attempt (2.5 GT/s ×1 ≈ 250 MB/s, 256 MB BAR; see
   `gtx1060-followup.md`). Don't repeat that without a real USB4/TB4 dock.

## Operational notes

- **Ports:** 8080 strata, 8100/8081 llama-server, **8090 is frigate-monitor** (a
  bench client hitting it gets `HTTP 405: Method Not Allowed` and looks like a
  Strata failure). The harness uses 8123.
- **The engine binds `10.3.1.32` (its `--host`), not loopback.** nginx's `/llm/`
  must therefore `proxy_pass http://10.3.1.32:8080`; `127.0.0.1:8080` returns a
  silent 502.
- **`ai.int.leighhack.org` is now broken by this module.** On services1
  (`machines/services1/ai.nix:28`) it proxies to `10.3.1.32:8081`, i.e. aibox's
  llama-server, which `displaceLlamaServer` stops → the vhost answers 502, and
  both Pi hosts still have it as their default (`llamaServerUrl` and
  `defaultProvider` in `~/.pi/agent/settings.json`). Either repoint that vhost to
  `10.3.1.32:8080` (Strata) or point pi at the `strata` provider in
  `~/.pi/agent/models.json`.
- **A killed engine keeps its GTT.** If an experiment engine is SIGKILLed, the
  next `strata.service` start fails with
  `cudaMalloc(1538035200) for the weight arena failed (out of memory): 60 MiB of 53393 MiB VRAM free on this GPU`.
  Kill the leftover `bin/strata --serve` process, wait ~10 s, then start the unit.
- **`--expert-cache 0` is not "no cache"** — the engine auto-fills whatever fits
  (12 167 experts / 16.31 GiB measured). Use an explicit number.
- **llama-server and strata cannot be resident together** (llama-server holds
  ~33 GB of GTT). `systemctl stop strata && systemctl start llama-server` for the
  small models; the unit's `ConditionPathExists` on the pack index keeps the
  service from crash-looping before the hand-run prep exists.
- `machines/aibox/configuration.nix` sets `system.autoRollback.enable = true`:
  every `nixos-rebuild switch` here must be followed by `sudo nixos-confirm`.
- New files must be `git add`ed — the flake reads its own directory through git.

## Pi clients

`~/.pi/agent/models.json` on **aibox** and **services1** (10.3.1.20) was written
from `pi-models.json` in this directory (the canonical copy, kept here because
`~/.pi` is per-user state and not in the flake):

- `strata` → `http://10.3.1.32:8080/v1`, model `qwen3.8-flash-next-iq2xs`,
  context 131 072, maxTokens 16 384.
- `strata-zen3` → `https://llm.ai.chrisdell.info/v1`, model
  `qwen3.8-flash-next-iq3xxs`, context 262 144 — the fast fallback (51 tok/s
  decode, 1184 tok/s prefill), worth using for anything with a big context until
  aibox's prefill gets better.

To make either the default, set in `~/.pi/agent/settings.json`:
`"defaultProvider": "strata"`, `"defaultModel": "qwen3.8-flash-next-iq2xs"`
(and drop `llamaServerUrl` / the `pi-llama-cpp` package if llama-server stays
displaced — it talks to llama.cpp's own endpoints, not Strata's).

## Re-measuring

`strata-tune/` in this directory: `sudo ./strata-exp.sh <expert-cache> <prefill>
[extra-env-json] [extra-args-json] [bench-script]` (derives the engine binary and
run config from the installed unit, so it follows a rebuild) and
`sudo ./run-arms.sh` for the full arm set. Benches: `strata-bench.py` (decode +
two cold ~8k-token prefills) and `strata-bench-short.py` (one short decode, for
reading the stage profile).

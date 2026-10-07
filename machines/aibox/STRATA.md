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

## UMA reality check: GTT, "VRAM" and "PCIe" are all the same DDR5 (2026-10-07)

Measured on the deployed engine (0.1.40.2, `--expert-cache 4096` which the
engine rounds to 4292 slots, 262144 context):

```
mem_info_gtt_total   56.00 GiB    the driver's cap on DDR5 the GPU may map
mem_info_gtt_used    15.11 GiB
MemTotal             59.6 GiB
```

GTT is not a second memory pool; it is the GPU's mapping of the same DDR5 the
CPU expert pool reads. Two things follow that are easy to get backwards.

### "Put the whole model in the GTT" is a compute change, not a bandwidth one

- All **24 576 experts ≈ 33-36 GiB** as IQ2_XS, so a full GPU copy *does* fit
  under the 56 GiB cap (a full cache ~34 GiB + dense/KV/MTP ~8 GiB ≈ 42 GiB).
- **But the engine defaults to a resident expert arena *plus* the GPU cache**:
  the same experts as CPU-readable memory (~33 GiB, `cudaHostRegister`ed, read
  through the file cache) *and* the hot ones copied into GTT. Both plus a full
  GPU copy is ~33 + 34 + 8 ≈ **75 GiB** — impossible. The only way it fits is
  `--mmap-experts` (drops the resident arena; single file-backed copy), a cache
  sized for all 24 576 experts and `--no-pool`.
- **The payoff is small, because the routing is very skewed.** 8192 slots (33 %
  of experts) already covers ~90 % of routing; `--expert-cache auto` (12 167
  experts, 16.3 GiB) covers 91 %. The last ~9 % is ~12 000 experts (~17 GiB)
  for a projected single-digit percent of decode — and it moves cold-expert
  work onto the same 6 CUs that are already the dense-compute bottleneck,
  instead of the idle CPU cores.
- **The ceilings do not move either way.** Decode is DRAM-bandwidth-bound
  (~2.7 GB/token at ~50 GB/s → ~18 tok/s theoretical; we sit at 9-12) and
  prefill is 6-CU compute-bound (~45 tok/s). Moving experts between CPU and GPU
  changes *who computes*, not the DRAM traffic.

`--gpu-only-full` (replay all 48 layers + the LM head, no pool) measures the
true per-token GPU floor; a full-cache `--no-pool` arm gives the
all-experts-on-GPU number. Neither is a serve config — upstream documents
`--no-pool` as a measurement mode.

### The "PCIe" numbers are RAM→RAM copies

The engine's own startup log:

```
strata generate: PCIe probe: 31.4 GB/s host->device -> pcie_frac 0.55 (default 0.55)
```

31.4 GB/s is a memcpy inside the same DDR5, not a link: there is no PCIe path
between the 660M and the CPU. What the per-request line reports
(`… more read by the GPU over PCIe … (16.1 % of all routed)`) is `--pcie-frac`,
the share of experts **missing from the GPU cache that are handed to the GPU
instead of computed by the CPU pool**. On this box that is 12-22 % of routed
experts (typically ~16 %), and the GPU reaches them through a **mapped alias**
into the pinned host arena — `--pcie-frac 0` is documented as "the GPUs get no
mapped alias".

- The profile shows it as `PCIe grp` (~14.9 ms of the 234 ms verify window on
  the GDN side, ~3.7 ms QSA), plus host-side `PCIe 5.51`/layer-window.
- It is not a bus traversal, but it is real DRAM traffic on a bandwidth-bound
  box, and the 31.4 GB/s probe is below this box's ~50 GB/s peak, so the mapped
  host path is slower per byte than a native GTT allocation.
- It is intentional: CPU expert compute is the biggest single cost here
  (~95 ms/window), so the engine offloads a share of the misses to the GPU.

**Open tuning lever.** `pcie_frac 0.55` is the fallback default (measured
upstream on a Ryzen 7600 + RTX 5070); the probe found no real link here. On UMA
the trade is "6 CPU cores vs. GPU + extra DRAM traffic", so the optimum is not
obviously 0.55. Upstream's `--calibrate` sweeps exactly this. Worth A/B-ing
`--pcie-frac 0 / 0.25 / 0.55 / 1.0` with the `strata-tune` short bench: if a
lower value wins or ties, it also cuts the RAM→RAM traffic and the copy path (a
small reliability bonus next to the #884 copy bug). Also worth checking whether
the mapped alias is a true zero-copy read or a staged copy into a GTT buffer —
if it stages, UMA pays 2x DRAM traffic for bytes already in RAM.

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

## Crash: verify-window timeout (#267) — 2026-10-06

At **2026-10-06 13:04:57** a decode request died with:

```
[strata] the engine reported an error: verify: timed out at layer 23; its GPU waits were released
         but the GPU did not finish within 5 s (#267)
[strata] done: 1171 tokens in 173 s (7.4 tok/s) (error, cancel=False)
```

The stream had been healthy (8.8-8.9 tok/s at ~1100 tokens, context ~90k) and
then decayed 8.8 -> 8.2 -> 7.7 tok/s and stalled; the watchdog fired ~16 s
later. **It was not a process crash:** `NRestarts=0`, the engine PID was
unchanged from the previous boot, and the next request (a 92,594-token prompt)
was already being served. Since 0.1.31 the engine treats a verify-window
stall as a *bounded* wait: after 5 s it releases every GPU wait, aborts only
that request and keeps serving. That is the #267 containment working, not a
fault — the alternative (pre-0.1.31) is the host waiting forever, and on
Windows a GPU wedged into an unrecoverable "device lost" until a power cycle.

This is a known open **gfx1030/RDNA2 HIP** problem, and our box is an exact
match for upstream's "stock" repro (engine 0.1.39 @ `6f32ec0`, built with
`-DSTRATA_PREFILL_MMQ=ON`, running for gfx1030):

- **#884** (open) — stock `6f32ec0`, 2x RX 6900 XT: `verify: timed out at
  layer N … (#267)` after a long prompt, after ~7 earlier requests. Needs
  **MMQ prompt path + adaptive expert swaps + SDMA copies** together; removing
  any one avoids it: `HSA_ENABLE_SDMA=0` (0/4), `--adapt-every 100000` (0/7),
  `STRATA_PREFILL_MMQ=0` (0/4). Root cause traced to a ROCm barrier packet
  whose dependency signal has already completed but the command processor never
  passes it. Costs: `HSA_ENABLE_SDMA=0` slows prompt reads 10-24% (decode
  unchanged), `--adapt-every` costs 3-11% decode, `MMQ=0` costs 28-42% prompt.
- **#1103** (open, filed 2026-10-06) — gfx1030 intermittent timeouts, stable
  for the reporter with `HSA_ENABLE_SDMA=0`.
- **#649** (open) — same message, different trigger: a pinned resident budget
  on a low-RAM box starves the CPU expert pool so it misses the verify
  handshake. Keep an eye on memory pressure here (51/59 GiB used, 3.3 GiB
  swap, 12 GB GTT at the time).
- Upstream 0.1.40 ships **`STRATA_HIP_ADAPT_KERNEL_COPY=1`**, which does the
  adaptive swaps' H2D copies with a kernel instead of SDMA — the intended
  gfx1030 workaround, without the prompt penalty of `HSA_ENABLE_SDMA=0`.

Only one #267-class event has been seen in a 1.5-day run; it is intermittent.
If it recurs, set `STRATA_VERIFY_TRACE=1` (0.1.39+) to capture the upstream
trace block, then try `STRATA_HIP_ADAPT_KERNEL_COPY=1` (0.1.40+) before the
blunter `HSA_ENABLE_SDMA=0`. (The Oct 05 failures were the separate, documented
"a killed engine keeps its GTT" crash-loop, not this.)

**Deployed 2026-10-06:** engine bumped to **0.1.40.1**
(`strata-package.nix`, rev `82f46a8`, same pinned llama.cpp `3cf03257`). The
new engine came up clean in ~15 s (`/health`, `/v1/models`, 11.2 GiB of GTT)
and the request in flight during the restart was the only casualty. The
`STRATA_HIP_ADAPT_KERNEL_COPY=1` workaround was not enabled at that point; it
went on with the 0.1.40.2 update below.

## Update: engine 0.1.40.2 — 2026-10-07

Reviewed the crash record before bumping. All of it is in the two sections
above; summarised:

- **2026-10-05 01:33** — a crash-loop of `cudaMalloc(...) for the weight arena
  failed`: a SIGKILLed experiment engine had not released its GTT. Not an
  engine fault ("a killed engine keeps its GTT" above); the manual recovery
  fixed it.
- **2026-10-06 13:04:57** — one `verify: timed out at layer 23 … (#267)`,
  contained by the engine: the request was aborted and the service kept
  serving. The only #267-class event in a 1.5-day run.
- **2026-10-06 13:18** — `exit code -15` + `done: 0 tokens in 809 s`: the
  in-flight request when the 0.1.40.1 unit was restarted, not a fault.
- Since that restart: `NRestarts=0`, up 1 day 2 h, no further timeouts.

0.1.40.2 is a fixes-and-speed release (byte-identical default answers to
0.1.40). What it changes for our two open gfx1030 problems:

- **The gfx103x build now defaults to PR #540's attention kernel** (8 cells per
  step, DPP lane exchanges, bit-exact) instead of the LDS-pipe-bound default —
  a change to the kernels around the #267 stall trigger. `STRATA_ATTN_PRE75=0`
  restores the old one.
- **New diagnostics for exactly this class of timeout:** the engine prints the
  free VRAM before each verify-window capture (#1275), and
  `STRATA_DBG_GDN=1` checks the commit kernel's window count (#937).
- **Restart robustness:** the server's read of the engine's `READY` line is now
  bounded (#1317), and a request the engine refuses with an `ERR` answers at
  once instead of waiting 300 s for a `DONE` (#1059) — so a future restart
  should not strand requests the way the 13:18 one was.
- Upstream `docs/AMD_HIP.md` now documents the gfx1030 verify-timeout
  workarounds: `HSA_USERPTR_FOR_PAGED_MEM=0` (two R9700, ROCm 7.2) and
  `GPU_PINNED_MIN_XFER_SIZE=1048576` (RX 6800, `--mmap-experts`, which we do
  **not** use). `STRATA_HIP_ADAPT_KERNEL_COPY=1` (#884) remains the intended
  fix for the MMQ-prompt + adaptive-swap case we are an exact match for.
- `setup.py`'s per-GPU ROCm-wheel pin for #1103 (gfx103X) does **not** apply:
  we build against nixpkgs' `rocmPackages` (ROCm 7.2.3), not a setup.py wheel.
- **No confirmed upstream fix for #267 / #884**, so the env workarounds are
  still the only mitigations. Both are now **enabled** (see below).

### Reliability workarounds now enabled (2026-10-07)

The unit's run-config `env` sets two independent mitigations for the two
identified mechanisms of the gfx1030 verify timeout. They are env-only and
removable; if the timeout still recurs, the next steps are `--adapt-every`
(a larger number / disabling swaps, 3-11% decode) and then `HSA_ENABLE_SDMA=0`
(10-24% prompt).

- `STRATA_HIP_ADAPT_KERNEL_COPY=1` — does the adaptive expert tier's H2D
  swaps with a kernel instead of SDMA. This is upstream's intended fix for
  **#884** (MMQ prompt + adaptive swaps + SDMA copies, our exact config), and
  unlike `HSA_ENABLE_SDMA=0` it does not cost prompt speed.
- `HSA_USERPTR_FOR_PAGED_MEM=0` — keeps ROCr off USERPTR for paged host
  allocations. This is the documented **#750/#920** mechanism: on this iGPU
  every GPU allocation is a KFD userptr over the same DDR5 as the expert
  arena, and when the kernel reclaims a pinned page the GPU queues are
  suspended, which presents as the same verify timeout. Upstream measured the
  same median with it (solo requests a little slower), so it is a reliability
  for speed trade, not a free win.

The unit also has `startLimitIntervalSec = 0` so a startup that fails while a
leftover engine holds the GTT keeps retrying instead of parking in `failed`.

**Watch on recurrence:** the 0.1.40.2 engine now logs the free VRAM before
each verify-window capture (#1275); if a timeout appears again, capture that
line, plus `STRATA_VERIFY_TRACE=1` if needed. Memory is the other lever
(#649): at 262144 context the box sits at ~51 GiB used / ~8 GiB available with
2.6 GiB of swap in use, and the expert cache is the dial to turn down
(`expertCache = 2048` frees ~2.9 GiB for ~10% decode).

Pinned llama.cpp is unchanged (`3cf03257`), so the ggml side is identical.
Built for gfx1030 and device-checked clean (`strata-device --list-devices`
with `HSA_OVERRIDE_GFX_VERSION=10.3.0` reports `arch gfx1030`).

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

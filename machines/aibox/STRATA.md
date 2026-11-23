# Strata on aibox — measured behaviour and what to try next

Qwen3.8-Flash-Next (125 B total / ~6 B active MoE) running as `strata.service`
on the Radeon 660M iGPU + system RAM + NVMe. Config: `strata.nix` (service) +
`strata-package.nix` (engine build) + `rocm10-runtime.nix` (the optional ROCm 10
userspace runtime). Endpoints: `http://10.3.1.32:8080/v1` directly and
`https://strata2.int.leighhack.org/v1` through the front door on `gw` (one row in
`common/front-door.nix`; this box has no nginx of its own).

> **Parked 2026-10-17, un-parked 2026-10-28, and the trade it was parked for is gone: no
> box delegates its builds anywhere since 2026-11-23.** It was disabled so this box could be
> the network's build machine: the IQ2_XS expert arena is
> ~33 GiB of RAM a build needs, and `llama-server` got its `wantedBy` back. It runs
> again — on the flake's `strata` input instead of its own nine-releases-stale pin,
> and behind a new name. **The RAM trade was real until it was fatal**: with the
> engine loaded this box had ~10 GiB left for the builds of five machines, and that is
> what killed the engine three times in 2026-11 (the 2026-11-23 section near the end of
> this file has both receipts, the `/proc/vmstat` totals and the fix). So `systemctl stop strata`
> before a nixpkgs bump or anything that links a browser is still the advice for *this*
> box's own builds, but the box's `nix-daemon` is now capped and the engine's pages are
> protected. `strata.nix`'s `expertCache` stays the dial when a build and the
> engine must genuinely share a window. What made this an
> un-parking rather than a rebuild from scratch is that the model data under
> `/home/leigh-admin/Models` and `/home/leigh-admin/Strata` lives outside the store
> and the parking never touched it. The numbers below are what it measured, on this
> box, with the config `strata.nix` still describes.

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

**Measured 2026-10-08** (see the "`--pcie-mode direct`" section further down): it
stages, in the default `auto` mode, with a copy kernel inside the graph — and the
copy costs 6.8 ms of the 230 ms window, not the 18 ms the `PCIe grp` line looks
like. `--pcie-mode direct` (the grouped kernel reads the arena through its device
alias) is deployed. Sweeping `--pcie-frac` says 0.55 is the right share after all.

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

**SUPERSEDED IN SIZE, NOT IN DIRECTION (2026-10-28):** the ~45 tok/s below doubled to
**82-84 tok/s** with `STRATA_HIP_PROMPT_F16=1`, so a 20 000-token prompt is ~4 minutes
rather than ~7 and a 100K prompt ~20 minutes rather than ~45. Everything else in this
section still holds — prefill is still the wall, it still does not respond to chunk size
or GTT, and it is still what limits agent use here. See "Prefill 2.2×" for the
measurement and the bit-identical caveat.

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
   **Measured 2026-10-08: 12 workers is worse — 7.7 vs 9.3 tok/s** (`waitCPU`
   14.6 → 46.4 ms/window). The SMT siblings add no expert throughput; leave the
   default.
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
   **Measured 2026-10-08: `--spec 4 --spec-min-p 0.5` is at the optimum** — 6
   costs 7.3 % of the window for 3.7 % more accepted tokens, 0.35 costs 16 % for
   12 %, 0.65 loses 18 % of the tokens to save 11 % of the window. See "The
   plateau" below.
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
   what killed the GTX 1060 attempt (2.5 GT/s ×1 ≈ 250 MB/s, 256 MB BAR — measured
   2026-08, in git history). Don't repeat that without a real USB4/TB4 dock.

## Operational notes

- **Ports:** 8080 strata, 8100/8081 llama-server, **8090 is frigate-monitor** (a
  bench client hitting it gets `HTTP 405: Method Not Allowed` and looks like a
  Strata failure). The harness uses 8123.
- **The engine binds `10.3.1.32` (its `--host`), not loopback.** `proxy_pass` to
  `127.0.0.1:8080` is a silent 502 — the one thing the `strata2.int` row in
  `common/front-door.nix` exists to get right. And `10.3.1.32:8080` is reachable on
  the LAN because this box's firewall is off (`machines/aibox/networking.nix`), which
  is why the engine has an `api_key` rather than trusting a network.
- **`ai.int.leighhack.org` is now broken by this module.** On services1
  (`machines/services1/ai.nix:28`) it proxies to `10.3.1.32:8081`, i.e. aibox's
  llama-server, which `displaceLlamaServer` stops → the vhost answers 502, and
  both Pi hosts still have it as their default (`llamaServerUrl` and
  `defaultProvider` in `~/.pi/agent/settings.json`). Either repoint that vhost at
  Strata on :8080 (the `strata2.int` row is the same shape with one address
  changed) or point pi at the `strata` provider in `~/.pi/agent/models.json`.
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

The unit is wrapped in `INFRA.mkNeverGiveUp` (`common/systemd.nix`) —
`Restart = always` plus `startLimitIntervalSec = 0` — so a startup that fails while a
leftover engine holds the GTT keeps retrying instead of parking in `failed`.

**Watch on recurrence:** the 0.1.40.2 engine now logs the free VRAM before
each verify-window capture (#1275); if a timeout appears again, capture that
line, plus `STRATA_VERIFY_TRACE=1` if needed. **It recurred on 2026-11-23** — as
the `#29` watchdog rather than a contained `verify: timed out`, and that section is
below, at "2026-11-23: the watchdog ended the engine". Memory is the other lever
(#649): at 262144 context the box sits at ~51 GiB used / ~8 GiB available with
2.6 GiB of swap in use, and the expert cache is the dial to turn down
(`expertCache = 2048` frees ~2.9 GiB for ~10% decode).

Pinned llama.cpp is unchanged (`3cf03257`), so the ggml side is identical.
Built for gfx1030 and device-checked clean (`strata-device --list-devices`
with `HSA_OVERRIDE_GFX_VERSION=10.3.0` reports `arch gfx1030`).

## UMA: `--pcie-mode direct` deployed, and what the copy actually costs (2026-10-08)

The deep dive of 2026-10-08 (in git history: `git log --diff-filter=D --
machines/aibox/strata-uma-2026-10-08.md`) read the engine's three delivery modes for the
"PCIe" share and predicts ~10 % decode from removing the copy. The arms measured
it the same day: the mechanism is real, the size is ~1 %, and one reading of the
profile in that doc is wrong. Arms: `strata-tune/run-arms-uma.sh` (decode, the
short bench) and `strata-tune/run-arms-prefill.sh` (prefill, the full bench);
IQ2_XS, 4096 slots, every arm with `STRATA_DECODE_TIMING=1 STRATA_VERIFY_PROFILE=1`.

| arm | ms/window | decode tok/s | `waitB` GDN | `PCIe grp` GDN / QSA | `waitCPU` GDN | GTT | cache hit |
|---|---|---|---|---|---|---|---|
| control (`auto` → mode 2, copy kernel in the graph) | 229.7 | 9.3 | **6.95** | 10.86 / 2.76 | 11.6 | 16.23 GB | 75.5 % (+16.1 %) |
| **`--pcie-mode direct`** (deployed) | **227.0** | **9.4** | **0.14** | 12.49 / 3.17 | 14.6 | 16.23 GB | 75.5 % (+16.1 %) |
| `--pcie-mode dma` (copy engine / SDMA) | 230.4 | 9.3 | 0.16 | 10.86 / 2.77 | 12.9 | 16.23 GB | 75.5 % (+16.1 %) |
| direct + `--pcie-frac 0.25` | 265.1 | 8.1 | 0.14 | 5.98 / 1.53 | 48.7 | 16.23 GB | 67.6 % (+6.4 %) |
| direct + `--pcie-frac 1.0` | 231.1 | 9.3 | 0.15 | 22.68 / 6.07 | 2.5 | 16.23 GB | 96.6 % (+33.6 %) |
| direct + `--pcie-frac 0` | 284.8 | 7.5 | 0.13 | 0.66 / 0.22 | 73.0 | 16.23 GB | — (no GPU share) |
| direct + `--pool-workers 12` | 278.5 | 7.7 | 0.14 | 12.46 / 3.16 | 46.4 | 16.23 GB | 75.5 % |
| direct + `--no-prefill-borrow` | 226.7 | 9.5 | 0.14 | 12.47 / 3.17 | 14.2 | **21.03 GB** | 75.5 % |
| direct + `STRATA_PREFILL_CPU_SHARE=auto` | 233.0 | 9.4 | 0.14 | 12.80 / 3.29 | 14.6 | 16.23 GB | 75.9 % |

**1. `PCIe grp` is not the copy — `waitB` is.** In `VerifyWindow::pre`
(`src/core/verify.cpp:1414-1424`) the stamps run: `grouped(p_ptr, …)` → stamp 20,
then `wait_flag_ge(m_flagB_)` **and, only when `pcie_mode == 2`, `fetch_blobs` +
`rebase_ptrs`** → stamp 21, then `grouped(p_ptr2, …)` → stamp 22. So the staging
copy lives inside the `waitB` stage (6.95 → 0.14 ms/window = **6.8 ms, 3 % of the
window**) and `PCIe grp` is the grouped kernel *computing* the PCIe share, which
happens in every mode. The UMA doc's "14.9 + 3.7 ms/window of copy work" is that
kernel, not the copy; the ~10 % it projected does not exist.

**2. The alias is live on this box with `HSA_USERPTR_FOR_PAGED_MEM=0` set.** The
request log keeps printing `+13…21 % of the routed experts over PCIe`, and that
counter only moves when `pcie_layer()` is true, i.e. when
`ArenaExpertSource::device_alias()` returned an address (`expert_source.cpp:3011,
3062`). So the #750/#920 mitigation and the zero-copy read coexist. The direct
check is `strata-tune/alias-check.sh`, which runs upstream's
`tests/hip/mapped_alias.cpp` both ways — the package now installs it as
`strata-alias-check` (upstream builds the HIP test targets whenever HIP is on,
so it costs one install line). What it reports on this box:

```
  host  0x78949dea4000
  alias 0x78949dea4000
  -> the host pointer itself (unified addressing)
  the alias is a distinct device address       DOES NOT HOLD
  a kernel reads the alias correctly           holds
```

identically with `HSA_USERPTR_FOR_PAGED_MEM=0` and `=1`. There is no separate
device address space for the arena here: `device_alias()` hands back the host
pointer and a kernel reads it correctly — the strongest zero-copy the engine can
do. It also explains item 3: the difference between an arena read and a cache
read is not an address space, it is the page attributes ROCr gives a
`cudaHostRegister`ed region versus one of its own allocations.

**3. Reading through the alias is ~15 % slower per byte than reading a GTT copy.**
At identical routing (16.1 % of the routed experts) `PCIe grp` is 10.86 ms in the
staged modes and 12.49 ms in `direct`. Net still favours `direct` (the copy costs
more than the slower read), but this is the number the "alias cache" idea — drop
the 5.7 GiB cache and have the GPU read the arena everywhere — has to be weighed
against: it would pay that penalty on *every* expert the GPU reads, and the cache
is what keeps `waitCPU` at 14 ms instead of 73 (`--pcie-frac 0` measures the
no-GPU-share end: 7.5 tok/s). The cache is not only about *where* the bytes are:
it is what makes 75 % of the routing a GPU hit at all, so an alias cache has to
keep that property while reading slower.

**4. `--pcie-frac 0.55` is right for the wrong reason.** The probe's 31.4 GB/s is
a RAM→RAM memcpy, so the value is not a link measurement — but sweeping it lands
on the same answer: 0 → 7.5, 0.25 → 8.1, 0.55 → 9.4, 1.0 → 9.3 tok/s. PR #1548's
cost fit would not change aibox's split; it would only stop the probe printing a
silly number.

**5. Prefill does not move.** Full bench, a 6 943-token prompt twice:
control 50.4 / 50.5, `direct` 50.7 / 50.6, `direct --no-prefill-borrow` 50.9 /
50.8 (and **+4.8 GiB of GTT**, 16.23 → 21.03 GB — the loan reserve, not a saving),
`direct + STRATA_PREFILL_CPU_SHARE=auto` 50.4 / 50.3. Prefill is the 6-CU
grouped-GEMM ceiling; none of the UMA levers touch it. The CPU share *does* help
short prompts (70-token prompt: 16.0 → 20.7 prompt/s, +29 %) at the cost of bit
changes (upstream: first-token KL mean 0.006) — not enabled, revisit if agent
turns start being short-context.

**Deployed:** `--pcie-mode direct` in `strata.nix` (live after the switch:
9.6-9.7 tok/s, `+17.1 % of the routed experts over PCIe`). The reason is not the
1 %: it removes the in-graph copy kernel and its 16 staging blobs from the decode
loop, which is the same copy machinery the #884 gfx1030 timeouts are traced to.
`--pcie-mode dma` measured the same speed but does the copies with SDMA — that is
the #884/#1103 mechanism, so do not swap them.

## The plateau: every knob is already at its optimum (2026-10-08)

`strata-tune/run-arms-knobs.sh` — the flags the deployed engine already ships that
could plausibly move decode: the cache admission policy, the MTP draft trade, the
CPU-pool task batching, and the three overlap arms upstream documents. All with
`--pcie-mode direct` deployed, 4096 slots, the short bench, profile on.

| arm | ms/window | decode tok/s | what moved |
|---|---|---|---|
| control (deployed) | **226.95** | 9.4 / 9.6 | — |
| `--expert-cache-per-layer` | 233.19 | 9.4 / 9.5 | hit 75.9 % (vs 75.5 %) — the profile fill already spreads slots across layers |
| `--spec 6` | 243.57 | 9.1 / 9.2 | 2.22 tok/window (+3.7 %) for a 7.3 % longer window |
| `--spec-min-p 0.35` | 263.42 | 9.1 / 9.2 | 2.40 tok/window (+12 %) for a 16 % longer window |
| `--spec-min-p 0.65` | 201.22 | 8.8 / 8.9 | 1.76 tok/window (−18 %) for an 11 % shorter one |
| `--pool-tasks 6` | 236.19 | 9.1 / 9.2 | `waitCPU` 14.0 → 22.0 |
| `--pool-tasks 12` | 229.69 | 9.3 / 9.5 | `waitCPU` 14.0 → 16.8 |
| `--no-host-worker` | 241.08 | 8.9 / 9.0 | the host thread joining the drain is worth 14 ms |
| `--no-hit-poke` | 226.93 | 9.4 / 9.6 | **identical** — there is no overlap here to lose |
| `--shared-late` | 227.26 | 9.4 / 9.6 | identical |
| `--expert-cache 5500` (262144 ctx) | 225.06 | 9.4 / 9.5 | hit 82.0 %, GPU 128.5 / host 54.3, GTT 18.35 GB |
| `--expert-cache 6500 --max-context 131072` | 227.03 | 9.6 / 9.7 | hit 84.5 %, GPU 135.0 / host 48.8, GTT 17.82 GB |

**Nothing moves it, and now we know why.** `GPU-reach wait + per-layer host` — the
part of the window that is actual work — is invariant to *where* experts are
computed:

| placement | GPU | host (CPU pool) | sum |
|---|---|---|---|
| `--pcie-frac 0` (all CPU) | 107 | 135 | 242 |
| deployed (frac 0.55, 4096 slots) | 118 | 68 | 185 |
| 5500 slots | 129 | 54 | 183 |
| 6500 slots + 131k ctx | 135 | 49 | 184 |
| `--pcie-frac 1.0` (all GPU) | 177 | 11 | 188 |

The verify window is a **chain**: layer *L+1*'s dense work cannot start until
layer *L*'s CPU-computed expert rows land, so moving an expert from the pool to
the GPU trades ~13 ms of host time for ~11-17 ms of GPU time at 1:1. Decode is
pinned at **225-231 ms/window = 9.4-9.6 tok/s** by every configuration tried —
cache size, share, admission policy, draft depth, pool batching, overlap pokes.

**Both halves are compute-bound, not bandwidth-bound**, which is what makes the
plateau hard:

- The CPU pool: upstream's reference is "663.6 MB of expert bytes per token at
  ~40 GB/s" (`include/strata/core/expert_cache.hpp`). aibox spends 31.5 ms of
  CPU expert time per token on the same bytes → **~21 GB/s**, about half the
  reference, and the engine says why at startup: *"this CPU has no AVX-512: the
  expert kernels run on AVX-2"*. That is a Zen 3 limitation, not a setting.
- The GPU: 6 CUs, and the dense stages (`q8+qkv gemv` 21 ms, `hc-read1+router`
  14.6 ms, `z` 12 ms, head 15 ms) are unchanged by everything tried.

So the remaining paths are hardware and engine kernels, not config: an eGPU
(upstream's RX 6900 XT, same engine: 38-42 tok/s decode, 330-339 prefill), or
better AVX2 IQ2 dequant in the CPU expert kernels. **Nothing from this section
was deployed** — the control is the deployed config.

## Can we overclock it? The three ceilings, measured (2026-11-14)

The plateau above ends with "the remaining paths are hardware and engine kernels", and
**both halves of the chain being compute-bound is exactly the case where clocks are worth
asking about** - so this is that question, measured rather than reasoned about. Short
version: **the GPU cannot be overclocked (it already runs at its ceiling), the memory cannot
be overclocked (and would not help), and the one ceiling with real slack under it is the
package's power window - which on this kernel is not reachable by a one-liner.**

Method: `strata-tune/power-probe.sh` at 1 Hz (read-only; its header explains which columns
lie if read wrong) while `strata-tune/strata-bench.py` runs against the live endpoint
through the door, as a transient unit (`systemd-run --collect`) rather than a backgrounded
`ssh` job.  Control band reproduced first, so the numbers below are comparable to this
file's: **decode 9.5 short / 12.0-12.6, prefill 88.1-90.3 tok/s** - the same band as
"The control measurement" above.

| phase | package W (mean/max) | Tctl | iGPU die | gpu busy | CPU MHz (mean/max of 3 cores) | active sclk |
|---|---|---|---|---|---|---|
| idle | 8.4 / 9.7 | 39 °C | 37 °C | 0 % | 1,774 / 2,942 | 1900 |
| short-decode (26 s) | 36.2 / 58.2 | 65 °C | 71 °C | 100 % | 4,055 / 4,542 | 1900 |
| **prefill 6,962 tok** | **44.1 / 56.9** | 78 °C | **79 °C** | 100 % | 2,943 / 4,559 | 1900 |
| **prefill, 2nd** | **43.6 / 63.4** | 77 °C | **81 °C** | 100 % | 3,132 / 4,561 | 1900 |

**1. The GPU has nothing left to give.** `pp_od_clk_voltage` *is* exposed on this APU, and
its `OD_RANGE: SCLK 200Mhz 1899Mhz` is **the same number as the top DPM state** (`pp_dpm_sclk`
lists 200 / 1900 / 1899, and the active state under load is 1900).  A ceiling you are
already sitting on is not headroom; there is no offset to apply, only the risk of applying
one.  (The `sclk` column also showed 400 and 533 in some samples: the DPM does drop the GPU
when the verify window is waiting on the CPU pool - so `power_dpm_force_performance_level`
from `auto` to `high` is a *testable* arm.  It is not obviously a win: with the package
sitting at ~44 W of a ~45 W window, keeping 6 CUs clocked while they wait buys nothing and
steals watts from the pool.  Untested; `echo auto` reverts it, and this is the only clock
lever left that is OS-level and reversible.)

**2. Memory: no headroom, and the workload is not the kind it would help.** `dmidecode`
reads `CMSX64GX5M2A4800C40` at `Configured Memory Speed: 4800 MT/s`, `1.1 V` - the kit is
running at its rated JEDEC speed, it is a SODIMM laptop part with no XMP/EXPO above that,
and a Rembrandt APU gives no memory multiplier to argue with.  Even if it did, the plateau
above measured the CPU pool at ~21 GB/s because **Zen 3 has no AVX-512 and the expert
kernels are AVX2** - that is a compute limit, not a bus limit, so extra MT/s lands on the
half of the chain that is not waiting for bytes.

**3. The slack that is real is power, and it is 18 °C of it.** Sustained 43.6-44.1 W with
63.4 W of burst means the platform is clamping at a ~45 W window while the die sits at
77-81 °C of a 95 °C limit.  That is the textbook shape of a firmware clamp wasting thermal
margin, and raising `sPPT/fPPT/slowPPT` (ryzenadj `-a/-b/-c`, optionally a negative curve
via `--set-cogfx`) is the one change with physical headroom behind it.  Three things stop it
being a command I would run unsupervised:

- **The tool cannot see the thing it is changing.** `ryzenadj` 0.19 on kernel 7.2.10 here:
  `no compatible ryzen_smu kernel module found, fallback to /dev/mem … Unable to get memory
  access … Unable to init power metric table: -5`.  So the current limits cannot even be
  *read*, let alone verified after a write.  Doing this properly needs the out-of-tree
  `ryzen_smu` module (not in nixpkgs; a kernel-module input of its own) or `iomem=relaxed`
  - which weakens a security boundary on a machine this flake deploys to.  The one meter
  that does work is `power1_input` in this probe, so an experiment is *judgeable* by
  watts+tok/s even while blind to the SMU values.
- **Unmeasured silicon.** Nothing here reports VRM or mosfet temperature; `nvme` sits at
  44-55 °C in the same airflow.  Pushing 60-70 W continuously through a small chassis
  without that reading is how you trade a fast endpoint for a dead board - and a hard hang
  here costs a power cycle at the box, because there is no console and no BMC.
- **It builds for itself, and for nobody else since 2026-11-23.** It used to compile every
  machine in this flake, which meant a limit that made the box unstable delayed everyone's
  deploys. That risk is now this box's own — and the *other* risk, the engine losing a RAM
  contest to a build, is what the cap and the reservation exist for. The failure was never
  auto-rollback-shaped (the box does not boot into anything bad; the endpoint just stops).

If it is ever done, do it as `just switch`-able state (a unit that writes the limits, so
removal is a switch), one step at a time (45 → 54 → 65 W), with this probe running and
`strata-bench.py` as the measurement, and stop at the first sign of the clocks *dropping*
(thermal re-limit is a loss, not a plateau).

**4. The cheapest untried thing is not a clock at all.** `amd_pstate-epp` reports
`balance_performance` (the kernel default; nothing in this flake sets it) with
`scaling_max_freq` at the full 4,566 MHz - so the OS is not clamping, but EPP `performance`
raises boost aggressiveness for exactly the AVX2-bound pool that owns ~50 % of the verify
window.  Free, declarative, reversible; unmeasured here.  One arm of `strata-bench.py` with
`echo performance > /sys/devices/system/cpu/cpufreq/policy0/energy_performance_preference`
answers it, revert with `balance_performance`.

**5. And the log handed over a better lead than any clock.** Every start-up says:

> `expert arena: cudaHostRegister PORTABLE ok; MAP_HUGETLB unavailable (needed 16907 2 MiB
> pages, vm.nr_hugepages=0); transparent huge pages NOT requested: only 10 GiB of free 2 MiB
> blocks for a 33 GiB arena, compaction would stall the load (STRATA_ARENA_THP=1 requests
> them anyway) (#2126)`

**The 33 GiB arena the CPU pool streams through is running on 4 KiB pages**, and upstream
*refuses* to ask for THP because on a box with under half the arena available in free 2 MiB
blocks the request costs minutes of per-fault compaction (its #2126: 148 K failed
compactions, 100 % system time, "the load never ended").  aibox is 10 GiB-of-16.5-needed
side of that line, so the refusal is correct behaviour, not a miss.  But it leaves a real
hypothesis: the plateau's central number - the pool at ~21 GB/s against upstream's ~40 GB/s
reference - is a per-byte *time* measurement that TLB walks over 33 GiB of 4 KiB pages would
directly explain.  The clean version is a **boot-time** huge-page reservation (before
fragmentation, no compaction: `vm.nr_hugepages` sized to the arena, or `hugepages=` on the
kernel line), which makes `MAP_HUGETLB` succeed outright - and that is a RAM-capacity
decision against frigate, immich and whisper on the same box, not a flag.  Do **not** start
by setting `STRATA_ARENA_THP=1`: on a box already holding ~50 GiB that is the documented
stall, on the machine that builds the fleet.  If the huge-page experiment is ever run, it
should be measured as pool-GB/s and tok/s with this probe attached, and the honest
expectation is "maybe a few percent, possibly nothing" - a 28 GiB reservation is a large
price for a TLB effect that has not been isolated.

Re-measure with:

```
sudo machines/aibox/strata-tune/power-probe.sh /tmp/ap.csv &      # read-only
python3 machines/aibox/strata-tune/strata-bench.py https://strata2.int.leighhack.org
sudo pkill -f power-probe.sh
```

(Do not start the probe by backgrounding it inside the `ssh` command: it inherits the
session's stdout and the session then never exits - which is how this investigation spent
ten minutes believing aibox was dead.  Use `systemd-run --collect`, as the repo's detached
recipe says, and see `docs/nix-dev-workflow.md`, "Prove your own client before you diagnose
the remote box".)

## Update: engine 0.1.42 — 2026-10-11

0.1.40.2 → **0.1.42** (`strata-package.nix`, rev `61b3fb5`; the two releases between are the
0.1.40.3/.4 hotfixes and 0.1.41). Checked before bumping, from the source at both tags:

- **The pinned llama.cpp is unchanged** (`LLAMA_CPP_COMMIT = 3cf03257…` in `setup.py` at both
  tags), so the ggml/i-quant side is byte-for-byte what 0.1.40.2 built against.
- **`data/` is unchanged** (`git diff v0.1.40.2 v0.1.42 -- data` is empty), so
  `expert-profile.bin` and `draft_vocab.bin` are the same bytes: **no re-prep** of the pack or
  the MTP runtime.
- **The build interface is unchanged**: same cmake options, same `strata` / `strata-device` /
  `hip_mapped_alias` targets (the HIP test targets are still built whenever HIP is on, so the
  `strata-alias-check` install line stands), same `serve/ tools/ data/` layout, same run-config
  keys and engine flags. The serve layer's new `jsonschema` use is optional (imported on first
  use in `serve/structured.py`), so `serverPython` still needs no new package.
- **gfx1030 is still only "unvalidated"** (`cmake/hip_backend.cmake`) and gfx1035 is still
  refused, so `HSA_OVERRIDE_GFX_VERSION=10.3.0` and the gfx1030 build stay.

### The answers do not move (the two new defaults are CUDA-only)

0.1.41 and 0.1.42 each shipped an answer-changing default, and neither is compiled in for HIP:

- `STRATA_ROUTE_TAIL_SKIP=7` (0.1.42, +13-16 % decode on one NVIDIA GPU) —
  `const bool tail_default_ok = false; // HIP and SYCL stay as they were`
  (`src/program/generate.cpp:8078`).
- the measured PCIe share that replaces the start-up probe's number — the block that arms it is
  inside `#if !defined(STRATA_USE_HIP)` (`generate.cpp:2683`), so aibox keeps the probe's 0.55.
- 0.1.41's CPU prefill share (the one that would have changed bits here) — "the layer split,
  batch slots and HIP stay off" (`src/prefill/prefill.cpp:162-167`).

So the `--pcie-frac` sweep and the plateau numbers above stay the record, and there is no
"the answers changed under us" entry to write.

### Still open upstream: the gfx1030 verify timeout

Nothing in 0.1.41/0.1.42 touches the #267/#884/#1103 root cause (the ROCm barrier packet), and
`STRATA_HIP_ADAPT_KERNEL_COPY` is still read (`generate.cpp:171-181`), so **both env
mitigations stay enabled**. What did improve is the containment around a stall:

- **The serve layer now ends a frozen engine** (#1317/#1407): silent for
  `STRATA_ENGINE_STALL_S` (default 90 s) *and* no CPU/disk progress *and* the GPU idle → the
  engine is ended and restarted, so the request fails instead of the box holding a wedged GPU
  forever. It needs `psutil`, which `serverPython` already ships, and an engine that is silent
  but working is only warned about.
- **The engine's own step watchdog is more tolerant of slow storage** (#1407): the 60 s
  `STRATA_WATCHDOG_S` limit now gets up to 10× (`STRATA_WATCHDOG_IO_S`) while the file tier is
  still being read — the right shape for a box whose 20k-token prompt is ~7 minutes.
- 0.1.41's #795 work ("crash on a CPU without AVX-512") moved the AVX-512 probe out of the
  wide-instruction files and now disassembles the kernels in CI. aibox runs the AVX2 expert
  kernels, so that is exactly the class of bug that could have landed on this box.
- Upstream now agrees with our explicit expert cache: setup writes a *number*, not `auto`, on a
  unified-memory APU (#1715).

### One flag added: `--ple-io direct` is now explicit

0.1.42 probes the n-gram table at start (<0.6 s) and, below 15 000 rows/s, switches
`--ple-io direct` to `mmap` itself — silently unless `--ple-io` was passed, in which case it
only warns (`generate.cpp:3090-3113`). `mmap` means the page cache holds the 28.8 GB table on a
box that runs with ~5 GiB available, and the profile has never shown a PLE I/O stall
(`waitA` 0.24 ms). The unit therefore passes `--ple-io direct` explicitly: the probe still runs
and still logs, it just cannot change the measured config.

### Deploying

Built and committed here; the switch restarts the engine, and the request in flight at that
moment is the only casualty (as with the 0.1.40.x bumps). The number to compare against is the
plateau: **225-231 ms/window = 9.4-9.6 tok/s** at 4096 slots, so `strata-tune/run-arms.sh`'s
control arm is the re-measurement — anything much outside that band is a regression to bisect,
not a win to enjoy.

## Update: engine 0.1.43.1 — 2026-10-13

0.1.42 → **0.1.43.1** (`strata-package.nix`, rev `a0116b2`). Checked at both tags before bumping:

- **The pinned llama.cpp is unchanged** (`LLAMA_CPP_COMMIT = 3cf03257…` at both tags), so the ggml /
  i-quant side is byte-for-byte what 0.1.42 built against.
- **Nothing the pack or the MTP runtime depends on moved**: `data/expert-profile.bin` and
  `data/draft_vocab.bin` hash identically at both tags (the only `data/` file that changed is the new
  `expert-profile-sycl.bin`), so **no re-prep**.
- **The build interface is unchanged**: same cmake options, `strata` / `strata-device` /
  `hip_mapped_alias` all still built (the HIP test targets are still outside `STRATA_BUILD_TESTS`, so
  the `strata-alias-check` install line stands), same `serve/ tools/ data/` layout, same run-config
  keys, and every flag the unit passes still exists. `cmake/hip_backend.cmake` and
  `src/core/device.cu` are byte-identical — gfx1030 is still "unvalidated", gfx1035 still refused —
  so `HSA_OVERRIDE_GFX_VERSION=10.3.0` and the gfx1030 build stay. `requirements.txt` is unchanged and
  `jsonschema` is still optional (tests only), so `serverPython` gains nothing.

### The answers do not move: every new default is gated on gfx12

0.1.43's decode push was measured on RDNA4 and is armed only there, by one function —
`decode_defaults_arch_ok()` (`generate.cpp:1609`) returns true on HIP only when
`gfx_arch_is_gfx12(gcnArchName)`. aibox reports gfx1030, so everything it gates stays off:

- `STRATA_ROUTE_TAIL_SKIP=7`: the whole block is inert here (`tail_default_ok`, `generate.cpp:8303`),
  including the new "nearly every expert cached ⇒ no default" rule (`STRATA_TAIL_SKIP_MAX_CACHED`,
  90 % on a single stage, 60 % on a HIP layer split, #1884).
- the measured PCIe share that replaces the probe's number (`o.pcie_frac_auto`, `generate.cpp:2805`)
  — aibox keeps the probe's 0.55 and `--pcie-mode direct`.
- `--pipeline-windows`, which now defaults to auto-on for a layer split of 3+ stages
  (`generate.cpp:3397`): gated on gfx12 *and* on a multi-GPU split, so aibox gets 0.
- the three new HIP kernel fusions — `STRATA_QFUSE_HC` (`verify.cpp:141`), `STRATA_FUSE_WAIT`
  (`verify.cpp:229`), `STRATA_FUSE_HITS` (`verify.cpp:260`) — are all gfx12 defaults.
- `--prefill auto` may reach 16384 on gfx12 (`generate.cpp:2208`); aibox passes an explicit
  `--prefill 8192`, which that rule never touches.

`STRATA_ADAPT_OVERLAP` is opt-in on every backend (0.1.43 had it default-on for Linux CUDA; 0.1.43.1
pulled that back after an RTX 5090 Laptop decoded 21 % slower with it, #1964/#1975) and the unit does
not set it. So the plateau numbers above are still the control.

### What does apply here

1. **The GPU host-callback thread is created before the CPU pool pins itself**
   (`STRATA_ADAPT_CALLBACK_WARMUP`, on by default, `generate.cpp:4796`). Linux threads inherit the
   creator's CPU mask and the host pins itself to one CPU later, so a callback thread the driver
   creates lazily after that runs on the pinned CPU and fights the host's spin-wait — 985 µs instead
   of 4 µs per callback. This is the only default-on change that can move aibox's measured decode,
   and it moves it the right way.
2. **The stall watchdog samples the file tier every second, and "busy within the last 10 s" counts as
   slow, not stuck** (`generate.cpp:9117-9133`) — the right shape for a box whose 20k-token prompt is
   ~7 minutes. `STRATA_WATCHDOG_S` / `STRATA_WATCHDOG_IO_S` are unchanged, and both gfx1030 env
   mitigations (`STRATA_HIP_ADAPT_KERNEL_COPY`, `HSA_USERPTR_FOR_PAGED_MEM`) are still read.
3. **Non-finite logits end a request instead of emitting garbage** (#879): a verify window whose head
   logits held a NaN or an infinity ends as `DONE … nonfinite`, none of that window's tokens are
   written, and every cached state that may carry the poison is dropped, so the next request reads its
   prompt from token 0. The serve layer reports it as `EngineNonFinite` and retries a request that had
   produced nothing yet.
4. **The PLE probe can no longer change the config.** 0.1.42 switched `--ple-io direct` to `mmap` by
   itself below 15 000 rows/s; 0.1.43 stopped choosing on its own on every platform (#2040 — a SATA SSD
   tops out at 10-14K rows/s with direct's 16 reads in flight, and mapped reads were 7× slower there),
   so the probe only warns and the switch is opt-in `STRATA_PLE_AUTO_MMAP=1`. `--ple-io direct` is no
   longer a guard; it is the measured config stated out loud, which is why it stays in `strata.nix`.
5. Serve-layer odds and ends: `Retry-After` on the 503 when the model is unloaded and the GPU is busy
   (`/unload?wait=N`, #1997), and a client that hangs up while its request is being prepared is
   `ClientGone` (#1999) rather than a half-served request.

### Deploying

Committed here — **not built and not deployed**, and nothing was run against the live engine. When it
is deployed the switch restarts the engine and the request in flight is the only casualty, as with the
0.1.40.x and 0.1.42 bumps. The number to compare against is the plateau, **225-231 ms/window =
9.4-9.6 tok/s** at 4096 slots, so `strata-tune/run-arms.sh`'s control arm is the re-measurement: if it
lands inside the band the bump changed nothing measurable and item 1 above is the only candidate for a
gain, and anything outside it is a regression to bisect rather than a win to enjoy.

## Update: un-parked — engine 0.1.43.1 → the flake input (0.1.52), ROCm 10, and a front-door name (2026-10-28)

The module came back for the same reason it was parked: this is the box's reason for
existing, and twelve days of being nothing but a builder did not use the card. Three
things changed at once, so this section is what a *bump* of this engine costs, written
as the checklist it should have been.

### What the engine now is

`strata-package.nix` takes its source from the flake's `strata` input (rev `5e8a4cca`,
`CMakeLists.txt: project(strata VERSION 0.1.52}`, upstream's `v0.2.7` app release) —
**the same tree services2's CUDA engine builds from**, so `nix flake update strata`
moves both engines. Before this, aibox pinned its own `fetchFromGitHub` at `a0116b27`
(0.1.43.1) and the two had drifted nine releases apart, which is the gap the
`flake.nix` input comment used to describe as a feature.

Checked at both tags before switching, from the trees and not the changelog:

- **The pinned llama.cpp is unchanged** — `setup.py:222 LLAMA_CPP_COMMIT =
  "3cf03257…"` at both tags, and `third_party/ggml/VERSION.txt` in the new tree names
  the same rev — so the ggml / i-quant block-layout side of the build is byte-for-byte
  what 0.1.43.1 built against.
- **`data/` moved nothing we read**: `expert-profile.bin` (`8f59b4aa…`),
  `expert-profile-coder.bin` (`a5146611…`) and `draft_vocab.bin` (`b1e1d3a7…`) hash
  identically at both tags. The only additions are `data/dsv41/` and
  `data/quant_quality/` (DeepSeek V4.1-Flash and a quant-quality tool — nothing this
  pack touches). **No re-prep**: the pack under `/home/leigh-admin/Strata/pack/iq2xs`
  and the MTP runtime under `…/mtp/rt`, prepared 2026-10-05, are the right bytes for
  this engine.
- **Every engine flag the unit passes is still parsed** (`--pack`, `--native`,
  `--ple-gguf`, `--ple-io`, `--expert-profile`, `--expert-cache`, `--prefill`,
  `--spec`, `--spec-min-p`, `--mtp`, `--max-context`, `--kv`, `--kv-resident`,
  `--pcie-mode`, `--pcie-frac` — grepped as string literals in `src/program/generate.cpp`
  at the pinned rev), and every cmake option the package passes is still an option,
  `STRATA_PREFILL_MMQ` (HIP-only) included.
- **The arch gate is unchanged**: `cmake/hip_backend.cmake` still refuses gfx1035 and
  still lists gfx1030 under `_strata_hip_unvalidated`, so the gfx1030 build plus
  `HSA_OVERRIDE_GFX_VERSION=10.3.0` stay, and `hip_mapped_alias` is still built
  whenever HIP is on (so the `strata-alias-check` install line still stands).

### The one thing that would have crash-looped, and the guard that now catches it

From 0.1.46 `serve/` imports top-level packages the old copy list did not have:
`serve/server.py:69-78` and `serve/lifeline.py:31` import `strata_control` at module
scope and `strata_control` reaches `strata_cli`. aibox's package copied exactly
`{serve,tools,data}`, so **the first boot of a 0.1.52 tree would have died in a
`ModuleNotFoundError` crash-loop** — the identical failure services2 met on
2026-10-20, nine days before it bit here. The package now installs
`strata_control/` and `strata_cli/` beside them and runs a contract guard in the
install phase:

```sh
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=$out/share/strata python3 -c \
  'import serve.server, strata_control.procs, strata_cli.client'
```

That is the pattern worth keeping off a changelog: it turns "the service crash-loops on
a box whose model load takes minutes" into "the build fails in two seconds".

### Nothing new turns on for this card — except one thing that costs RAM

The 0.1.4x–0.1.5x answer and speed defaults are gated on RDNA4 and stay off here:
`gfx_arch_is_gfx12()` guards the tail-skip and the measured PCIe share
(`generate.cpp:2727`, `:3406`) and `STRATA_HIP_QUEUE_PRIME` is "gfx1200 / gfx1201 on
Linux only" (`generate.cpp:2712`); aibox reports gfx1030. So the plateau numbers above
are still the control, and both #884/#750 mitigations
(`STRATA_HIP_ADAPT_KERNEL_COPY=1`, `HSA_USERPTR_FOR_PAGED_MEM=0`) are still read and
still set.

What did change is invisible and expensive: **#3493** made `--conversation-cache-mib`
default to -1, meaning `--serve` sizes a host-RAM parked-conversation cache at 40 % of
(free − 2560 MiB) in 256 MiB steps, capped at 8192, *computed after the model loads*
(`include/strata/core/conversation_memory.hpp:70`). Measured against the running unit
rather than estimated: `free -m` reads **6964 MiB available** with the arena and cache
loaded, so the rule computes `(6964 − 2560) × 2/5 = 1761`, steps it down to 256, and
answers **1536 MiB** of RAM the build farm wants. `strata.nix` passes
`--conversation-cache-mib 0` — which is not a change of behaviour but the spelling of
the engine this unit last ran (0.1.43.1 had no parked conversations). Before "just set a
number": `available_host_bytes()` is MemAvailable/cgroup-limited and never counts swap,
and the same 2560 MiB floor refuses *parking* as well as sizing — worked out end to end
for the CUDA box in `machines/services2/STRATA.md`, "The conversation cache cannot be
bought with the swap file", and it reads identically here. The R9700 box runs 16 GiB of
that cache because parked conversations are what makes alternating agents cheap; on a
bandwidth-bound box with a 45 tok/s prefill
ceiling and 10 GiB of headroom, it is a dial to turn on deliberately, not to inherit.

### The gfx1030 prefill lever upstream just added (measured: refused here)

`src/prefill/rocblas_tuning.hpp` says it plainly: "rocBLAS picks a kernel per GEMM shape
from its own table, and on gfx1030 that pick is up to 6x slower than the best kernel the
library holds for the same shape". It is read through `STRATA_ROCBLAS_TUNING` (a table
file, or a directory holding `<arch>-rocblas-<version>.txt`), upstream ships
`tools/hip/gfx1030-rocblas-5.6.0.8d1ae90e.txt`, and `tools/hip/tune_rocblas.cpp` makes
the table. The table is refused unless arch **and rocBLAS version string** match, and
nixpkgs' rocBLAS reports `5.2.70203` (`librocblas.so.5.2.70203`) — so **the shipped
table does nothing here** and rocBLAS's own picks run, same as last time.

This is the cheapest untried prefill win this box has — prefill is the ~45 tok/s wall
that makes a 20k-token prompt a 7-minute prompt — and the work is one run of
`tune_rocblas.cpp` against nixpkgs' rocBLAS build, not a rewrite. Open item in
`docs/follow-ups.md`.

### ROCm 10 on an RDNA2 card: what is true, measured 2026-10-28

Requested as "use ROCm 10 if you can", so: **nixpkgs cannot express it and the card
makes it an experiment rather than a config line.**

- **nixpkgs has exactly one ROCm.** `pkgs/top-level/all-packages.nix` defines
  `rocmPackages = recurseIntoAttrs (callPackage ../development/rocm-modules {})`, its
  `rocm-core` is **7.2.4** at this flake's nixpkgs, and there is no
  `rocmPackages_8/_9/_10` anywhere in the tree. So ROCm 10 has to arrive as a runtime,
  not an attribute: `rocm10-runtime.nix` streams one layer of the
  `rocm/dev-ubuntu-24.04@10.0.0-full` image out of the registry (8 050 797 785 bytes —
  and note the fixed-output hash of that stage *is* the layer digest, since an OCI
  blob's sha256 is what the manifest publishes: `nix hash convert --to sri <digest>`
  derives it) and filters `lib/` out of it (16 GB, 22 997 files). `strata.nix` hands it
  to the engine through the run config's `lib_dirs`; serve's `child_env()` prepends it
  to `LD_LIBRARY_PATH`, which beats the RUNPATH. `services.strata.rocm10Runtime` is the
  switch, and it costs one restart, not a rebuild of the engine.
- **Why:** upstream #879, the non-finite / poisoned verify window, is a *runtime* fault.
  On the R9700 box the same binary and source fired it 2 times in 1 399 requests on
  ROCm 7.2.3 and 0 times in 1 911 on 10.0.0 the same day (`cjdell/nixos-config`,
  `hosts/zen3-nixos/ai/rocm10-runtime.nix`, whose `docs/strata-degeneration.md` is the
  write-up). aibox runs the same 7.2.x series.
- **Why it is not a given here:** Strata's own `docs/AMD_HIP.md` ships its Linux
  `rx6000` package — gfx1030/gfx1031, this card — on TheRock **7.13.0** wheels with
  "the gfx1030 wheels are unvalidated; a system ROCm 7 is the tested path there", and
  RDNA2 gets a 10.x package only on **Windows**. So the question was "does this card
  run on it at all", and the cheap answer is `strata-device`, which costs seconds and
  loads no model:

  | check | nixpkgs ROCm 7.2.4 | ROCm 10.0.0 (TheRock) |
  |---|---|---|
  | `strata-device --list-devices` | `gfx1030, 52.1 GiB, wave32` | same |
  | `--selftest` `driver / runtime` | `70253211 / 70253211` | **`71526333 / 71526333`** |
  | `--selftest` result | OK (allocations, over-allocation refused) | OK |
  | `ldd bin/strata` under it | nixpkgs 7.2.4 for all five | **all five from the ROCm 10 path**: `libamdhip64`, `libhsa-runtime64`, `libhipblas`, `librocblas`, `libhipblaslt` |
  | `strata-alias-check` (the `--pcie-mode direct` gate) | alias == host, kernel reads it correctly | identical |
  | hipBLASLt Tensile kernels for gfx1030 | none (unused: gfx1030 has no hipBLASLt kernels) | none — `lib/hipblaslt/library/` is gfx1100 and newer |
  | rocBLAS Tensile kernels for gfx1030 | yes | **yes** — `lib/rocblas/library/*gfx1030*`, 144 files, and `lib/rocblas/library/` spans gfx1010…gfx1250 |

  The last two rows are the whole risk and they came out the right way: the dense path
  on this card runs on rocBLAS, and the runtime that now supplies it does carry gfx1030
  code. `rocm10-runtime` writes its own answer into `$out/RUNTIME-CODE.txt`, so the
  next reader does not have to trust this table.

  Cost of the pull: 2 min 55 s of download at ~43 MB/s plus a re-extract, 24 GB of
  store for the blob and the runtime on a box that was at 81 % — which is why
  `allowSubstitutes = false` and "build aibox on aibox" are in the header, and why
  `nix gc` (`--delete-older-than 30d`, `common/nix-gc.nix`) will take the 16 GB
  back if the service is ever parked again.

### The endpoint, and why there is no nginx here

`strata.nix` used to end with `services.nginx.virtualHosts."aibox.int.leighhack.org"
.locations."/llm/"`. That is wrong twice over now: this box has no nginx (see the
headers of `frigate-monitor.nix` and `status-dashboard.nix`), and the front door on
`gw` owns every name the network has one of. The engine is therefore one row in
`common/front-door.nix` — `strata2.int.leighhack.org` → `http://10.3.1.32:8080`,
`strataBearer = true`, 1024M body, 3600 s timeouts, no buffering — plus its label in
`common/int-dns.nix`; the guard in `machines/gw/int-dns.nix` is what refuses a door
that serves a name DNS does not answer, and `nixos-rebuild dry-build --flake .#gw` is
clean with both. `strata2` because `strata` is services2's engine: different box,
different quant, two names for two models.

The engine also has an `api_key` now (it said so itself for a year: "WARNING: no API
key - anyone on your network can use this model", and this box's firewall is off).
It is `strata_api_key`, the same secret services2's engine holds, because
`strataBearer` in the door renders exactly one `proxy_set_header Authorization` — a
per-engine key would 401 the door's own path. Clients that used
`http://10.3.1.32:8080/v1` directly now need that key; the pi clients are pointed at
`https://strata2.int.leighhack.org/v1` instead, where the door injects it and no client
holds a secret (`common/pi-endpoints.nix`, and the finding in `machines/aibox/STRATA.md`
"Pi clients" — the direct addresses had crept back into four boxes' `models.json`, with the
key beside them).

**And the trade that is not free:** enabling strata clears `llama-server`'s
`wantedBy`, so `ai.int.leighhack.org` and `ai.leighhack.org` — origin vhosts on
services1 proxying to `10.3.1.32:8081` — answer 502 while the engine has the box.
`systemctl stop strata && systemctl start llama-server` is the swap, and `strata.nix`
keeps `services.strata.displaceLlamaServer` as the switch for it.

### Deploy record, 2026-10-28 (the first run of this config on this box)

`nixos-rebuild switch --flake .` at 02:12, confirmed against the same generation. What
the running service says, which is the part a bump is really judged on:

- Engine came up in ~25 s of wall clock: `experts loaded: 33.02 GiB at 3.82 GiB/s (18 s
  so far)`, then `filling the GPU's expert cache (4292 experts, 5.76 GiB of VRAM)` — the
  `--expert-cache 4096` rounding the engine has always done, unchanged by the bump. The
  budget at the top of this file says 4096 slots land at ~50 GiB used and ~10 GiB of
  headroom; with 262144 context measured on the deployed unit it is **54 GiB used, 6.9
  GiB available** and 15.1 GiB of GTT, so read the headroom as ~7 GiB when sizing what
  else this box does.
- **ROCm 10 really is the runtime**: the engine process maps
  `…-rocm10-runtime-10.0.0/lib/libamdhip64.so.7.15.26333-0000000`, where the same binary
  without `lib_dirs` reports runtime `70253211`. Steady-state footprint as before: GTT
  15.10 GiB (`mem_info_gtt_used`, the 262144-context figure in the UMA section) and 52
  GiB of 59 used, ~6 GiB available — i.e. the headroom the header above this section used to
  warn about (and which is now why a build here is capped),
  measured.
- **Decode is not slower on the new engine or the new runtime**: 220 completion tokens in
  18.3 s through `/v1/chat/completions` ≈ **12 tok/s**, at or above the 9.4-9.6 tok/s
  plateau, with `finish_reason: length` and correct output. One `#267`-class event in
  1.5 days of the old engine remains the fault record; nothing to add yet on this one.
- The key is enforced: `/health` says `"api_key": true`, `/v1/models` is a 401 without it
  and the model list with it, and the serve banner now ends `API key required`.
- `strata.service` has `NRestarts=0` since the switch.

Two operational notes from the switch itself:

**The door half, same day.** `just deploy gw` landed 210731b on the router (the recipe
fetches, switches and confirms in one ssh call; `system-good` and `/run/current-system`
equal afterwards, `systemctl list-units --failed` empty, nginx and dnsmasq restarted
to pick up the new row and the new hosts file). Per AGENTS rule 22 the mandatory
follow-up is `systemctl start front-door-cert-check.service`, and it read
`leighhack.org is CA-issued and not near expiry: issuer=C=US, O=Let's Encrypt, CN=YE2`
— the only check that distinguishes a real certificate from the minica fallback.
What the name then does, from a LAN client with **no key of its own**:

```console
$ getent ahostsv4 strata2.int.leighhack.org
10.3.1.1  STREAM strata2.int.leighhack.org
$ curl -s https://strata2.int.leighhack.org/v1/models | head -c 80
{"object": "list", "data": [{"id": "qwen3.8-flash-next-iq2xs", …
$ curl -s -o /dev/null -w '%{http_code} %{time_total}s\n' -X POST \
    -H 'Content-Type: application/json' -d '{"model":"qwen3.8-flash-next-iq2xs",…220 tokens…}' \
    https://strata2.int.leighhack.org/v1/chat/completions
200 17.787979s      # 220 tokens = ~12.4 tok/s, the same rate as localhost
```

17.8 s for 220 tokens through TLS via the door is the same decode rate measured
straight off `10.3.1.32:8080`, so the extra hop, `proxy_buffering off` and the
injected `Authorization` cost nothing measurable. The first request after an idle
engine took 41 s for 24 tokens — that is the ramp, not the door, and it is the reason
the row carries `timeouts = "3600s"`.

- **The activation failed with exit 4 and it was not strata.** `Failed to restart
  exports-netboot\x2dsquashfs.mount` — the Pi Room netboot export's mount — killed
  `switch-to-configuration` *after* `/run/current-system` had flipped and after
  `strata.service` had started, with the auto-rollback timer already armed and firing in
  ~3 minutes. Re-running `/run/current-system/bin/switch-to-configuration switch` (the
  same activation, second attempt) exited 0 — the mount was busy at the wrong moment and
  stayed mounted and active throughout — and only then was `nixos-confirm` run, against
  the generation the switch meant. That is the documented ordering (never confirm a failed
  activation; a retry that exits 0 is what makes the confirm true), and it is worth
  re-reading `machines/aibox/netboot.nix` before the next switch here, because a
  busy-restart of that mount is a switch failure waiting to happen on a box whose netboot
  export NFS clients are real machines booting off it.
- **Stopping `llama-server` is part of un-parking, and the flake only does half of it.**
  `displaceLlamaServer` clears `wantedBy`, so the unit goes to `linked` and will not start
  at boot — but a unit already running is left running by an activation, and an idle
  llama-server that answers one request through `ai.*` will try to take ~33 GB of GTT the
  arena now owns. `systemctl stop llama-server` was run by hand after the switch; do the
  same on any future un-parking.

## Prefill 2.2× from `STRATA_HIP_PROMPT_F16`, and what the conversation cache actually parks (2026-10-28)

Two changes, one restart each, measured against the same fixed prompt in both arms
(`strata2.int.leighhack.org`, `temperature: 0`, one request in flight, ~59 KB of
pseudo-random prose = **13,069 prompt tokens**). Both are in `strata.nix`; both came from
the question "should we drop to 192K context to fund the conversation cache?", and the
answer measured out as **no** — the context change was never needed and neither of these
required it.

| | before (`F16` off, cache off) | after (`F16` on, cache 1536 MiB) |
|---|---|---|
| cold read, 13,069 tok | **349.7 s** = 37.4 tok/s | **158.4 s** = 82.5 tok/s |
| second fresh prompt, ~13K tok | 258.0 s = 50.7 tok/s | 158.0 s = 81.7 tok/s |
| decode | 8.5–10.6 tok/s | 10.4–12.4 tok/s |
| repeat of an identical 13K prompt | full re-read | **1.2 s** client-side, `restored 13064 tokens (checkpoint) in 37 ms` |
| same prompt **+30 tokens** (what an agent turn is) | full re-read | **full re-read: `reused 0`, 157.5 s** |
| parked bytes | — | 0 when nothing repeats; **436 MB per 13K conversation**; `parked=3 bytes=1.22 GiB evictions=1` at the cap |

### `STRATA_HIP_PROMPT_F16=1` — keep it, it is as advertised

The engine had been printing the offer on every start of this card and nobody had taken
it: "reads long prompts about 2x faster here (FP16 prompt GEMMs, rocBLAS is tuned only
for FP16 on gfx103x; the numbers differ slightly)". It is opt-in
(`src/prefill/gemm.cu:588`, #835) and when enabled it replaces the tip with
`STRATA_HIP_PROMPT_F16=1: the prompt's 16-bit GEMMs run FP16 in and out … not
bit-identical`. Measured: **2.21× on the cold read and 1.65× on the second**, across four
independent reads in the after arm (81.7–83.7 tok/s) against two in the before arm
(37.4, 50.7) — a spread far wider than the noise, so the effect is real even though the
noise is not small (below). Prefill is this box's wall, so a 100K-token prompt moves from
~45 minutes to ~20; that is the number that matters for agent use, and it cost one env
line and no RAM. Decode is untouched, as it should be for a prompt-path switch.

The cost is upstream's phrase, not a measured bound: **bit-identical no**. This box cannot
bound the difference — see the methodology note under the table's footnotes, greedy output
here is not reproducible run to run even with a fixed prompt and `temperature: 0`, so the
digest comparison that `docs/follow-ups.md` recommends for exactly this purpose is not a
usable instrument at this size.

### The conversation cache parks *identities*, not prefixes

On, with `--conversation-cache-mib 1536 --conversation-cache-slots 3
--conversation-cache-min-tokens 2048`. It works exactly as designed and *not* as
advertised in this file's own reasoning when it was chosen:

- A parked conversation restored in **37 ms** turned a 158 s re-read into a **1.2 s**
  turn, `parked=2 … bytes=1.07 GiB evictions=0`. Alternating two conversations, the second
  and later visits to each cost 1.2 s. That is a 130× win and it is real.
- A conversation whose prompt had **grown by 30 tokens** — i.e. every turn of every agent —
  got `reused 0` and paid the full 157.5 s. Parking matches on prompt *identity*; it is not
  a prefix cache. So this is not the cure for long growing contexts, and anyone reading
  "turn on the conversation cache" as that will be disappointed.
- The cap behaves: 436 MB per 13K-token conversation (`snapshot_bytes` in the engine's own
  line), so 1536 MiB ≈ three of them, and at `parked=3 bytes=1.22 GiB` the next park
  evicted one (`evictions=1`) rather than growing. With nothing repeating, `bytes=0`.

Kept, on that reading: it costs no RAM until something repeats, it demonstrably cannot
make the non-repeat case slower (the extension re-read measured *faster* than the baseline
by exactly the F16 factor), and identical-prompt retries are cheap in a way they were not.
It is not the reason this box has a 20-minute 100K prompt anymore — F16 is, and the next
real step is the gfx1030 rocBLAS table / `--resident-budget-gib`, not more cache.

### Methodology, because three things here nearly produced a fake result

1. **Cold must be compared with cold.** The *baseline's* two cold reads of the same-size
   prompt differed by 26% (349.7 s vs 258.0 s — the second had the page cache and PLE
   tables warm). Any effect under ~30% would have been unmeasurable with one pair; a 2.2×
   is not.
2. **Greedy output is not reproducible here.** Two runs of one fixed 200-token probe in the
   *same* arm, `temperature: 0`, produced different reasoning digests (`edf775d3…`,
   `e83db119…`). So an md5 of a completion cannot detect the FP16 rounding this switch
   introduces, in either direction; the file's own advice to "compare the md5" needs the
   within-arm repeat first (`docs/follow-ups.md` says so now).
3. **A probe that prints nothing is broken, not negative.** The harness filtered the engine
   log for `" strata serve: prompt "` with a leading space; the lines start at column 1, so
   both arms' first drafts reported `(no line)` while the timing next to them was fine.
   Recovered from the log by hand and fixed, but it is the same failure as a UI that writes
   "ok" when the button did nothing: the instrument's silence is not a result.

And one thing still unexplained: this file's older logs show `prompt 97819 tokens = 97781
reused + 38 read` on 0.1.43.1 with **no** parking available — so grown-prompt reuse exists
somewhere (the built-in chat endpoint, multi-message `messages` arrays), and the
single-user-message probes used here do not reproduce it. Worth chasing before believing
anything above about what does and does not reuse; filed in `docs/follow-ups.md`.

## Pi clients

**Superseded 2026-11-14 by `programs.pi-agent` — `docs/pi-agent-models.md` owns this now.**
What remains true of the description below is the *why* of the URLs; what is false is every
number in it, which is the reason the mechanism changed. `pi-models.json` in this directory
is deleted: it was the canonical copy of `~/.pi/agent/models.json`, hand-written, and asked
to carry exactly the two facts only a running engine knows — which models it serves and with
how large a context. Measured the day it was removed:

- `strata` claimed context **131 072**; the engine answers `meta.n_ctx` **262 144**;
- `strata-zen3` claimed `qwen3.8-flash-next-iq3xxs` at **262 144**; that endpoint now serves
  **`qwen3.8-flash-next-unsloth-ud-iq4-xs-v31`** at **393 216**, so the provider had been
  offering a model id that answers nothing, on every box, for as long as nobody noticed;
- and the copies on the boxes were no longer the copies in this file: they dialled
  `http://10.3.1.21:8080/v1` and `http://10.3.1.32:8080/v1` directly, which is how the real
  `strata_api_key` came to be sitting in `~/.pi/agent/models.json` on four machines. Rotation
  is the open item in `docs/follow-ups.md`.

Now: `common/pi-endpoints.nix` declares the endpoints (including the front-door URLs and why,
below), `common/pi-agent.nix` renders them and runs a timer per box, and
`crates/pi-agent-sync` discovers the rest. `just pi-agent-status` on any box prints what each
engine said and where each number came from.

What has **not** changed, and is the part worth keeping:

- `strata` is `https://strata2.int.leighhack.org/v1` on purpose. `strataBearer = true` on that
  door row injects the engine's `api_key`, so no client holds an inference secret and the
  placeholder `apiKey` never reaches anything that reads it. Pointing a client back at
  `http://10.3.1.32:8080/v1` is a 401 for anyone without `strata_api_key` — and putting the
  address in a client is also how the key ends up next to it, which is what just happened.
- `strata-zen3` (`https://llm.ai.chrisdell.info/v1`) is the fast fallback (51 tok/s decode,
  1184 tok/s prefill), worth using for anything with a big context until aibox's prefill gets
  better.

To make either the default, set in `~/.pi/agent/settings.json`:
`"defaultProvider": "strata"`, `"defaultModel": "qwen3.8-flash-next-iq2xs"`
(and drop `llamaServerUrl` / the `pi-llama-cpp` package if llama-server stays
displaced — it talks to llama.cpp's own endpoints, not Strata's).

## Update: engine 0.1.52 → **0.1.56** (the flake input), deployed — 2026-11-04

`nix flake update strata` moved the shared input `5e8a4cca7d690ac60ca37eb09e7d0e010099ad6d`
→ **`ffa80d73133ea5c548aefe57889a16e8085a938f`** (engine **0.1.56**, `CMakeLists.txt:11
project(strata VERSION 0.1.56 LANGUAGES CXX)`; app **0.2.10**, `apps/desktop/package.json:4`
`"version": "0.2.10"`). This is the **HIP side** of the bump services2's CUDA engine took
the same day; what changed on the CUDA side — including the one default that applies to
that box's sm_86 and turned *off* — is `machines/services2/STRATA.md`, "Update: engine
0.1.56 (app 0.2.10) — 2026-11-04".

### What the HIP engine at 0.1.56 actually is (read off the compile lines, not the changelog)

The build's own command line, from the running `aibox-hip-build` job:
`-DSTRATA_VERSION="0.1.56"`, `-DSTRATA_HIP_ARCHS="gfx1030"`,
**`-DSTRATA_VERIFY_MAX_T=16`**, `-DSTRATA_HAVE_DFLASH2=1`, `-DSTRATA_Q6K_EXPERTS=1`,
`-x hip --offload-arch=gfx1030`, toolchain clang 22 / clr 7.2.4 / device-libs 22.0.0.
The engine is `/nix/store/f0jwild16n6wjxjly1h1shslsrnw87x4-strata-unstable` and
`strata --version` prints `strata 0.1.56`.

Checked at the new rev **before** the switch, from the store source
(`/nix/store/yr7fhrbwnb7hwjclcf535kc486rhy7k5-source`, i.e. the flake input, not a
changelog):

- **The pinned llama.cpp is unchanged** — `setup.py: LLAMA_CPP_COMMIT =
  "3cf03257f219afbe7334045ff7c6a06ac68c627d"` and `third_party/ggml/VERSION.txt` names
  the same rev — so the ggml / i-quant block-layout side of the build is the same bytes
  every previous engine here built against.
- **`data/` moved nothing we read**: `expert-profile.bin` (`8f59b4aa…`),
  `expert-profile-coder.bin` (`a5146611…`) and `draft_vocab.bin` (`b1e1d3a7…`) hash
  identically at the new rev. **No re-prep**: the pack under
  `/home/leigh-admin/Strata/pack/iq2xs` and the MTP runtime under `…/mtp/rt` (prepared
  2026-10-05) are still the right bytes for this engine.
- **The install-phase contract guard ran and passed** (`import serve.server,
  strata_control.procs, strata_cli.client`). That guard exists because a 0.1.5x tree
  would otherwise have crash-looped this box in a `ModuleNotFoundError` — the identical
  failure services2 met nine days before it bit here; "the build fails in two seconds"
  is the point of it, and this bump exercised it.
- **The arch gate is unchanged**: `strata-device --list-devices` run *without*
  `HSA_OVERRIDE_GFX_VERSION=10.3.0` still refuses the card verbatim — "arch gfx1035 …
  is not an architecture this Strata engine was compiled for (gfx1030)" — so the override
  stays, and `--pcie-mode direct` is still gated by `strata-tune/alias-check.sh`: run
  after the deploy it answers `alias 0x78da28fc8000 → the host pointer itself (unified
  addressing)`, "a kernel reads the alias correctly **holds**", and the running engine
  reports **+14.3 % / +14.4 % of routed experts over PCIe** — the same shape as before the
  bump. (The one check it prints as "did not hold" is *the alias is a distinct device
  address*, which does not hold on Linux by design; the script says a Windows host is
  expected to fail some.)

### The two switches the bump turns for this card, and what each one is worth

| Dial | Gate, read where it is decided | What it costs here, measured |
|---|---|---|
| `STRATA_AMD_COMMUNITY_TUNING` (default **ON**) — the engine log now prints this line for this card **for the first time** | `include/strata/core/amd_community.hpp` — a file that does **not** exist in the 0.1.52 source still in this box's store (`/nix/store/vcaxcamli22j5frq81xbzxjl98lmc6g5-source`) and does exist in the new tree | Two halves that behave differently. **(a)** the community rocBLAS tables stay **inert**: the gate is an exact version match — "loaded when the running rocBLAS is exactly `5.4.0.ad84daa81c` or `5.2.0.dabb6df2b98-dirty` and `STRATA_ROCBLAS_TUNING` points at them" — and nixpkgs' rocBLAS reports **5.2.70203**. **(b)** the **simt2 QSA selector is now the default** (#3073, "community-tested only") — a different reduction order in the QSA layer, so **answers move slightly**. `STRATA_SELECT_SIMT = "0"` in `extraEnv` puts the old selector back with no rebuild. The log line, verbatim from `/var/lib/strata/strata.log`: `strata: AMD community tuning (STRATA_AMD_COMMUNITY_TUNING=0 turns it off): on: community rocBLAS tables for gfx1030 are loaded when the running rocBLAS is exactly 5.4.0.ad84daa81c or 5.2.0.dabb6df2b98-dirty and STRATA_ROCBLAS_TUNING points at them (#3343/#3924); the simt2 QSA selector is the default (STRATA_SELECT_SIMT=0 or 1 overrides) (#3073); community-tested only` |
| `STRATA_VERIFY_MAX_T` **8 → 16** | `CMakeLists.txt`'s `STRATA_WINDOW_MAX_T` — the variable does **not exist** in the 0.1.52 source on this box (`grep -c WINDOW_MAX_T` there answers 0), and that source has **no `include/strata/kernels/window_limits.hpp` at all**, so 8 was the only window that ever got compiled; the new CMake's `auto` answers **16 for HIP** (8 for CUDA) | **Nothing at this config's runtime.** The engine's own line is still `strata verify: window up to 6 tokens, 58.3 MiB of device buffers` — `--spec 4` caps the window at draft+1, so the wide instantiations are extra *code*, not a wider dial. What it does change is that this box now carries the wide instantiations, i.e. the #267 verify-window crash zone this directory records has more compiled shapes in it than the box ran before; pinning it back to 8 is a rebuild (`-DSTRATA_WINDOW_MAX_T=8` in `strata-package.nix`'s `cmakeFlags`), not an env line. |

### The control measurement (the plateau band is the control, per the ritual above)

`machines/aibox/strata-tune/strata-bench.py` driven **through the front door**
(`https://strata2.int.leighhack.org`; the door's `strataBearer` row injects
`strata_api_key`, so the bench holds no secret), against the deployed 0.1.56 engine:

```
short-decode    prompt_n=70     prompt/s=16.1   decode_n=200  decode/s=9.3    draft=101/149  wall=26s
long-prefill    prompt_n=6943   prompt/s=90.3   decode_n=30   decode/s=10.2   draft=18/25    wall=80s
long-prefill-2  prompt_n=6953   prompt/s=90.1   decode_n=30   decode/s=10.8   draft=18/22    wall=80s
```

Decode at **9.3 tok/s** sits inside the plateau band this directory's control is
(**9.4-9.6 tok/s = 225-231 ms per window** at 4096 slots), so the bump did not move decode
on this card — and that is the only comparison the two eras have in common. **The two cold
prefills at 90.1-90.3 tok/s are NOT comparable to the 82-84 tok/s recorded on 2026-10-28**
("Prefill 2.2×"): that was a **13 069-token** cold read and these prompts are **6 943 /
6 953 tokens**, and the per-token rate depends on the prompt length (the fixed start-up
amortises over it). Read it as *no measured prefill regression*, not as +8 %.

`draft=101/149` is MTP accepting; the start line says
`Drafting: MTP (the DFlash2 drafter isn't downloaded yet)` — `-DSTRATA_HAVE_DFLASH2=1`
compiles the block drafter in, but using it needs a `tools/dflash2_pack.py` pack and
`--draft dflash2`, which is **not** ours to inherit silently: DFlash2 stays an open item,
not a switch. Answers move (the QSA-selector half of the table above), so the house rule —
compare the *answer*, not the banner — applies to anything compared across this bump.

### What the CUDA side of the same bump did (recorded here for the pair, measured on services2)

The same input move put a **CUDA** engine on services2 as well (`nix flake check` on
aibox still builds `packages.strata-cuda`, and the bump needed it to stay buildable). Verified
there the same day: `strata.service` active, generation and `system-good` equal
(`i571xinhbg6ccpwk3qsm9zaz21rq0mr8-nixos-system-services2-…`), the engine reports
`session is up (engine 0.1.56)`, and `/v1/models` still answers one model
(`qwen3.8-flash-next-coder-iq1m`, `n_ctx 131072`), i.e. the pack did not change shape.
One `/v1/chat/completions` call at a 73-token prompt measured **23.4 tok/s** against that box's
own logged band of **26.4-28.2 tok/s** — same order, slightly low, and its log carries the
MTP acceptance lines (`accepted 344 of 512, 5 checkpoints`). **Its prefill was not
re-measured at this bump**: the chat-path attempt at a 31 641-prompt-token read returned an
empty answer and its `usage` numbers do not survive reading (a 1 100 tok/s prefill on a 3060
is a misread, not a finding), so no prefill claim is made for the CUDA side, and the bench
scripts that CUDA's ritual used are not in the repo (`machines/aibox/strata-tune/` is).

### Deploy record, 2026-11-04 (what a bump of this engine costs on THIS box)

- **The engine job is one derivation** (`kry29l9i1qawdkz30kw4bxm1yn8jca8p-strata-unstable.drv`):
  every ROCm/HIP dep was already in aibox's store, so unlike the 0.1.52 bump's 25-derivation
  CUDA job this one costs the engine's own compiles and nothing else. Run as
  `systemd-run --no-block --unit=aibox-hip-build` → `/tmp/aibox-hip-build.log`, exit 0.
  **`systemctl stop strata` before it**, as the header says: measured during the job, 57 GiB
  available / 2 GiB used; with the engine loaded the box has ~7-10 GiB and a HIP engine
  build does not fit in that.
- **A plain `just switch` on this box fails, and the failure reboots the box.** The netboot
  export's `/exports/netboot-squashfs` bind mount is busy — activation stops and starts it
  and the unmount answers `umount: /exports/netboot-squashfs: target is busy` (status 32)
  — so `switch-to-configuration` exits **4** (`Failed to restart
  exports-netboot\x2dsquashfs.mount`). Measured: 16:13:30 the failed unmount, the new
  generation already current, and because the activation had made it current
  **auto-rollback rebooted aibox back into the previous generation** (16:19:03 the rollback
  unit, 16:20:02 the old mount remounting), after which `/run/current-system` was the old
  generation and `strata.service` was running the **old** engine (`2i0ncx0s…`, 0.1.52).
  This is `docs/network-upgrade-runbook.md`'s known trap ("The netboot export makes every
  aibox switch fail, and the fix is the umount, not the reboot") — the fix is that doc's
  dance, i.e. `just switch-netboot`'s first line, and not a new idea. What that doc did not
  yet say is the consequence: a status-4 activation **arms the rollback countdown on a
  deployable box**, so the retry has to be planned, not improvised under a reboot.
- **The landing attempt, one unit, chained confirm** (`aibox-sw2`, script
  `/tmp/aibox-sw2.sh`, log `/tmp/aibox-sw2.log`): `umount -f -l /exports/netboot-squashfs`
  → `nixos-rebuild switch --flake .` (**exit 0**) → **`nixos-confirm` in the same invocation**
  (exit 0) → `mount -a`. That switch built **1 154 derivations and copied 2 087 paths** — aibox
  was behind on the SPA/wasm rebuilds, the pi-room netboot image (`nixos-system-pxeclient`,
  `squashfs.drv`) and the CUDA-era packages — but the **engine derivation and the run config
  were already in the store**, which is what makes a strata bump cheap on aibox once the
  toplevel has been carried.
- Landing numbers, from the switch's own journal: the new generation
  `/nix/store/wgnqn5a1lnnd5xn5bmsgxjfs3bcyvxk5-nixos-system-aibox-26.11.20261014.319e4ce`
  is **both** `/run/current-system` and `system-good`; **0 failed units**; the activation
  restarted `strata.service`, which reloaded at **33.02 GiB at 4.30 GiB/s (14 s)**,
  `filling the GPU's expert cache (4292 experts, 5.76 GiB of VRAM)`, `ready: http://10.3.1.32:8080/v1
  … context 262144 tokens, API key required`, and the engine's log says `session is up
  (engine 0.1.56)` with **9234 MiB of VRAM free with everything loaded**.
- **The run config moved only where the engine moves.** Diffed the previous generation's
  run config (`qb7bsqxc0id215sh6y0zpmdg51crysgg-strata-iq2xs.json`) against the new one
  (`1ndcfbc6iq4hck44p5cgv2wyjybkq3h3-strata-iq2xs.json`): exactly two keys differ — `exe`
  (the engine store path) and the `--expert-profile` path inside `args` (it lives *in the
  engine package*). Every flag and every env line is byte-identical, so `STRATA_HIP_PROMPT_F16=1`,
  `--pcie-mode direct`, `--conversation-cache-mib 1536` (+ slots/min-tokens), `--spec 4 --spec-min-p 0.5`,
  `--expert-cache 4096`, 262 144 context, and the #884/#750 mitigations are **our dials, unchanged**
  — the F16 prompt path is ours and was already on, not a new default from this bump.
- **The pi Room clients were not rebooted.** The runbook's "don't" (rebooting ten clients
  unprompted) is what stopped `just switch-netboot`'s client-reboot loop here; the switch did
  rebuild the image and re-export it, so the clients take the new image on their next boot,
  and nothing in `machines/pi-room-sys/` changed in this commit. Open, deliberately: the
  clients are still booting the previous image's closure.

## Update: engine 0.1.56 → **0.1.59.1** (app 0.2.13), deployed — 2026-11-14

The shared `strata` input moved `ffa80d73133ea5c548aefe57889a16e8085a938f` →
**`13eedf552417a6a791a7d13c6597e24300ee3a09`**: engine **0.1.59.1** (`CMakeLists.txt:13
`project(strata VERSION 0.1.59.1 LANGUAGES CXX)`), app **0.2.13** (`apps/desktop/package.json:4`),
upstream head authored 2026-11-11 / pushed 2026-11-12, **682 commits**. One `nix flake update strata`
and both of this network's engines move in the same switch, which is why this section exists next to
the CUDA one.

**The bump's size is written up once, on `services2`, and not repeated here**:
`machines/services2/STRATA.md`, "Update: engine 0.1.56 → 0.1.59.1 (app 0.2.13), deployed — 2026-11-14"
— the 682-commit range, the tree diff (**606 files differ, 299 added, 115 removed** — the first bump
in either file's history with removals, all of them `apps/host/docs/**`, `docs/work/**`, `tools/dgx_*.py`
and friends: nothing the install phase copies), the `CMakeLists.txt` **2,950 → 645** split into
`cmake/*.cmake` + `cmake/backends/*.cmake`, `third_party/strata-ggml` vendored at the same
`3cf03257f…` pin `strata-package.nix` already fetches, the re-check of all 13 `engineArgs` flag pairs,
the `configFile` keys, `/v1/messages`, `reasoning_content`, the Host check, the 274-module import
closure, the unchanged data md5s (**no re-prep** of pack / MTP runtime / expert-profile) and the
unchanged IQ1_M reader. Both engines were built from this lock with **no edit to either
`strata-package.nix`** and both installed clean, guard included.

What follows is what is different on this side: the HIP gates, the control measurement, one genuinely
new instrument for this box's oldest open fault, and the deploy shape an aibox switch has to have.

### Nothing in this bump is for gfx1030, and the engine's own lines say so

Four header files are new this bump — `include/strata/core/{gpu_pause_time,pcie_share_rule,nv_chunk,
resident_ram_copy}.hpp` are **all absent from the 0.1.56 tree** (`ls include/strata/core` in both) — and
of those four, exactly one touches this card, and it is not a speed switch. Everything else is named
for a different device. Comparing this box's own two starts, `0.1.56` at 13:55 and `0.1.59.1` at
02:54, from `/var/lib/strata/strata.log`:

| Line | 0.1.56 | 0.1.59.1 | Reading |
|---|---|---|---|
| `strata: AMD community tuning (STRATA_AMD_COMMUNITY_TUNING=0 turns it off): on: community rocBLAS tables for gfx1030 are loaded when the running rocBLAS is exactly 5.4.0.ad84daa81c or 5.2.0.dabb6df2b98-dirty and STRATA_ROCBLAS_TUNING points at them (#3343/#3924); the simt2 QSA selector is the default (STRATA_SELECT_SIMT=0 or 1 overrides) (#3073); community-tested only` | present | **byte-identical** | Not a new consequence of this bump: the simt2 selector (whose "answers move slightly" is recorded under 0.1.56) was already in force, and the community rocBLAS tables stay inert because nixpkgs' rocBLAS reports **5.2.70203**, which is not one of the two exact pins. `amd_community.hpp` exists in the old tree |
| `strata generate: PCIe probe: 31.5 → 31.4 GB/s host->device … -> pcie_frac 0.55 (default 0.55)` | 0.55 | **0.55** | The new auto-share rule (`pcie_share_rule.hpp`, k 1.85→0.8, ceiling 0.60→0.95) is gated on **one CUDA GPU without `--pcie-frac`**, and integrated cards keep the old numbers either way. It does not reach this box — unlike `services2`, whose `pcie_frac` moved 0.50 → 0.70 in the same hours. Our `--pcie-mode direct` is still our dial, unchanged |
| `strata generate: expert cache 4292 slots, 5.76 GiB of VRAM` / `R4 hit path ON` / `token graph hit path: 4292 resident experts` | same | same | The cache the plateau is pinned at (`--expert-cache 4096` → 4 292 usable) is unchanged |
| `strata verify: window up to 6 tokens, 58.3 MiB of device buffers` | same | same | Still capped by our `--spec 4`, not by the new `--lookup-max-t 12` default (which is CUDA-8-row and was already on at 0.1.56) |
| `strata prefill: HIP device 0 gfx1030 - STRATA_HIP_PROMPT_F16=1: the prompt's 16-bit GEMMs run FP16 in and out` | same | same | Ours since 2026-10-28 (the 2.2× prefill switch); this bump neither added nor removed it |

Not for this card, each gate read where it is decided rather than from a changelog: `STRATA_HC_Q8`
(HIP **gfx12 / gfx1151** — `native_dense.cpp:hc_q8_env()`, and its CUDA branch is `return false`, so off
on *both* boxes despite the commit message saying "on CUDA"), the gfx12 `--prefill auto` 32 768 ceiling
and its staging ring (`STRATA_GFX12_RING`), `STRATA_PF_FUSED_Q50` / `STRATA_PF_DM16` /
`STRATA_EXPERT_V2K` (gfx1151 Windows), `STRATA_GDN_CHUNKED=1` by default (Linux, **all cards gfx12**),
`STRATA_IQ4_W4A8` / `STRATA_W4A8_*` (CUDA, IQ4_XS/IQ4_NL experts only — this pack is IQ2_XS),
`STRATA_MMQ_KQ=1` (NVIDIA cc ≥ 8 and K-quant experts only), `STRATA_MMQ_TAB`/`STRATA_MMQ_DIRECT=1`
(default OFF, costs 36-38 MiB of VRAM per GPU), `STRATA_VNNI_TIER` (opt-in, changes CPU rounding),
`src/dsv41` (`STRATA_BUILD_DSV41` OFF), `src/split` (`STRATA_BUILD_SPLIT` OFF — that is the multi-GPU split
engine, *not* this box's `bin/strata-alias-check`, which the HIP build does install and `alias-check.sh`
drives), the sm_121/GB10 table, and `STRATA_SPEC_ADAPT_SM86`
(still opt-in: the owner's 3060 A/B is "length-confounded, not adopted", `nv_defaults.cpp`).

The one switch that *does* reach this box is serve-layer and output-neutral: **`serve_stop_bound` is now
on for every backend** (`include/strata/core/serve_stop_bound.hpp`, whose own comment is the record:
"#4677: the bound is on by default on every backend; `STRATA_SERVE_STOP_BOUND=0` … turns it off. It only
limits which accepted rows enter the cached conversation: **the tokens a request emits are the same either
way**"). **Methodological consequence for this
directory's favourite instrument:** the conversation cache parks *identities* (see "The conversation
cache parks identities, not prefixes"), and which rows are eligible to be parked can differ across this
boundary — so across this bump compare **cold-to-cold**, not warm-to-warm, and read `reused` in the
`prompt N tokens = X reused + Y read` line before believing any tok/s.

### One new instrument, and it is aimed at this box's oldest open fault

`include/strata/core/gpu_pause_time.hpp` is new and reads **`/sys/class/kfd/kfd/proc/<pid>/stats_<gpu>/evicted_ms`**:
the amdgpu driver's KFD part suspends ("evicts") *every* GPU queue of a process while the kernel moves or
reclaims host-mapped pages, and counts the total. Upstream wrote it for the same fault this file has been
carrying under **"Crash: verify-window timeout (#267)"** and the stall watchdog: "a slow prompt read lost
about exactly that time while background compaction moved pages (… "GPU queue evictions from page
migration"), **and no log showed it**". The counter is read once when a request begins, once when it ends,
and in the stall report — *never on the hot path* — and a missing or unreadable counter means "not
available": no line, no warning. HIP builds on Linux only; everywhere else it is "not available".

**It works here, and the first reading is not small.** aibox runs kernel **7.2.10**, the counter exists
for the engine's own process, and it read **15088 ms** about twenty minutes after the engine started,
immediately after two bench passes. Read it as root, on the **engine**, not on the unit's main pid — `strata.service`'s `ExecMainPID` is the
python server wrapper and has no KFD directory (`pgrep -f 'strata-unstable/bin/strata --serve'` answers
two pids and only one of them opened the GPU, so walk the KFD tree instead, which is self-identifying):

```bash
for p in $(sudo ls /sys/class/kfd/kfd/proc); do           # one line per GPU-using process
  printf "%s %s evicted_ms=" "$p" "$(sudo tr '\0' ' ' < /proc/$p/cmdline | cut -c1-46)"
  sudo cat /sys/class/kfd/kfd/proc/$p/stats_*/evicted_ms
done
```

Run on the box 2026-11-14, it prints exactly one line — the engine — which is also how you tell it apart
from anything else that touches the card. (The 02:54 start's pair was wrapper 3240667 / engine 3240785,
whose counter file was `stats_23001/evicted_ms`.)

That is ~15 s of GPU work the driver held off, on a box whose experts live in host memory the GPU maps —
exactly the configuration the header describes. And it is not one process's artefact: the engine restarted
at 03:22 (to measure the GEMM-warm line below) read **6978 ms** about twelve minutes into its own life,
the same order. Two honest limits on both numbers: they are **cumulative since the process opened the
GPU**, not per request, and there is **no 0.1.56 figure to compare them with**, because the file does not
exist in that tree — the finding is that the instrument now exists and does not read zero, not a
before/after.

What to look for at the next slow request or stall, verbatim from the header: a request line
`strata serve: the graphics driver paused the GPU for N ms during this request while Linux moved memory
the GPU uses; the request was slower by up to that much`, written only when the pause inside that
request reached **100 ms** (`kGpuPauseReportMs`; "a normal request on p620 shows 0-16 ms (the runtime's
own buffer changes); the slow reads lost 245-954 ms"), and inside the stall report
`  GPU work paused by the graphics driver: N ms since the engine started, N ms in this request, N ms in
the last 2 s (the AMD driver pauses all of the engine's GPU work while Linux moves memory the GPU uses)`.
**If a stall prints that second line, the fault is Linux moving memory and not the engine** — which is
the distinction this file has been unable to make from logs until now. Worth knowing before the next
"the prompt took 349 s" investigation: `mm`, `ksmd`, and anything else that touches THP move memory
the GPU is mapping, and this box has 33 GiB of it.

### The control measurement (the plateau band is the control)

Run through the front door (`https://strata2.int.leighhack.org` — the door injects the key, so the
bench holds no secret), with this directory's own `strata-bench.py`:

| arm | 0.1.56, 2026-11-04 | 0.1.59.1 pass 1 | 0.1.59.1 pass 2 | 0.1.59.1 pass 3 (patched script) |
|---|---|---|---|---|
| short-decode, 200 tok | 9.3 tok/s | 9.0 (draft 106/168) | 10.0 (draft 117/166) | **10.9** (draft 125/149) |
| long-prefill, 6,943 tok | 90.3 tok/s | **90.6** (16/24) | *cache hit, not a read* | **90.5** (21/25) |
| long-prefill, 2nd variant | 90.1 tok/s | **90.5** (9/19) | *cache hit, not a read* | **81.4** (16/25) |
| load | 33.02 GiB at 4.30 GiB/s (14 s) | 33.02 GiB at **3.63 GiB/s (20 s)** | | |

**Verdict: the bump is speed-neutral on this box, which is what the gate reading predicts — and this
box's own spread is wider than anything a bump with no switch for it could produce.** Six cold prefill
reads across the two revisions span **81.4-90.6 tok/s** (the low one is a *repetition* of the same length
in the same pass, so it is not a prompt-shape effect), and decode spans **9.0-10.9 tok/s** on the same
script, all of it around the plateau this directory is pinned to (**9.4-9.6 tok/s at 4096 slots**). Still
*not* comparable to the 82-84 tok/s of 2026-10-28, for the reason recorded under that heading. The
engine's own decode hit rates over the passes: **75.7-83.9 %**. The slower load (3.63 vs 4.30 GiB/s, same
33.02 GiB) is one sample taken minutes after a 42 GB build — not a regression claim; re-read it at the
next start.

**The second pass is the trap this box has always had, caught live.** Its two `long-prefill` arms
printed 17.0 and 14.1 "tok/s"; those are not prefills. The engine says so: `prompt 6943 tokens =
**6938 reused** + 5 read in 294 ms (17.0 tok/s)` — the conversation cache parked the prompt on the first
pass and answered the identical repeat from the parked identity. `strata-bench.py`'s `long-prefill` case
had a *fixed* variant marker, so a second run of the same script hit the cache on both long arms. Now
fixed (`machines/aibox/strata-tune/strata-bench.py`, same change the CUDA bench shipped with): the marker
is random per process **and** per case, and **the check is the script's own `prompt_n` column — a
`long-prefill` arm that does not print ≈6,950 measured no read at all.** (Same failure shape as the CUDA
side's 1,100-tok/s-on-a-3060 misread that 0.1.56's record refuses to repeat.)

The other number worth one line, and it is a measurement rather than an inference because the second
start was taken on purpose: `strata prefill: warmed the BF16 and FP16 GEMM libraries on GPU 0 in
**16755 ms** (STRATA_PREFILL_WARM=0 skips)` at the first 0.1.59.1 start, against **5652 ms** at the second
start of the same binary (taken 03:22, a plain `systemctl restart strata` to test exactly this), against
**2979 / 3374 ms** at 0.1.56's two starts. So: **the first start after any engine bump pays this, and the
second does not** — a new binary means a cold Tensile/rocBLAS kernel cache. Budget ~13 s of extra load time
once per bump and do not read it as the bump being slow. `systemctl restart strata` costs ~40 s of
endpoint downtime (33 GiB load + cache fill) and needs no confirm, being no activation.

**And the run config moved exactly where the engine moves, re-proved at this rev rather than inherited
from 11-04:** diffing the 0.1.56 config (`/nix/store/1ndcfbc6iq4hck44p5cgv2wyjybkq3h3-strata-iq2xs.json`)
against the 0.1.59.1 one (`…fc03a1cvqw7dh3p2x541m61cmmagcwqz…`) through `python3 -m json.tool` answers
**two lines**: `exe` and the `--expert-profile` entry in `args` — both store paths of the engine, because
the profile ships *inside* the package. Every flag and all five `env` keys are byte-identical
(`HIP_VISIBLE_DEVICES=0`, `HSA_OVERRIDE_GFX_VERSION=10.3.0`, `HSA_USERPTR_FOR_PAGED_MEM=0`,
`STRATA_HIP_ADAPT_KERNEL_COPY=1`, `STRATA_HIP_PROMPT_F16=1`) — so `--pcie-mode direct`, `--expert-cache
4096`, `--spec 4 --spec-min-p 0.5`, the 1536 MiB / 3-slot / 2048-token conversation cache, 262 144 context
and the F16 prompt switch are **our dials, unchanged by this bump**. On selecting the card while this is
being re-read: there is **no `STRATA_ROCM_AGENT` anywhere in the engine** (grep of the whole 0.1.59.1 tree
answers nothing) — upstream's word is "Select the AMD device with `ROCR_VISIBLE_DEVICES` or
`HIP_VISIBLE_DEVICES` if needed" (`docs/AMD_HIP.md:249`, and `:72` adds that it never overrides them), and
that is exactly what we deploy.

### Deploy record, 2026-11-14 — the umount goes **before** the switch, and that is the whole trick

The 2026-11-04 record below says a plain `just switch` on this box fails with status 4 on
`exports-netboot\x2dsquashfs.mount` and that the failed activation then **armed the rollback countdown
and rebooted the box into the old generation**. This time the dance was planned rather than improvised,
and the result is worth writing down as the recipe:

- Build both engines first, with the inference stopped: `systemctl stop strata` (57 GiB available then,
  ~7-10 GiB with the engine loaded), then one detached unit — `systemd-run --no-block --unit=aibox-strata-build
  --uid=leigh-admin`, script `/tmp/aibox-strata-build.sh`, log `/var/tmp/aibox-strata-build-20261114.log`
  — building `packages.aibox.strata-hip` **and** `packages.aibox.strata-cuda` (services2 still needs it
  buildable here). Exit 0, **02:29:47 → 02:39:54 = 10 min 7 s wall, 26 derivations**, and the two engine
  jobs compile in parallel. Outputs: HIP `/nix/store/97dahyfvskvdgq559bbk3d63cs62402r-strata-unstable`,
  CUDA `/nix/store/h40ij73r2r34wgagk1hs6a6zr55yyac8-strata-unstable` — tell them apart without running
  either: the HIP one's `bin/` installs `strata-alias-check`, the CUDA one's installs
  `libstrata_kq.so` (both packages install into `bin/` and ship no `lib/`; that `.so` is not new to this
  bump, and neither name is a runtime signal — the pack decides which engines get opened).
- **One unit for the switch, because the GOLDEN RULE's same-shell chaining has to survive an umount.**
  `systemd-run --no-block --unit=aibox-sw` running `umount -f -l /exports/netboot-squashfs` →
  `nixos-rebuild switch --flake .#aibox` → **`&& sudo nixos-confirm`** → `mount -a`, each exit code
  echoed to `/var/tmp/aibox-sw-20261114.log`. Receipts in order: `START 02:48:12`, `-- export before:
  /dev/nvme0n1p2[/nix/store/cpf4f8lkkcb27c3mxrv8dsqhgpdh3jlv-squashfs]`, `UMOUNT_EXIT=0`,
  `SWITCH_EXIT=0`, `'/nix/var/nix/profiles/system-good' -> '…9lwa77jc…'`, `CONFIRM_EXIT=0`,
  `MOUNTA_EXIT=0`, `-- export after: /dev/nvme0n1p2[/nix/store/775smggc9b942dlpdlfdr7j96agpa8ph-squashfs]`,
  `END 02:54:30` — **6 min 17 s wall**, 35 min 59 s CPU, 42.5 GB peak RSS, 29.3 GB written, 4.8 GB in.
- **With the lazy umount done first, activation restarts that mount itself and the export follows the
  new generation: no reboot, and the rollback countdown never fired** (the switch exited 0, so the timer
  armed once, at the end, and `nixos-confirm` disarmed it inside the window).
  `findmnt -no SOURCE /exports/netboot-squashfs` afterwards names the **new** squashfs store path.
- What the switch cost: `these 481 derivations will be built` + `these 1 747 paths will be fetched
  (4.3 GiB download, 14.1 GiB unpacked)` — and **`strata-unstable.drv` appears zero times in the switch
  log**, i.e. the engine the build unit produced is exactly the one switched to, as 11-04 said. The 481
  are the closure's own rebuilds that aibox's GC had collected since 11-04 (largest groups `python3.*`,
  the Qt stack, `hdf5-cpp`) — which is the honest version of "a strata bump is cheap on aibox": the
  *engine* is cheap, the toplevel is cheap only if the box has not been GC'd in a fortnight.
- State after: `strata.service` active, **0 failed units**, `/run/current-system` == `system-good` ==
  `/nix/store/9lwa77jc381yig608sn3rrbx1a407dg8-nixos-system-aibox-26.11.20261014.319e4ce`, engine
  `session is up (engine 0.1.59.1)`, `ready: http://10.3.1.32:8080/v1 … context 262144 tokens, API key
  required`, **9235 MiB of VRAM free with everything loaded** (11-04: 9234), cgroup `MemoryCurrent`
  40,448,520,192 B ≈ 37.7 GiB, `free` 7 GiB available.
- **The pi Room clients were not rebooted, again, deliberately.** The export moved to the new squashfs
  inside the switch, so the next client boot takes this image; nothing in `machines/pi-room-sys/`
  changed in this commit. Still open on purpose: the clients are booting the previous image's closure.
- **What gated it**: `just check-eval` (the four assertion evals, `just check-source-size`, alejandra)
  passes at this lock, and this box's `system.build.toplevel` is what the switch built. `just check`'s
  flake step still dies exactly where 11-04 recorded it — `error: path
  'w1qfxxfc0wwjzfqivfnqfqjf60dily0h-sops-secret-pg_pass.drv' is not valid`, under `services.outline.databaseUrl`
  on **services1** — and `nix build` of that drv answers "don't know how to build these paths", so it is
  an eval-time secret read to fix over there, not a bump symptom.

## 2026-11-23: the watchdog ended the engine — what it was, what it was not, and what changed

Three engine deaths in three days, **two different classes**, and the fix for the
dominant one is a memory ceiling rather than an engine bump. This section is the
receipt; the config half is `machines/aibox/configuration.nix` (the cap) and
`machines/aibox/strata.nix` (the reservation, the watchdog limit, the trace).

### What died, and how

`strata.service` never failed. In every case the **engine child** (`strata-host`) went and
the Python serve layer kept serving — `NRestarts=0` on the unit throughout, `/health` 200
within a second, and the next request pays a ~1-2 min reload of a 33 GiB arena because
the serve layer restarts the engine **on demand** (`lazy_load` is the only related knob;
there is no eager-reload-on-child-death in the serve layer, so an abort leaves the next
caller holding the reload).

* **2026-11-21 17:54, 17:55, 17:57 — kernel OOM, class A.** `Out of memory: Killed process
  … (strata) total-vm:53964468kB, anon-rss:33760700kB`, twice more with `strata-host`
  (32.5 GiB, 34.2 GiB), the second time dying inside `kfd_ioctl_alloc_memory_of_gpu` →
  `amdgpu_bo_create`. The `oom-kill:` line names `task_memcg=/system.slice/strata.service`
  and the cpuset of the process that *asked* for memory was `netboot-recut.service`:
  a Pi Room squashfs was being built while the arena was loaded. `systemd-oomd` is enabled
  and did nothing useful (it sat at `oom_score_adj=-900` while the kernel did the killing).
  Unit accounting across its stops is the size of the contest: **46.1 G / 41.3 G / 36.6 G
  memory peaks against 3.7 G / 6.9 G / 7.4 G swap peaks, on an 8.8 GiB swap partition.**
* **2026-11-23 13:01:31 — the engine's own hang watchdog, class B.** `exit -6`,
  `no progress for 60 s during a request (mtp draft: waiting for the GPU -1) - stopping
  the engine so the server starts it again (issue #29)`. In-flight casualty: one
  ~105 329-token read (99 s) that then froze at **298 decode tokens** for ≥70 s and came
  back `done: 298 tokens in 216 s (2.6 tok/s) (error)`. No kernel event at all —
  `journalctl -k` for that window has no ring timeout, no reset, no VM fault, no OOM.

### Reading the stall report, in the three places it misleads

```
stall report (engine 0.1.63): stage "mtp draft: waiting for the GPU -1" for 62 s;
  0 layers served since the last finished step
  verify window: 4 tokens at position 105624, host at layer step 48; the GPU rang 48;
                 flags: served 48, plan (A) 48, copies (B) 48
  expert pool threads: w0..w4=sleeping; host idle for 62272 ms
  VRAM free when this request began: 8344 MiB
  memory: 33187 MiB resident, 2193 MiB in swap, 844 MiB RAM available; 34189 major faults
  GPU work paused by the graphics driver: 354270 ms since the engine started, 9599 ms in this request
strata: released the verify window's GPU waits (#267): the GPU finished in 1078 ms
```

1. **`-1` is not a GPU index.** `progress_at(...)` prints `Progress::detail`, which is
   initialised to `-1` (`include/strata/core/progress.hpp`); the stage is
   `progress_at("mtp draft: waiting for the GPU")` at `src/core/mtp.cpp:1787` and `:1824`,
   and upstream's comment on the second one is *"a stall here read as 'decode', the verify
   window before it"*.
2. **The GPU was not stuck.** Every verify-window flag had completed (served/plan/copies
   all 48, "the GPU rang 48") and the `#267` exit path — which releases the GPU's spin
   waits on host flags so no kernel outlives the process — finished it in 1078 ms. The
   host was spinning on a host-mapped flag (`while (((volatile int32_t*) h_out_)[j] < 0)`,
   `mtp.cpp:1789`) for something the GPU had already done. `progress.hpp` states the class:
   *"the engine's host loop and the GPU wait on each other through flags; a protocol bug
   there does not crash, it spins forever."*
3. **`0 layers served` does not mean "deadlocked at layer 0".** It means the heartbeat
   stopped after the last completed window — which is exactly what a lost flag handshake
   in the *next* stage looks like.

### It is not the R9700's prefill stall — same watchdog, different signature

The off-network R9700 box (gfx1201) reverted 0.2.17/**engine 0.1.63** → 0.2.13.1/engine
0.1.59.1 on 2026-11-20 over 20 `#29` aborts in a day; its evidence pack is
`cjdell/nixos-config`, `docs/strata-prefill-stall-2026-11-20.md`. aibox was on that same
0.1.63 (since `f1c7c1f`, 2026-11-21) and had **one** abort in the whole log since
2026-10-05 — out of 153 prompt reads >50 K tokens and 127 >100 K in the 20 h of 0.1.63
before it, ≈0.8 % where that box was failing ~50 % of big reads. And the fields that
discriminate all disagree:

| | R9700, 2026-11-20 | aibox, 2026-11-23 |
|---|---|---|
| stage | `reading the prompt (batched): layer 22 …` — mid-prefill, layers 18-31 | `mtp draft: waiting for the GPU` — mid-decode |
| GPU | *"the prompt's GPU work was still running"*, sampled `gpu_busy 100 / mem_busy 0 / 60-80 W` | finished; released and done in 1078 ms |
| VRAM free | **760 MiB** in every stall (`--expert-cache auto`) | **8344 MiB** (`--expert-cache 4096`) |
| host RAM | 31 GiB available, 0 swap → **ruled out by control run** | **844 MiB available, 2.2 GiB swap, 34 189 major faults** |
| driver MM pauses | 549 ms lifetime, 19 ms in request | **354 270 ms** lifetime, **9599 ms** in request |
| drafter | **DFlash2 chosen by hand** (upstream's named gfx1201 trigger) | **MTP** (`Drafting: MTP (the DFlash2 drafter isn't downloaded yet)`) |

So: aibox already sits on two of that box's three "next steps" (MTP; expert cache capped
with GiB free), its stall is in the drafter handshake rather than the prompt path, and the
one field where aibox *inverts* the other box's ruled-out list is host RAM. aibox's own
older write-up above — the 2026-10-06 #267 entry, and upstream **#649**: *"a pinned
resident budget on a low-RAM box starves the CPU expert pool so it misses the verify
handshake"* — fits this event better, and it is a mechanism with a lever here.

### The mechanism, measured rather than inferred

Nothing about the card or the runtime moved (ROCm 10 runtime since 2026-10-28,
`HSA_USERPTR_FOR_PAGED_MEM=0` and `STRATA_HIP_ADAPT_KERNEL_COPY=1` both still set, engine
0.1.63 for ~20 h). What was different at 13:01 was **memory**. `/proc/vmstat`, since boot
(23 days):

```
pgscan_direct   53,942,974     (~206 GiB scanned inside the allocating path: the caller
                                 stops and moves pages itself)
pgsteal_direct  10,420,292
pswpin 46,615,215  pswpout 61,096,721   (~165 GiB in / ~217 GiB out, of 8.8 GiB of swap)
allocstall_movable 165,554   allocstall_normal 21,971
pgmajfault 8,992,689          compact_fail 60,088
```

On this card every "VRAM" allocation is GTT — pages of the same DDR5 as the arena — so
reclaim *is* GPU work paused, and the engine counts it in its own log: requests here have
been paused for 24 519 / 24 636 / 51 678 / 63 973 / **133 139 ms** in a single request,
354 270 ms in one engine run. Against that, the watchdog's limit was **60 s** (the default;
`STRATA_WATCHDOG_S` was never set here), so the engine was being killed for the box's
normal behaviour. **That is the single most likely difference between aibox and the R9700's
stalls: their box had 31 GiB free and 19 ms of pause in the failing request; this one had
0.8 GiB free and seconds-to-minutes of it.**

Nobody had bounded either side. Live before this change: `strata.service` had
`MemoryMin=0`, `MemoryHigh/Max=infinity`, `OOMScoreAdjust=0`, and `nix-daemon` was
uncapped with `max-jobs = auto` = **12 jobs on 6 physical cores** — declared nowhere in
the tree, only defaulted.

### Deployed 2026-11-23

| knob | value | why that number |
|---|---|---|
| `strata.service` `MemoryMin` | **40G** | cgroup protection: reclaim takes everyone else's pages first. Sized to arena+caches (unit peak 39.3 GiB via systemd) with ~19 GiB left for the rest of a 59 GiB box. Protection, **not** a cap — a `MemoryMax` here would OOM the engine itself |
| `strata.service` `OOMScoreAdjust` | **-700** | if the box does run out, a compiler dies and the endpoint lives; 2026-11-21 had it the other way round, twice |
| `env.STRATA_WATCHDOG_S` | **180** | legitimate measured pauses (51.7 s, 64.0 s, 133.1 s) exceed the 60 s default; the R9700 has run 180 since 2026-11-20. Cost: a genuinely stuck engine holds the single slot 3 min instead of 1 |
| `env.STRATA_VERIFY_TRACE` | **1** | armed because this class recurred and the section above says to capture the trace block; it is the only artifact that separates #649 from #29. Costs nothing unless a window times out. `STRATA_KERNEL_AUDIT` stays unset (it adds ~10 graph nodes/layer) |
| `nix.settings.max-jobs` (aibox) | **4** | was the `auto` = 12 default, declared nowhere; ~2-3 GiB link peak per job |
| `nix-daemon` `MemoryHigh/Max` | **12G / 18G** | builds are the daemon's children, so that is the cgroup a runaway `rustc` lands in; the number is RAM − the reservation − the apps. **A build that needs more now fails as a build** — which is the old `systemctl stop strata` advice with the failure moved to the cheap side |
| builder duty | **withdrawn network-wide** | `common/nix-gc.nix`'s header; aibox's 59 GiB is the engine's, and every other box builds for itself (gw bounded too, `machines/gw/default.nix`) |

Not changed, deliberately: `--expert-cache 4096` (the table at the top of this file is why;
the user asked for the RAM to go to the engine, not to be shrunk), the model and pack,
`--max-context 262144` (agent traffic arrives at 100 K+), the runtime, and the engine pin —
**do not read this section as a case for reverting to 0.1.59.1.** aibox had zero hangs on
0.1.59.1 too, but it came to 0.1.61 for `5be334a` (256 K at full precision via KV in RAM),
`nix flake update strata` moves all three engines in one switch, and upstream has since
shipped 0.2.18/0.2.19/0.2.20 (engines 0.1.64/65/66) — all *after* the R9700's stall write-up
and all shipping `hip-rdna2 (gfx1030–gfx1032)` packages. If the pin is moved, move it
forward through the bump ritual, not back.

### Still open, and how to tell next time

* **n=1.** One class-B event is not a rate. If it recurs with `MemoryMin` and 180 s in
  place, the `STRATA_VERIFY_TRACE` block names the answer; if it recurs **without** memory
  pressure (`free -g` roomy, `pgmajfault` flat, `paused the GPU for` small), then this box
  does share the R9700's open upstream issue and belongs in that report.
* **Was the 13:01 request reclaim or a lost wait?** Today's report says `9599 ms in this
  request` but `0 ms in the last 2 s` — so the long pause had ended before the 62 s stall.
  Reclaim made the box sick; whether it is what held the flag is unproven. That is exactly
  what the trace block is for.
* **Swap is 8.8 GiB on a box that peaks at 7.4 GiB of swap.** If the engine's overflow is
  routinely hitting swap, `MemoryMin` protects it from reclaim but not from a full swap
  partition; the honest levers are the arena (`expertCache`, the Coder pack's 23.4 GiB) or
  more swap/RAM, and the model swap is an answers-change (see "Suggestions" #2).
* **A dead engine costs the next caller a reload** and nothing reloads it eagerly. If that
  starts to matter more than the deaths, the fix is a warm-up poke, not a watchdog tweak —
  and it needs the api key, so it is a secret-handling decision first.
* **Re-verify after any switch here:**
  `systemctl show strata -p MemoryMin -p OOMScoreAdjust --value`,
  `systemctl show nix-daemon -p MemoryHigh -p MemoryMax --value`,
  `nix config show | grep max-jobs`, and
  `sudo cat /var/lib/strata/strata.log | grep -cE "paused the GPU for|no progress for"` —
  the second number is the one that says whether this section was the end of the story or
  an early chapter.

## Re-measuring

`strata-tune/` in this directory: `sudo ./strata-exp.sh <expert-cache> <prefill>
[extra-env-json] [extra-args-json] [bench-script]` (derives the engine binary and
run config from the installed unit, so it follows a rebuild) and
`sudo ./run-arms.sh` for the full arm set. Benches: `strata-bench.py` (decode +
two cold ~8k-token prefills) and `strata-bench-short.py` (one short decode, for
reading the stage profile). The arm sets measured on 2026-10-08 are
`./run-arms-uma.sh` (`--pcie-mode`, `--pcie-frac`, pool, prefill borrow),
`./run-arms-prefill.sh` (the same arms against the full bench, for prefill) and
`./run-arms-knobs.sh` (cache admission, draft depth, pool batching, overlap
pokes); `./alias-check.sh` answers the device-alias question without loading a
model. Pass the bench script to `strata-exp.sh` as an ABSOLUTE path —
`strata-prep` chdirs into its own tools directory, so a relative one fails.
Note the profile lines (`strata decode timing`, `decode GPU stages`) go to the
*engine's* log — the run config's `log`, which `mkexp.py` points at
`/tmp/exp-strata.log`, appended across arms — not to the server log the arm
script tails.

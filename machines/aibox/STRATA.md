# Strata on aibox — measured behaviour and what to try next

Qwen3.8-Flash-Next (125 B total / ~6 B active MoE) running as `strata.service`
on the Radeon 660M iGPU + system RAM + NVMe. Config: `strata.nix` (service) +
`strata-package.nix` (engine build) + `rocm10-runtime.nix` (the optional ROCm 10
userspace runtime). Endpoints: `http://10.3.1.32:8080/v1` directly and
`https://strata2.int.leighhack.org/v1` through the front door on `gw` (one row in
`common/front-door.nix`; this box has no nginx of its own).

> **Parked 2026-10-17, un-parked 2026-10-28.** It was disabled so this box could be
> the network's build machine (`common/build-farm.nix`): the IQ2_XS expert arena is
> ~33 GiB of RAM a build needs, and `llama-server` got its `wantedBy` back. It runs
> again — on the flake's `strata` input instead of its own nine-releases-stale pin,
> and behind a new name. **The RAM trade is unchanged and still real**: with the
> engine loaded this box has ~10 GiB left for builds, so `systemctl stop strata`
> before a nixpkgs bump or anything that links a browser, and `expertCache` in
> `strata.nix` is the dial if the two must coexist for long. What made this an
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
   what killed the GTX 1060 attempt (2.5 GT/s ×1 ≈ 250 MB/s, 256 MB BAR; see
   `gtx1060-followup.md`). Don't repeat that without a real USB4/TB4 dock.

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
line, plus `STRATA_VERIFY_TRACE=1` if needed. Memory is the other lever
(#649): at 262144 context the box sits at ~51 GiB used / ~8 GiB available with
2.6 GiB of swap in use, and the expert cache is the dial to turn down
(`expertCache = 2048` frees ~2.9 GiB for ~10% decode).

Pinned llama.cpp is unchanged (`3cf03257`), so the ggml side is identical.
Built for gfx1030 and device-checked clean (`strata-device --list-devices`
with `HSA_OVERRIDE_GFX_VERSION=10.3.0` reports `arch gfx1030`).

## UMA: `--pcie-mode direct` deployed, and what the copy actually costs (2026-10-08)

`./strata-uma-2026-10-08.md` reads the engine's three delivery modes for the
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
  `nix gc` (`--delete-older-than 30d`, `common/build-farm.nix`) will take the 16 GB
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
`http://10.3.1.32:8080/v1` directly now need that key; `pi-models.json` is pointed at
`https://strata2.int.leighhack.org/v1` instead, where the door injects it and no client
holds a secret.

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
  GiB of 59 used, ~6 GiB available — i.e. the build-farm headroom the header warns about,
  measured.
- **Decode is not slower on the new engine or the new runtime**: 220 completion tokens in
  18.3 s through `/v1/chat/completions` ≈ **12 tok/s**, at or above the 9.4-9.6 tok/s
  plateau, with `finish_reason: length` and correct output. One `#267`-class event in
  1.5 days of the old engine remains the fault record; nothing to add yet on this one.
- The key is enforced: `/health` says `"api_key": true`, `/v1/models` is a 401 without it
  and the model list with it, and the serve banner now ends `API key required`.
- `strata.service` has `NRestarts=0` since the switch.

Two operational notes from the switch itself:

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

## Pi clients

`~/.pi/agent/models.json` on **aibox** and **services1** (10.3.1.20) was written
from `pi-models.json` in this directory (the canonical copy, kept here because
`~/.pi` is per-user state and not in the flake):

- `strata` → `https://strata2.int.leighhack.org/v1`, model
  `qwen3.8-flash-next-iq2xs`, context 131 072, maxTokens 16 384. It is the front-door
  URL on purpose: `strataBearer = true` on that row injects the engine's `api_key`,
  so no client holds an inference secret and the `"apiKey": "strata"` placeholder in
  `pi-models.json` never reaches anything that reads it. Pointing it back at
  `http://10.3.1.32:8080/v1` is a 401 for anyone without `strata_api_key`.
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

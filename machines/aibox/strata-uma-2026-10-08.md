# Strata on a UMA box — where the "PCIe" copies actually go, and what a fork would change

Deep dive on 2026-10-08 against upstream `main` (`fb58e0d`, read via a shallow clone in
`/tmp/strata-up`). aibox's deployed engine is `0.1.40.2` at `e8ca9afd`; the line numbers
below are from current `main` and are stable in the 0.1.40.x tree.

Companion doc: `./STRATA.md` (the measured behaviour on this box). This one is about the
memory model and what could be changed.

## TL;DR

1. **The copy is not a link, and on aibox it is a real copy.** `--pcie-mode` has three
   values (`include/strata/core/expert_source.hpp:281-285`): `0` = the copy engine stages
   the blob, `1` = **the grouped kernel reads the pinned arena through its device alias
   (true zero-copy)**, `2` = a copy kernel stages it inside the graph. `auto` resolves to
   **2** (`src/program/generate.cpp:6868`), so aibox is doing a DRAM→DRAM copy for its
   PCIe share today. `--pcie-mode direct` is the flag that already exists and is not set
   in `strata.nix`.
2. **The bandwidth saving from removing it is small; the latency saving is the real one.**
   ~16 % of routed experts × 1,382,400 B ≈ 106 MB read + 106 MB written + 106 MB re-read
   per token, against ~2.7 GB/token of unavoidable weight reads → ~8 % of DRAM traffic
   (~4 ms of the 121 ms/token window). The profile's `PCIe grp` line (14.9 + 3.7 ms/window)
   is copy *work*, i.e. ~8 % of the window, plus the `waitB` handshake it creates. Expect
   ~10 % decode, not a step change.
   *(Measured the same day: ~1 %, not ~10 % — `PCIe grp` is the grouped kernel, not
   the copy, and the copy is `waitB` at 6.95 ms/window. See "Measured on aibox the
   same day" below.)*
3. **The structural waste is the duplicate residency, not the copies.** On aibox the engine
   holds the same experts twice: the pinned host arena (~33 GiB, read by the CPU pool) *and*
   a `cudaMalloc`'d cache (GTT pages = the same DDR5, different pages, 4096 slots = 5.7 GiB).
   On UMA a cache is only worth having if reading it is faster than reading the arena — it
   isn't, it is the same DRAM. So the cache is pure duplication, and the adaptive tier
   (`--adapt-swaps 96` every `--adapt-every 4` rounds ≈ 24 blobs/window ≈ 15 MB/token) is
   ongoing copy traffic for no bandwidth benefit.
4. **The right UMA design is one copy + a cost-model split.** The engine already has every
   piece except the policy: `device_alias()` per (layer, expert), `pcie_mode == 1`, and the
   `ar_on()` "zero-doorbell graph" (`src/core/verify.cpp:632-680`, `include/strata/core/verify.hpp:346`).
   What is missing is (a) an *alias cache* — residency table + slot offsets pointing at the
   arena's device alias instead of a device allocation — and (b) a split decided by measured
   CPU-vs-GPU cost instead of by a link probe that measures a memcpy on a box with no link.
5. **No fork does this.** The only UMA-aware code in upstream is memory *reporting*
   (`device_free_bytes()`, `src/core/expert_cache.cpp:25-41,491`) and the APU OOM warning
   (`src/program/generate.cpp:4353-4366`). Upstream **issue #1236** ("Will zero-copy be
   implemented on Strix Halo?", open, 2026-10-06) asks for exactly this and is unanswered;
   **PR #1548** (`STRATA_PCIE_BALANCE=1`, open, unmerged) is the cost-model half of it.
   **Issue #514** was closed as *not planned* — but its reasoning ("an iGPU reads the same
   system RAM as the CPU, so the gain would be small") is about using an iGPU as a *second*
   device beside CUDA, not about the iGPU being the primary device, which is aibox's case.

## Measured on aibox the same day (the arms ran after this doc was written)

`strata-tune/run-arms-uma.sh` (decode) and `strata-tune/run-arms-prefill.sh`
(prefill) ran every arm below; the numbers are in `STRATA.md`, section "UMA:
`--pcie-mode direct` deployed". What they settled:

- **`--pcie-mode direct` is deployed**, for the reliability reason rather than the
  speed reason: 229.7 → 227.0 ms/window (9.3 → 9.4 tok/s), because it removes the
  in-graph copy kernel and its 16 staging blobs — the copy machinery #884 names.
- **Item 2 is wrong about where the copy is.** `PCIe grp` is *not* copy work: in
  `VerifyWindow::pre` (`src/core/verify.cpp:1414-1424`) the stamps are
  `grouped(p_ptr)` → 20, then `wait_flag_ge(m_flagB_)` **plus, only in mode 2,
  `fetch_blobs` + `rebase_ptrs`** → 21, then `grouped(p_ptr2)` → 22. The copy is
  inside `waitB` and costs 6.95 ms/window, not 14.9 + 3.7; the 14.9/3.7 is the
  grouped kernel *computing* the PCIe share, which runs in every mode. Predicted
  ~8 % of the window, measured 3 %, net win 1 %.
- **Item 4's premise ("a cache is only worth having if reading it is faster than
  reading the arena — it isn't") is half right.** Reading through the alias is
  ~15 % *slower* per byte than reading a staged GTT copy (identical routing:
  `PCIe grp` 10.86 ms staged vs 12.49 ms through the alias). An alias cache would
  still likely be net positive — the copy it deletes costs more than the slower
  read costs — but it is a RAM-for-speed trade, not the free 5.7 GiB implied here.
- **The alias is not refused by `HSA_USERPTR_FOR_PAGED_MEM=0`.** The engine's
  `+13…21 % of the routed experts over PCIe` line only increments when
  `pcie_layer()` is true, so `device_alias()` returns addresses in the live
  production run with the mitigation on. `strata-tune/alias-check.sh` (upstream's
  `tests/hip/mapped_alias.cpp`, now installed as `strata-alias-check`) is the
  direct check and runs it both ways — and it says the alias is **the host pointer
  itself** on this box (`-> the host pointer itself (unified addressing)`, "a kernel
  reads the alias correctly: holds", identically with the mitigation on and off). So
  `device_alias()` is not a device address here at all: the arena is one mapping and
  the GPU reads it in place.
- **The probe's 0.55 survives the sweep** (0 → 7.5, 0.25 → 8.1, 0.55 → 9.4,
  1.0 → 9.3 tok/s), so PR #1548's cost fit is not what this box needs: the split
  it would compute is the one the probe already picked.
- **The ceilings did not move, and the reason is now measured.** A second arm set
  (`strata-tune/run-arms-knobs.sh`: cache admission, draft depth, pool batching,
  the overlap pokes, plus 5500/6500-slot caches) lands every configuration in
  225-231 ms/window. `GPU-reach wait + per-layer host` is invariant to where
  experts are computed (frac 0: 107+135; deployed: 118+68; 5500 slots: 129+54;
  frac 1.0: 177+11) — the verify window is a chain, so the trade is 1:1. Both
  halves are compute-bound: the CPU pool moves ~21 GB/s of expert bytes against
  upstream's ~40 GB/s reference ("this CPU has no AVX-512"), and the 6 CUs are
  unchanged. See STRATA.md, "The plateau".
- **Prefill is unmoved by all of it** (50.4 → 50.7 tok/s on a 6 943-token prompt),
  and `--no-prefill-borrow` *costs* 4.8 GiB of GTT here (16.23 → 21.03 GB): the
  loan is cache slots, the reserve is prompt buffers. `STRATA_PREFILL_CPU_SHARE`
  is the one prefill-side win, and only for short prompts (70-token prompt:
  16.0 → 20.7 prompt/s); long prompts unchanged, and it changes bits.

## What aibox runs today, tier by tier

| Tier | What it is on aibox | Cost |
|---|---|---|
| host arena | 33 GiB, `cudaHostAlloc(Mapped\|Portable)` (`src/core/expert_source.cpp:2408`), `LimitMEMLOCK=infinity` | the only copy that must exist |
| expert cache | `cudaMalloc`'d → GTT pages, 4096 slots = 5.7 GiB | **duplicate** bytes + fill copies |
| PCIe share | `pcie_mode` 2 → copy kernel into staging (16 blobs, `include/strata/core/verify.hpp:459`) | copy + handshake, no link |
| CPU pool | 5 workers (6 physical cores − host thread; issue #40 fixed Linux SMT counting) | the biggest line in the profile |
| PLE | 28.8 GB read unbuffered from NVMe | I/O, not DRAM |

The startup line `PCIe probe: 31.4 GB/s host->device -> pcie_frac 0.55` is `probe_pcie_h2d_gbps`
(`src/program/generate.cpp:1493`) timing a 256 MiB `cudaMemcpyAsync` between two allocations
that are the *same DDR5*. `pcie_frac_for_gbps` (`:1556`) then scales the x16 reference share
by that reading — on aibox the number is meaningless, and 0.55 is inherited from a
Ryzen 7600 + RTX 5070.

## The three delivery modes, in code

`src/core/expert_source.cpp:3166-3239` is the plan. For each distinct missed expert:

```cpp
P.ptr2[q] = P.pcie_mode != 0 ? (unsigned long long) d.src->device_alias(d.layers, ids[i0])
                             : P.staging + q * bb;                 // :3220
if (P.fetch) P.fetch(P.ctx, dma_src, P.pcie_mode != 0 ? 0 : fetches, bb);   // :3239
```

and in `src/core/verify.cpp:1457-1466`:

```cpp
if (sink_.pcie_mode == 2) {          // stage it with a copy kernel, then point at staging
    fetch_blobs(p_ptr2, p_counts + 2, stage, blob, per, cs);
    rebase_ptrs(p_ptr2, p_counts + 2, stage, blob, cs);
}
grouped(p_ptr2, p_start2, p_counts + 2, kPcieGroupRows, hit_out);
```

So mode 1 never copies: `ptr2` stays the alias and the grouped kernel reads through it.
`tests/hip/mapped_alias.cpp` is the check that the alias is usable on a given stack — on
Linux it is a device address of the pinned host memory (on an iGPU it is usually the host
pointer itself, i.e. unified addressing). **Run that test on aibox with and without
`HSA_USERPTR_FOR_PAGED_MEM=0`** — the reliability workaround currently in `strata.nix` keeps
ROCr off userptr for paged host allocations, and the arena alias is exactly such a mapping.
If the alias is refused there, `pinned()`/`device_alias()` return null, `pcie_layer()` is
false, and `--pcie-frac` becomes inert. That is a testable interaction, not a hypothetical.

Two caps to know about for a "GPU reads everything through the alias" mode:
`fetches < P.staging_cap && fetches < 64` (`:3191`) — at most 16 alias groups per window
(8 per group with `G == 2`), so `--pcie-frac 1.0` cannot send *all* misses.

## The prefill path is the same story

`--no-prefill-borrow` exists (`src/program/generate.cpp:515-517,1780`). Upstream #1236's
comment measures it on a Strix Halo: a 16K prompt lends 2,840 cache slots, **streams 6.9 GB
through host staging (7.9 s of staging time)** and refills afterwards (676 ms) — "on a
unified-memory APU none of that is needed". `--no-prefill-borrow` gave +5.2 % prompt.

On aibox prefill is compute-bound on 6 CUs (~45 tok/s), so removing the copies will not move
prefill much. The lever there is the other zero-copy path: `STRATA_PREFILL_CPU_SHARE`
(`docs/DETAILS.md`, "Short prompts: the CPU shares the experts") — the CPU takes experts it
**reads from RAM as they are** (the arena, the page-locked copy, or the mapped
`experts.bin`), no staging. It is on by default on CUDA builds since 0.1.41 but **off on HIP
builds unless you set the variable**, and `STRATA_PREFILL_CPU_SHARE_MAX=3072` is opt-in. On a
box where the GPU is the compute bottleneck and the CPU is idle during a prompt, that is the
natural UMA split. (It changes bits: first-token KL mean 0.006, max 0.026.)

Note also that gfx1030 gets no WMMA/prompt-expert kernels (the list is gfx1100/1101/1102/
1150/1151), so aibox's prompt experts are the MMQ path — which the Strix doc names as the
slow part when nothing streams.

## The patch a fork would make (the actual gap)

Two small changes, both in files aibox already has:

**1. Alias cache when the device is integrated.** In `ExpertCache`, when
`cudaDevAttrIntegrated` is set and the source is a pinned arena, do not allocate: set
`cache_base` to the arena's device alias, `slot_off[e] = blob_offset(layer, e)`, and the
residency table to "every expert resident". No fills, no swaps, no staging, no
`STRATA_HIP_ADAPT_KERNEL_COPY` — the #884 trigger (MMQ prompt + adaptive swaps + copies)
disappears with the copies. Memory: 33 GiB instead of 33 + 5.7 GiB, which is exactly the
headroom the Coder pack (`ISTA-DASLab/.../IQ1_M`, arena 23.4 GiB instead of 35.5) needs.

*Measured input for this patch (2026-10-08, see STRATA.md):* the alias is the host
pointer (unified addressing), and a grouped kernel reading the arena through it is
~15 % slower per byte than reading a staged GTT copy (`PCIe grp` 10.86 → 12.49 ms at
identical routing). The patch therefore trades ~5.7 GiB of RAM for a couple of
percent of the decode window, and it has to keep the cache's other property — being
what makes 75 % of the routing a GPU hit rather than a CPU miss (`--pcie-frac 0`
measures the miss-heavy end at 7.5 tok/s). Worth building; not the free win this doc
first assumed.

**2. Split by measured cost, not by a probe.** PR #1548's `PcieModel` is the right shape:
fit `pool = a + c·n_cpu + d·n_pcie` and `GPU = g0 + g·n_pcie`, then pick the `m` that
minimises `max(GPU, CPU)`. On UMA `d` is not "the link slowing the pool" but "the GPU's read
contending with the pool for the same controller" — with mode 1 the `d` term collapses to
read contention, which is exactly why the fit is better than a fixed 0.55 on a 6-core AVX2
box. From the profile, per entry the GPU is ~0.39 ms and the CPU ~0.75 ms, so the optimum is
"give the CPU as much as it can absorb without becoming the critical path" — the fit finds
that, the probe cannot.

Optional third piece: allow the alias on a `STRATA_ARENA_MMAP=1` arena. Today the docs say
"the GPUs then get no mapped alias: run with `--pcie-frac 0`" (`src/core/expert_source.cpp:3888-3892`)
because the mapping is not registered. `hsa_amd_memory_lock`/userptr can register it, and on
UMA that gives the one-copy-two-readers design with the page cache handling pressure.

## Arms worth running (with the existing `strata-tune/` harness)

See `strata-tune/run-arms-uma.sh` (decode, short bench) and
`strata-tune/run-arms-prefill.sh` (the same arms against the full bench, for the prefill
numbers). Order matters: the control first, then mode, then the share, then the pool, then
prefill. Each arm reloads the model (~35 s). All of these ran on 2026-10-08; the results are
in `STRATA.md`.

```
control            4096 8192
mode direct        4096 8192  args: --pcie-mode direct
direct + 0.25      4096 8192  --pcie-mode direct --pcie-frac 0.25
direct + 1.0       4096 8192  --pcie-mode direct --pcie-frac 1.0
direct + 0         4096 8192  --pcie-mode direct --pcie-frac 0     (no GPU share at all)
direct + pool 12   4096 8192  --pcie-mode direct --pool-workers 12
no-borrow          4096 8192  --pcie-mode direct --no-prefill-borrow
cpu share          4096 8192  env STRATA_PREFILL_CPU_SHARE=auto, STRATA_PREFILL_CPU_SHARE_MAX=3072
```

Read `mem_info_gtt_used` on each arm: with an alias cache it should fall to the dense+KV
figure (~8 GiB) rather than 15 GiB, and the `PCIe grp` line in the profile should go to zero
on the `direct` arms. If `PCIe grp` does not go to zero, the alias is being refused — check
`HSA_USERPTR_FOR_PAGED_MEM` first.

## Caveats

- **Do not use `--expert-cache auto`** on aibox: `device_free_bytes()` on an integrated
  device returns `MemAvailable - STRATA_UMA_HEADROOM_GIB` (6 GiB), i.e. the whole machine.
  The engine warns about this for APUs (`generate.cpp:4353-4366`) but does not cap it.
- **Reliability vs zero-copy.** `HSA_USERPTR_FOR_PAGED_MEM=0` is the #750/#920 mitigation and
  may be what disables the alias. If the alias works with it on, keep it; if the alias only
  works with it off, that is a reliability-for-speed choice that has to be made deliberately,
  with `STRATA_VERIFY_TRACE=1` watching.
- **Bit-exactness.** `--pcie-frac 0` is the only way to keep byte-identical repeats
  (`docs/DETAILS.md`); `STRATA_PREFILL_CPU_SHARE` changes rounding. Fine for A/B, not for a
  parity harness.
- **The ceilings do not move.** Decode is DRAM-bound (~18 tok/s theoretical, 9-12 measured)
  and prefill is 6-CU compute-bound (~45 tok/s). A UMA patch removes waste, it does not add
  bandwidth or CUs. The measurable wins are the ~10 % copy latency, the 5.7 GiB of RAM, and
  the removal of the copy path that the gfx1030 timeouts are traced to.

## Fork survey (2026-10-08)

100+ forks listed; all are mirrors except:

- `calvinvette/strata-inference-orin` — Jetson Orin AGX (UMA), but `src/` is byte-identical
  to upstream: a rename, not a zero-copy port.
- `Kaltharos/Strata-Custom`, `architectds/Strata`, `Nowng/Strata-Heterogeneous` — differ from
  upstream only by version drift / multi-GPU work, nothing UMA.
- Nothing in `docs/`, `bench/results/` or the commit messages implements a UMA read path.

Open upstream items that matter to this box: **#1236** (zero-copy on unified memory, open,
unanswered), **#1548** (`STRATA_PCIE_BALANCE`, open, unmerged — needs its own build),
**#884** / **#1103** (the gfx1030 verify-timeout class aibox is an exact match for).

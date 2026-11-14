# How fast is this, really: the 2026-11-14 decode and prefill sweep

Every number Swift 1.5 and the 0.1.59.1 bump were quoted against, and every lever
`STRATA.md` names, was put through one A/B harness on the live box over ~2 hours.  The
harness is `machines/services2/strata-tune/` (it was written for the aibox HIP box and
`strata-exp.sh` was its CUDA twin); this file is what came out for services2.

**The answer, up front.** On the dials that do not move answers, **the deployed
configuration is already at the optimum this engine offers on sm_86**: 23 arms measured,
none of them beat control by more than the harness's noise floor, and several lost badly
(`--pcie-frac 0.85` −13 %, `--spec 6` −10 %, `--aux-cpus 8-15` −5 %).  The reason is on
the meters and it is worth internalising before the next sweep: **this card is not
power-capped, not clock-throttled, not PCIe-bound and not CPU-scheduling-bound** —
prefill and decode both run at ~1,900 MHz of a 1,950 MHz maximum, drawing 157 W and 98 W
of a 170 W limit that cannot be changed (min = max = 170 W on this VBIOS), and a card
pushing its **full bf16 peak of 26 TFLOP/s** under cuBLAS draws only **95 W**.  The one
thing that moved a meter was `STRATA_PF_FUSED=1` — **+5 to +11 % on 8k prompts, +1 to +3 %
at 28k, decode unmoved** — and since it is listed upstream among "options that change
answers", it sat undeployed until the question was answered with an answer key rather than
a hash: **8 items scored, 8 correct before it and 8/8 after** (§"The one win").  **It is
deployed now.**

---

## The box under load, measured rather than inferred

Sampled at 2 s through labelled phases driven through the **live endpoint**
(`sudo python3 machines/services2/strata-tune/strata-phases.py`, which reads the key out
of the rendered config and never prints it): three 63,052-token prompts (56 s each, one
chunk of real prefill) and one 332-token/240-token generation, with idles around them.

| phase | W mean | W max | SM clock | util | what it was |
|---|---|---|---|---|---|
| idle, engine resident, no traffic | 13.7 | 14.0 | **210 MHz** | 0 % | P-state 8 |
| **prefill** ×3 (1,130-1,138 tok/s) | **157.0** | 165.6 | **1,900 avg / 1,950 max** | 100 % | 168 s of it |
| **decode** (33.4 tok/s, 240 tokens) | 97.8 | 115.7 | **1,927** | 100 % | 8 s |
| cuBLAS bf16 GEMM storm, n=4096 | 95.2 max | — | — | 100 % | **26 TFLOP/s** |

Corroborating dials, all read on 2026-11-14:

- `nvidia-smi -q -d POWER`: **Current = Requested = Default = 170 W, and Min = Max = 170
  W**.  There is no power knob to turn on this card; 170 W is also what the board is
  rated at, so the "raise the limit to 160 W" idea that a first reading suggests was
  never available (and 160 < 170 anyway).
- `nvidia-smi -q -d PERFORMANCE` counters: **SW Power Capping went up by 1.25 s across
  ~176 s of load** (26,655,454 µs → 27,902,254 µs).  `HW Slowdown`, `HW Power Brake`,
  `SW Thermal` all 0.  The driver clamps almost never, and the clock readings say the
  same thing: 1,900-1,950 MHz during prefill is maximum boost, not a sagging clock.
- The engine's own start-up meters, from the arms' logs: expert cache **1770 slots / 3.41
  GiB of VRAM**; `PCIe share: the CPU pool takes 297-307 µs per missed expert (6.7-6.9
  GB/s of expert bytes), the link 13.4 GB/s -> pcie_frac 0.70`; decode expert-cache hit
  rate 49.6-78.9 % depending on how long the prompt is (a 200-token generation leans on
  the cache far more than a 96-token one).
- Memory: `/proc/meminfo` `Shmem: 25,041,688 kB` — 23.9 GiB of the pack is resident as
  **shared memory**, because the unit passes `--resident-experts`; 30 GiB of RAM; a 16
  GiB swap **file** at `/var/swap/services2`.  **The box is memory-tight for this
  workload**: with the engine idle, swap holds ~450 MiB and 4.5 GiB is "available"; after
  three 63k-token prompts it held **1.70 GiB** (engine `VmSwap` 1,077,456 kB, `SwapCached`
  156 MiB) and available was 3.4 GiB — at `vm.swappiness=10`, i.e. deliberately biased
  against evicting exactly that.  Nobody has measured whether that swap traffic costs
  tok/s (see the open items); what is measured is that it does not change the pool's
  297-307 µs per missed expert, because those readings are identical before and after.

Two conclusions that reorganise the rest of this file:

1. **The 157 W is not a ceiling, it is a bill.** A card genuinely at its compute limit
   spends 95 W here.  Prefill spends ~60 W more than that and produces 1,130 tok/s, which
   means the watts go into dequantising IQ1_M and moving bytes — into the *non*-tensor
   part of the chip — while the tensor cores sit mostly idle.  "Prompt speed is bought
   with watts and the meter is at the ceiling" was the wrong model, and it is the model
   that made `--pcie-frac` look promising.
2. **The engine's reported "1,060 TFLOP/s" of prefill is not a hardware rate.** This card
   does 26 TFLOP/s of bf16 under cuBLAS, measured two ways (CUDA events and wall clock,
   three runs, 26/26/26).  1,060 is model-FLOP accounting across a sparse MoE, roughly
   40× what any RTX 3060 can do; it is useful as a before/after ratio and useless as a
   ceiling.

## The harness, and the noise floor you have to clear

```
sudo machines/services2/strata-tune/strata-exp.sh <expert-cache> <prefill> [env-json] [args-json] [bench] [more-benches...]
python3 machines/services2/strata-tune/strata-tune-report.py     # /tmp/arm-*.log -> the table above
```

Each arm regenerates `/tmp/exp.json` from the **installed** unit (so it follows a
rebuild), stops `strata.service`, runs the same binary and pack on `127.0.0.1:8123` with
`api_key: none`, benchmarks it, and hands 8080 back.  **Cost: 60-100 s of the model being
down per arm**, plus ~40 s of bench for the fast script and ~3.5 min for the full one.

**Noise floor (measured, not guessed): control repeats gave 31.9 / 32.0 / 32.6 / 31.2
decode tok/s and 1,048-1,056 tok/s of 8k prefill, and one control pass produced 807.**
So a single-digit percentage is nothing; an effect has to replicate.  Two harness bugs
found by this sweep are now fixed in `strata-exp.sh` — `/tmp/exp-strata.log` was shared
between arms (an arm's tail could quote its predecessor's `PCIe share:` line), and an
unresolvable bench path exited 0 without benchmarking (it silently ate the speed numbers
of arms 18 and 21).

## The arms

Decode is the **steady median** of the fast bench (reps 2-3, i.e. the 200-token
generations minus the first, which pays the pool warm-up); 8k/28k are `prompt/s` from the
full bench; `short` is the 70-token/200-token pass; `answer sha` is
`strata-answer-check.py` — three fixed greedy (temperature 0) prompts, `sha256` of
reasoning+content, first 6 hex.  Full rows: `strata-tune-report.py`.

| arm | decode tok/s | 8k | 28k | short | verdict |
|---|---|---|---|---|---|
| 00 control | 31.9 | 1,056 | | | baseline |
| 06 control | 32.0 | 1,048 | | | baseline |
| 22 control | 31.2 | 1,050 | | | baseline |
| 15 control (answers) | – | **807** | 1,123 | 13.6 | the noise floor's worst sample |
| 01 `--expert-cache 2000` | 31.8 | 1,041 | | | flat |
| 02 `--expert-cache 1500` | 30.2 | 1,041 | | | worse |
| 03 `--spec 6` | 29.2 | 1,048 | | | worse |
| 04 `--spec-min-p 0.35` | 30.5 | 1,047 | | | worse |
| 05 `--pcie-frac 0.85` | **28.1** | 1,050 | | | clearly worse |
| 13 `--aux-cpus auto` | 31.6 | 1,036 | | | flat |
| 14 `--aux-cpus 8-15` | 30.9 | 1,001 | | | worse |
| 17 `--pipeline-windows 2` | 30.4 | 1,048 | | | worse |
| 19 `STRATA_PREFILL_CPU_SHARE=0` | 29.5 | 1,016 | | | **worse: the CPU share pays** |
| 20 `STRATA_ADAPT_CALLBACK_WARMUP=1` | 32.1 | 1,046 | | | flat |
| 07 `STRATA_EMB_REUSE_ACCOUNT=1` | – | 1,022 | 1,146 | 22.1 | flat |
| 08 `STRATA_SELECT_TWO_PASS=1` | – | 1,022 | 1,139 | 22.3 | flat |
| 09 `STRATA_PF_SMALL=0` | – | 1,048 | 1,140 | 22.0 | flat |
| 11 `--prefill 8192` | – | 1,065 | **1,065** | 21.2 | 28k **−7 %**: the chunk is not the lever |
| 12 `STRATA_RESIDENT_HEADROOM_GIB=2` | – | 1,051 | 1,141 | 21.1 | flat |
| 10 `STRATA_PF_FUSED=1` | – | **1,170** | **1,160** | 21.2 | **the only win** |
| 16 `STRATA_PF_FUSED=1` (answers) | – | **1,099** | **1,161** | 23.0 | replicates, more weakly |
| 18 `STRATA_ROUTE_TAIL_SKIP=9` | – | – | – | – | bench lost to the path bug; answers move |
| 21 `STRATA_HC_SPLIT=0` | – | – | – | – | ditto, answers move |

What each group says:

- **Cache size is not the gate.** `auto` resolves to 1770 slots and forcing it to 2000
  did nothing (31.8) while 1500 cost ~5 %.  `STRATA.md` already recorded "bigger cache:
  no"; this is now measured on this card rather than borrowed from the 7900 XTX.
- **More speculative draft is not the gate either.** `--spec 6` −10 %, `--spec-min-p
  0.35` (accepting more marginal drafts) −5 %.  At 58-62 % acceptance the 0.1.59.1
  window is on the right side of its own curve; leave `--spec 4 --spec-min-p 0.5`.
- **The PCIe share dial points the wrong way for more traffic.** The auto rule already
  chose 0.70 from the pool's measured 300 µs/expert, and overriding it to 0.85 was the
  single worst decode result of the sweep.  The link is not the constraint — 13.4 GB/s
  against a pool that asks for 6.9.
- **Threads are not the gate, even though the crowding is real.** `/proc/<pid>/task/*/status`
  on the running engine: `strata-pool-0..6` own masks `{1}`…`{7}` (one logical CPU each,
  one per physical core), `strata-host` owns `{0}`, and **32 `strata-stage` threads plus
  `strata-watchdog` and `strata-stdin` all inherit that same `{0}`**.  The engine prints
  the fix it computed and left switched off: `strata aux cpus: threads that are neither
  pool workers nor the host go to CPUs 8 (off at start; a request's aux_cpus=1 turns it
  on)`.  Turning it on (`auto` = CPU 8; a list `8-15` = every SMT sibling, which upstream's
  `auto` deliberately refuses to take because it steals worker cycles) changed nothing —
  −1 % and −5 %.  So 32 staging threads sharing one core is a real oddity that is **not
  costing throughput**: they are waiting on the GPU and on the pool, not on a CPU.
- **The CPU's share of prompt-time pays.** `STRATA_PREFILL_CPU_SHARE=0` cost 7 % of
  decode and 3 % of 8k prefill.  It is auto-on here despite the row saying "prompt chunks
  under 1,024 tokens", and the short-message case is exactly where it applies.  Keep auto.
- **Nothing about the resident arena's size or headroom is a speed dial**: 2 GiB of
  headroom (arm 12), 8192-token chunks (arm 11), `STRATA_PF_SMALL=0`,
  `STRATA_SELECT_TWO_PASS`, `STRATA_EMB_REUSE_ACCOUNT` are all within noise, and the two
  28k numbers that moved at all moved the wrong way.

## The one win, and the measurement that decided it

`STRATA_PF_FUSED=1` at start-up prints `strata: prompt experts on the fused int8 kernels
(STRATA_PF_FUSED=1, #136)` and changes the chunking decision the log explains on its own:

> `small expert cache: the default prompt path would read in chunks of 6656 tokens
> (under 8192), so prompts read with the unfused experts and, from two chunks, the
> layer-major walk`

With the fused kernels the chunk becomes 8192, so an 8,089-token prompt is **one** chunk
instead of 6656 + a tail.  Measured in the arms, then again on the live endpoint after
deploying:

| | 8k prompt | 28k prompt | decode |
|---|---|---|---|
| control (five clean arm passes) | 1,048-1,056 tok/s | 1,123-1,146 | 31.2-32.6 |
| `STRATA_PF_FUSED=1`, arms | **1,170.5**, **1,099.4** | **1,160.2**, **1,160.7** | unmoved |
| **`STRATA_PF_FUSED=1`, live endpoint through the door** | **1,105.3**, **1,167.0** | **1,156.3**, **1,156.2** | 22.5 short / 32.3 |

### Why "the hash changed" was not the evidence to decide on

Upstream's own row is honest — "Fused int8 prompt experts … Replaces the MMQ prompt
kernels", in the table headed *Options that change answers* — and
`strata-answer-check.py` confirmed the drift on this pack with three greedy prompts:
`fib 7c8cd8→455df6`, `sortdoc 84ffb8→4b5d4b`, `arith f8423c→f8423c`.  **But a changed
fingerprint is not a worse answer, and treating it as one would be a category error:**
under greedy decoding a single tie-break diverges the entire continuation, so *any*
last-bit rounding change reads as "2 of 3 continuations differ".  That number measures
sensitivity, not quality, and the honest question is whether the model is still **right**.

So `strata-answer-score.py` exists: nine short items with an **answer key** (arithmetic,
integer division, a negative, a slice length, a required Rust expression, a Python `len`,
a binary literal, a modulus, a small sum), greedy, scored by regex on the emitted
`content` — with a third state, `INCOMPLETE`, because a reasoning model whose token budget
runs out mid-calculation has not answered *wrongly*.  (That distinction paid for itself on
the first control run: at `max_tokens=64` two items "failed", one of them showing
`…=1139` with the correct answer one token away.  A budget artefact nearly decided a
deploy.)

| | scored | correct | incomplete |
|---|---|---|---|
| control (live, before the line was added) | 8 | **8/8** | `bit` |
| `STRATA_PF_FUSED=1` (live, after) | 8 | **8/8** | `bit` |

Same score, same items, same one abstention — and the speed arrived.  **Deployed**,
`env.STRATA_PF_FUSED = "1"` in `strata.nix`'s `configFile`, with the numbers and the
revert in the comment above it.

What that does *not* claim: that 8 arithmetic-and-one-liner items are a code-quality
benchmark.  They are a guard against the blunt failure mode ("it got dumber in a way you
can see").  A real eval — perplexity, or KL against the unfused path over held-out
completions — is still the thing that would prove more, and it is still worth doing if
this switch is ever suspected.  And one datum worth keeping: `STRATA_HC_SPLIT=0`'s
fingerprints are `d97d83 / 4b5d4b / f8423c` — **`sortdoc` agrees with `PF_FUSED`'s** and
not with control, so the two switches plausibly perturb the same rounding step, which is
worth knowing if either is ever bisected against a bad completion.

## Shipped

- **`env.STRATA_PF_FUSED = "1"`** in `strata.nix`'s `configFile` — the one measured win
  (§above): +5 to +11 % at 8k, +1 to +3 % at 28k, decode unmoved, and answer-key-correct
  before and after.  It is not answer-*neutral*, and the revert is one line.
- **`boot.kernel.sysctl."vm.swappiness" = 10`** in `machines/services2/strata.nix`.  The
  arena is 23.9 GiB of *swappable* shmem on a 30 GiB box with a 16 GiB swap file, and at
  the kernel's default 60 the arena is the cheapest thing to reclaim — the pool's next
  expert read is then a swap read.  **Measured, so nobody oversells it: this does not make
  the box swap-free.** Swap went 450 MiB → 1.70 GiB (engine `VmSwap` 1.03 GiB) across the
  three 63k-token prefills above *at swappiness 10*.  What the dial changes is which pages
  the kernel reaches for first, and it moved nothing you can see: decode is 31.2-32.6 on
  control repeats either way.  **It had been applied earlier in the day with `sysctl -w`
  and would have died at the first boot; persisting it is the entire change.**  Rendered
  proof, because `grep -r` will not show it through `/etc/sysctl.d`'s symlinks:
  `grep vm.swappiness /etc/sysctl.d/60-nixos.conf` → `vm.swappiness=10`.

Nothing else in this sweep is deployed, because nothing else was both faster and
answer-neutral.

## Do not re-run these

To save the next person the 100 s/arm: **cache size (1500/2000), `--pcie-frac` upward,
`--spec` upward, `--spec-min-p` downward, `--aux-cpus` (both forms),
`--pipeline-windows 2`, `STRATA_PREFILL_CPU_SHARE=0`, `STRATA_ADAPT_CALLBACK_WARMUP=1`,
`STRATA_PF_SMALL=0`, `STRATA_SELECT_TWO_PASS=1`, `STRATA_EMB_REUSE_ACCOUNT=1`,
`STRATA_RESIDENT_HEADROOM_GIB`, `--prefill 8192`** — all measured 2026-11-14 on this pack
and this card, all flat or worse.  A rebuild that changes the expert path, or a different
pack (Swift 1.5's IQ1_M is a different geometry), reopens the question.

What is genuinely still open, in the order I would try it:

1. ~~**`STRATA_PF_FUSED=1`**~~ — **done, deployed 2026-11-14** (see §"The one win").  Its
   follow-up, if anyone wants a stronger claim than nine answer-keyed items: a real eval
   (KL or perplexity against the unfused path over held-out completions).
2. **A different prompt-shaped workload than a synthetic 8k wall of text.** Every prefill
   number here is `Repeat the digit N exactly.` × 9,000, which is the *best* case for the
   prefix cache and probably not what a pi session's context looks like.  Real receipts
   should come from `/var/lib/strata/strata.log`, which times every prompt
   (`prompt 8089 tokens = 0 reused + 8089 read in 7959 ms`).
3. **The pool's 300 µs per missed expert.** Nothing in this sweep moved it (threads,
   worker count from the aibox side, PCIe share), and it is the number the auto PCIe
   share is computed from, so it is where a structural win would live — but it is also
   the number that says "the pool is waiting on something", and the thing it waits on is
   the next small kernel, which is a rebuild-the-engine conversation, not a flag.
4. **Memory, which nothing here measured as a speed factor.** 23.9 GiB of shmem arena +
   the engine's ~1.5 GiB of resident anon leaves 3.4-4.5 GiB of headroom on a 30 GiB box,
   and a three-prompt prefill burst puts 1.7 GiB of it into swap (above).  The candidate
   dials are `--max-context 131072` (the KV reservation is what makes the arena compete
   with the engine; `--kv int8` is already on) and the file tier's page-cache appetite
   (`prefill plan: … file set 0.5 GiB fits the page cache 3.0 GiB`).  An arm that cuts
   `--max-context` in half and re-measures both phases would be the next useful number,
   and it moves nothing about the model — it only refuses long conversations.
5. **Hardware**, the only lever with a large coefficient: the 12 GB of VRAM is what makes
   63 % of a 28k prompt's expert reads go over PCIe at all.

## How to re-measure

```bash
# one arm, fast (3 decode reps + prefill), strata is down for ~2.5 min
sudo machines/services2/strata-tune/strata-exp.sh auto auto '{}' '["--aux-cpus","auto"]'
# with a full prefill sweep and the greedy answer fingerprints as well
sudo machines/services2/strata-tune/strata-exp.sh auto auto '{"STRATA_PF_FUSED":"1"}' '[]' \
    machines/services2/strata-tune/strata-bench.py strata-answer-check.py
python3 machines/services2/strata-tune/strata-tune-report.py   # /tmp/arm-*.log -> a table

# the greedy answer checks: the fingerprint (does output move) and the score (is it right)
python3 machines/services2/strata-tune/strata-answer-check.py http://127.0.0.1:8123
python3 machines/services2/strata-tune/strata-answer-score.py  http://127.0.0.1:8123
python3 machines/services2/strata-tune/strata-answer-score.py  https://strata.int.leighhack.org  # live, via the door

# the box under real traffic, phases labelled in one CSV, no downtime (~3 min)
sudo python3 machines/services2/strata-tune/strata-phases.py   # -> /tmp/strata-phases.csv
```

The cuBLAS probe is kept beside the harness as `gemm-probe.cu` + `gemm-probe.default.nix`
but is **deliberately not wired into the flake**: `callCuda` comes from an unfree-marked
package set, and nothing should have to build a CUDA binary to evaluate this repo
(AGENTS.md, "Evaluation must not build anything").  The nix file carries the three lines
that build and run it.  What it said: **26 TFLOP/s of bf16 at n=4096, by CUDA events and
by wall clock, three runs**, at 89.6-95.2 W - which is the card's peak, and the fact that
made the "prefill is power-capped" model collapse.  Its one loose end is a print bug: it
emits `cublas …:84 code 0` once per GEMM, where 0 is *success*, because of how its status
macro is written; the rate is unaffected (two independent clocks, and a GEMM that had
really failed would have made the measured figure 2× the card's peak rather than equal to
it), and the comment in the source says so rather than hiding it.

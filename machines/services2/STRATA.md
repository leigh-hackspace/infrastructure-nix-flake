# Strata on services2 — Qwen3.8-Flash-Next Coder on the RTX 3060

Second Strata instance in the network. `strata.service` serves the **Coder**
variant of Qwen3.8-Flash-Next (IQ1_M, 256 of the original 512 experts kept) on
services2's **RTX 3060** (GA106, Ampere, sm_86), with the non-GPU experts in
system RAM and the ~27 GiB n-gram table read from the NVMe.

- Config: `machines/services2/strata.nix` (service) + `strata-package.nix` (the
  CUDA engine build).
- Endpoint: `http://10.3.1.21:8080/v1` (OpenAI chat/completions,
  `/v1/messages` for Anthropic-style clients, `/v1/models`, `/health`).
  `services2.int.leighhack.org` already resolves to 10.3.1.21, so clients can
  use that name with the port; the front door (`gw`) also terminates TLS for it as
  `https://strata.int.leighhack.org` and publicly as `https://strata.leighhack.org`
  (both rows in `common/front-door.nix`).
  The two LAN names are in `allowed_hosts` in `machines/services2/strata.nix`;
  the public one deliberately is not — see the Host-check note below.
- API key: every `/v1/*` call needs `Authorization: Bearer <key>` or `x-api-key`
  (`_authorized()` in `serve/server.py`).  The value is the `strata_api_key` sops
  secret, read into the engine's config as `api_key`; `/health` reports
  `"api_key": true` and needs no key.  The LAN name stays keyless for its clients
  because services1's nginx presents the key on their behalf, which is what keeps
  the engine's own dashboard at `/` working from a browser: that page is a set of
  `/v1/*` calls and 401s on every one of them if nobody holds the key.  Rotating
  it means `sops edit secrets/secrets.yaml` **and** re-deploying both boxes —
  services2 for the engine, services1 for the injected header.
- Logs: `journalctl -u strata -f`, `/var/lib/strata/strata.log`.

The aibox instance (`machines/aibox/strata.nix`, `machines/aibox/STRATA.md`) ran
the **full 125B model on the Radeon iGPU** until it was parked on 2026-10-17, when
that box became the network's build machine; the module and measurements are kept.
That doc's measurements (decode vs prefill, why the expert cache is a compute
tier, the UMA reality) are the background reading; this file is what is different
on a real, discrete 12 GB card and a 30 GB box.

## Why the Coder pack here

The full model's expert arena is ~35.5 GiB; services2 has ~30 GiB usable RAM and
must also run Prometheus/Grafana/gatus. The Coder's arena is **~23.4 GiB**
(`setup.py`: `"IQ1_M": {"ram_gb": 32, "arena_gb": 23.4}`), which is the only
reason it fits. Its shard 2 (the `per_layer_token_embd` table, ~26.8 GiB) is the
same file as the original model's shard 2 — the engine reads it from the SSD
(`--ple-io direct`), it is not loaded into RAM.

## Sizing (services2: ~30 GiB RAM, 12 GB VRAM, 16 GiB swap file)

| item | size | where |
| --- | ---: | --- |
| expert arena (Coder IQ1_M) | 23.4 GiB | split RAM/VRAM |
| experts the GPU does not hold | ~16 GiB | RAM, pinned once (`--resident-experts`) |
| expert cache | `auto` | VRAM, sized from free VRAM |
| dense weights + buffers | ~5 GiB | VRAM |
| KV int8 @ 262144, streamed | 0.9 GiB VRAM + 3.09 GiB pinned RAM | split (see "256K at full precision") |
| PLE table | 0 | read from NVMe (`--ple-io direct`) |

The model's own RAM figure (32 GiB) is above this box's, so the engine runs its
**low-RAM mode**: `--resident-experts` copies the experts the GPU cache does not
hold into RAM once, instead of the full arena being resident. `setup.py` picks
exactly this variant when the arena does not fit but the non-GPU part plus
~10 GiB of headroom does (`low_ram_needed` / `low_ram_resident`). The alternative
is `--mmap-experts`, which reads them through the OS file cache; it is faster in
some reports but lets RAM fill to zero under long prompts, which is not wanted
next to Prometheus.

The KV is **not** streamed to RAM and **cannot** be made elastic here; both
possibilities are dead ends with a guard at the front of them, and the numbers are
in the next section, because that is where the context question actually lives.

## Context and the KV cache: what each option costs (measured 2026-11-18)

Asked: can this box reach 256K, or at least 192K?  **Yes, both — at 262144 the price
is ~11 % of the expert cache and ~10 % on long prompts, and it is paid for by the KV
format, not by anything else.**  Six arms were run on one engine build, one pack and
one bench, changing only `--max-context` and `--kv` (`prompt/s` from
`machines/services2/strata-tune/strata-bench.py` and its 200 K-token sibling
`strata-needle.py`, slots and chunk from the engine's own start lines):

| `--max-context` | `--kv` | cache slots | prompt chunk | 8k prompt | 28k prompt | long read | needle |
| --- | --- | ---: | ---: | ---: | ---: | ---: | :---: |
| 131072 (was) | int8 | 1781 | 7680 | 1064 | 1193 | 1118 @127,669 | 2/2 |
| 196608 | q4_0 | **1878** | 8192 | 1111 | 1097 | 1038 @127,669 | 2/2 |
| 196608 | k8v4 | 1558 | 6400 | 966 | 1020 | — | not run |
| **262144 (now)** | **q4_0** | **1580** | 6400 | 990 | 1055 | 995 @138,016; 974 @183,987 | 2/2 |
| 262144 | k8v4 | 1152 | 4352 | 954 | 933 | 892 @138,016 | 2/2 |
| 262144 | int8 | 785 | 2048 | 806 | 785 | 744 @138,016 | 2/2 |

`prompt_n` is in the long-read column because it is the only fair way to compare a
long prompt: the tokeniser, not the caller, fixes it.  Decode did not move in any
arm — **37 to 45 tok/s across the whole table, with no ordering by config** — which
is the shape of this box: decode is bought from the RAM tier and the experts, and
the KV only really bites on the read.  (The 974 figure is the one number here taken
with a nix build running on the same box, so read it as a floor.)

**The mechanism, measured.** The card is left with 628–632 MiB free in *every* arm,
because `--expert-cache auto` spends whatever is spare on slots.  So context is not
paid for out of headroom: it is paid for out of **cache slots and the prompt chunk**,
and both fall together — at 262144 with int8 the chunk is 2048, a quarter of the
131072 arm's.  That is why `--max-context` alone is the most expensive way to ask
for a longer context and `--kv q4_0` is the cheap one: the KV's bytes are what fund
both.  Bytes per token, from `strata_control/vram_fit.py:49` (`B per token per
attention layer × 13 layers`), which the engine's own sizing agrees with:

| `--kv` | B/cell/layer | per token | 262144 | in words |
| --- | ---: | ---: | ---: | --- |
| fp16 | 2112 | 26.8 KiB | 6.7 GiB | does not fit; the arm was not run |
| int8 | 1056 | 13.4 KiB | 3.4 GiB | upstream's own figure: "the K/V of a 262K context (3.35 GiB with the drafter's at int8)", `generate.cpp:10785` |
| k8v4 | 816 | 10.4 KiB | 2.6 GiB | INT8 K (exact scores) + rotated Q4_0 V |
| q4_0 | 576 | 7.3 KiB | 1.9 GiB | Hadamard rotation, then 4-bit codes |

**What each KV format costs in answers** — this is the part this box cannot measure
for itself, so the number is upstream's (`bench/results/2026-09-27-kv-q4/README.md`,
teacher-forced, same text, same expert set, only `--kv` differing; fp16 the
reference).  int8 is *indistinguishable from fp16* everywhere, and q4_0 costs more
the longer the document is:

| text | q4_0 same top-1 as fp16 | q4_0 perplexity vs int8 | q4_0 KL from fp16 / int8's |
| --- | ---: | ---: | --- |
| code agent (512) | 95.7 % | −2 % | 0.030 / 0.014 |
| chat (1024) | 90.7 % | +1.5 % | 0.028 / 0.009 |
| document (1024) | 91.3 % | **+7 %** | 0.039 / 0.009 |
| document, 8K | 88.0 % | **+13 %** | 0.297 / 0.114 |

k8v4 sits between them: 23 % less KV memory than int8, "more precise than `q4_0` on
long documents, prompts 2–5 % slower" (`docs/DETAILS.md:234`), and its one big
measured win is this model on a 24 GB card — 198K context, Coder, `--kv k8v4`: **99
tok/s instead of 85**, same needle results (`docs/DETAILS.md:1435`).  It is the right
choice on a card with room to spend the saving; on this 12 GB card at 262144 it does
not buy enough to keep the chunk up (4352 against q4_0's 6400), so it reads slower
than q4_0 at every depth and this box took q4_0.

What our own instruments could and could not see: the answer key is **unchanged in
every arm** (8 of 9, with the same `bit` item failing in all of them — nine short
items cannot see a long-document perplexity), and retrieval is intact: two unique
records buried at 2 % and 97 % depth of a 127,669- and a 138,016-token prompt come
back 2/2 with q4_0, and the deployed config reads a **183,987-token** prompt the
same way — a prompt this box could not accept before.  `strata-needle.py` is that
check, kept.

**262144 is the trained length, so nothing here needs rope scaling.**  Upstream
sizes YaRN against it: "final context / 262,144, at least 1", and an explicit
`--rope-scaling none` above 262,144 is *refused* (`docs/DETAILS.md:632-633`).
Anything past 262144 is therefore a YaRN experiment plus the VRAM for it — and
YaRN's own evidence is that it works (a `yarn 2` run read 421K and `yarn 4` 714K at
8/8 on real-document Q&A, `docs/DETAILS.md:1570`) but that is a different question
than this one.

### 256K at full precision, at this box's speed: the KV in RAM, and what it bills (deployed 2026-11-18)

Asked right after the table above: can 256K be had **at close to the old speed and
without giving up output quality**?  That table says no, because it only spends VRAM — and
the answer is **yes, by spending host RAM instead**, which is what the box now runs:

```
--max-context 262144  --kv int8  --kv-resident 32768   +   env.STRATA_RESIDENT_HEADROOM_GIB = 2
```

Measured on the deployed generation (`strata-bench.py`, `strata-needle.py`,
`strata-answer-score.py`, and the engine's own start lines), against the config this box
ran an hour earlier:

| | 131072 int8 (was) | **262144 int8 + streamed (now)** |
| --- | ---: | ---: |
| expert cache | 1781 slots | **2332 slots** (4.47 GiB) |
| prompt chunk (auto) | 7680 | **8192** |
| 8k prompt | 1064 | **1210 / 1204** (+13 %) |
| 28k prompt | 1193 | **1202 / 1199** (+0.7 %) |
| 184k-token read | — | **1081 tok/s**, needle 2/2 at 2 %/97 % depth |
| short decode | 39.7 | 46.7 |
| answer key | 8/9 (`bit`) | 9/9, then 8/9 (`bit` again — that item is the flaky one) |
| KV precision | int8, indistinguishable from fp16 | **the same int8, and the streamed path is checked against the resident one bitwise** (`kv_stream_parity` ctest) |
| MemAvailable, idle | ~3.9 GiB (12.7 % of RAM) | **~1.1–1.6 GiB (~4 %)** |

Why it works: at 32768 cells per QSA layer a session holds ~0.9 GiB of KV instead of
3.4 GiB, so ~2.5 GiB comes back and `--expert-cache auto` spends it — on slots *and* on
the prompt chunk.  A context of ≤ 32768 cells is **not streamed at all**, so ordinary
turns never touch the host copy; the long ones pay one DMA per array to it
(`STRATA_KV_HOST_DMA`, default on NVIDIA since 0.1.61 — the bump made this config cheaper
than it would have been a week ago).

**The line to read after any change to this start, before any benchmark:**

```bash
sudo grep "lendable slots" /var/lib/strata/strata.log | tail -1
```

It must read **`1533 of the prompt path's 1533 lendable slots keep their experts in RAM
too`**.  During a prompt the prompt path borrows most of the cache, and every borrowed
slot wants its expert in RAM; when they are not there the read goes to the NVMe
mid-prompt and the speed collapses.  Four arms, same VRAM, differing only in how much RAM
the resident expert copy was allowed to keep:

| `STRATA_RESIDENT_HEADROOM_GIB` | complement | lendable covered | 8k prompt | 28k | 184k read | MemAvailable after that read |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 4 (`--resident-experts`' default) | 19.85 GiB | 475/1533 (31 %) | **562 / 725** | 1028 | 1045 | — |
| 3 | 20.9 GiB | 1179/1533 (77 %) | 917 / 1186 | 1199 | 1077 | 859 MiB |
| **2 (deployed)** | **21.88 GiB** | **1533/1533** | **1210 / 1204** | **1202** | **1081** | 1006 MiB |
| 2 with `--kv k8v4`, at headroom 4 | 20.96 GiB | 1099/1503 (73 %) | 908 / 1068 | 1061 | 959 | 1503 MiB |

The engine names the failure and the remedy in one line (`src/core/expert_source.cpp:4264`):
*"Free RAM or lower STRATA_RESIDENT_HEADROOM_GIB (now the RAM left free at start) and
restart; the line above should read N of N."*  That is the dial; `--expert-cache` is not,
and neither is the KV format once the KV has left VRAM.

**What it costs, once.** 21.88 GiB of experts page-locked + 3.09 GiB of pinned K/V + ~4
GiB of OS and monitoring ≈ 29 GiB of a 30.7 GiB box; the old config was ~27 GiB, and the
difference is exactly what streaming bought in VRAM.  In order of how much they matter:

- **The fleet's `LowMemory` alert now fires on this box.**  It is
  `node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes * 100 < 10` for 5m
  (`machines/services1/services/observability.nix:101`) = 3.07 GiB here, and the box runs
  at 1.1–1.6 GiB.  It sat at 12.7 % before this change, so the threshold has always been
  this box's ceiling in disguise; making it per-instance, or buying the RAM, are in
  `docs/follow-ups.md`.  Do not respond by paging someone: make the rule know this box.
- Swap sits at ~1.7–2.3 GiB (the 16 GiB file; `vm.swappiness = 10`, per the section after
  this one — which stays true, and now matters more).
- No room for a spike: a `nix build` here (this box has no builder, so it builds locally),
  a Prometheus churn, or a second 180k-token read at once.

**The fix is hardware and it is small:** the board takes 128 GiB and has **two DIMM slots,
both filled with 16 GiB DDR4-3200 running at 2933** (`dmidecode -t memory`), so it is a
2×32 GiB swap rather than an added stick.  With 64 GiB the headroom goes back to 4, the
`lendable slots` line still reads N of N, `LowMemory` stops firing, and the shape is
exactly upstream's own measurement of this lever (a 12 GB card with 45 GiB of RAM: 756 →
1267 slots, 32K prompt 21–34 s → 12–17 s, RAM used unchanged because the expert budget
absorbs the KV).

**Rollback, and both directions are short:** delete the `--kv-resident` pair and the
`env.STRATA_RESIDENT_HEADROOM_GIB` line and set `"--kv"` to `"q4_0"` (990 / 1055, 974
@184k, ~3.9 GiB free, −12 % at 28k, small precision cost on long documents); or keep int8
and set `context = 196608` (int8 at 192K needs 2.8 GiB of VRAM, so the cache lands near
1240 slots with ~3.9 GiB of RAM free — the old speed band, no precision cost, 192K
ceiling).

### The lever setup says no to, and what the no was really about

Both hand the cache its slots back, and both are gated, so write the gate
down rather than rediscovering it:

- **KV streaming (`--kv-resident N`) — this is what the box now runs;** what follows is
  why its own tooling declines to, and where that was right.  It puts the whole K/V in
  host RAM and keeps only N cells per QSA layer in VRAM, and upstream's measured result on
  a 12 GB card at 128K is the one this box wants — **756 → 1267 expert slots, a 32K prompt
  in 12–17 s instead of 21–34 s** (`kv_streaming_need_gb`'s docstring, `setup.py:6494`,
  citing KV-128).  Setup will not choose it here: it streams only when the box has
  `min(ram_gb + kv + 1, arena + 10 + kv)` GB, i.e. **34.0–36.6 GB across 131072–262144**
  (`LOW_RAM_HEADROOM_GB = 10`, `setup.py:6405`) against this box's 30.7.  That is why this
  file said "not an option" on the morning of 2026-11-18 and was wrong about the outcome
  while being right about the bill: the rule refuses to spend RAM the box has not got, and
  spelling the flag into `engineArgs` is the owner's override — but the RAM the K/V takes
  (3.09 GiB) comes **out of** the resident expert copy, not on top of it, which is why the
  only thing that needed tuning was how much of that copy survives (the headroom table
  above), and why the override left untuned reads 562 tok/s instead of 1210.  What the
  0.1.61 bump changed in our favour: streaming on NVIDIA now ships a prompt's new K/V
  cells to the host copy in one DMA per array (`STRATA_KV_HOST_DMA`, upstream
  `f0356cb25`/`6aae30348`).
- **The elastic K/V (`--kv-grow`).** The one whose description is written for this
  box — "the K/V takes VRAM only for the cells the requests reach and the expert
  cache holds the rest, giving slots back as the context grows … it pays on cards of
  about 24 GB or less with contexts of 128K and more", and whose 0.1.40 record is a
  262144 start that would not otherwise start (`docs/DETAILS.md:236`, `:1441`) —
  **cannot run here at all**: its gate (`generate.cpp:7287-7295`) requires
  `!o.resident_cpu_experts`, and this box is in the resident low-RAM mode, since
  `--resident-experts` *is* `--mmap-experts --resident-cpu-experts`
  (`generate.cpp:4021`).  Start it anyway and the engine says so: `--kv-grow is off
  (one GPU, a profile, the whole K/V in VRAM, every expert in RAM, no --batch …)`.  To
  earn it, the arena has to live in a pinned arena instead — and what the box lacks
  for that is not the 21.8 GiB of arena but the **10 GiB of headroom** the same rule
  asks beside it.

**So the dial is `--kv`, and what it is dialled to is above:** 262144 with int8
**streamed into host RAM** (full precision, the old speed or better, and the box's RAM
headroom spent) — or `--kv q4_0` unstreamed if that headroom is wanted back (−7 % at 8k,
−12 % at 28k, ~3.9 GiB free).  Either is a few lines and `just switch`.

**Swap is a 16 GiB file, `/var/swap/services2`** (`./hardware-configuration.nix`; there is
no swap partition on this NVMe). It is not there for the model: the arena is page-locked where
it can be (`LimitMEMLOCK = infinity` in `./strata.nix`), so what swaps is the overflow. It is
there because strata measured **28.3 GiB RSS** against 30 GiB of RAM, which left ~1.5 GiB for
Prometheus/Grafana/gatus/exporters and for the nix builds this box was the network's build
machine for until 2026-10-17 (`maxJobs 8` then; aibox held the role from then until
2026-11-23 and nothing holds it now) — without it the box
went unresponsive
(journald: "Under memory pressure, flushing caches") and a build here was OOM material. If
`--resident-experts` is ever lowered, or a bigger pack lands here, re-read this table: the
swap file is headroom, not a licence to size the arena above RAM.

**`--expert-cache` is `auto`, not a number.** On this discrete card `auto` reads
dedicated VRAM (aibox's iGPU warning — that `auto` reads GTT and fills the
machine — does not apply). If the engine's log shows VRAM pressure, set an
explicit smaller number, as aibox does.

## The conversation cache cannot be bought with the swap file

Asked on 2026-10-27 whether the 16 GiB swap file could be the cache's home, so
`--conversation-cache-mib 0` could be lifted. It cannot — three mechanisms and a
number, all of it read off the 0.1.52 tree and the running engine, not argued.

- **The guard is RAM-only.** `available_host_bytes()` (`include/strata/core/conversation_memory.hpp:12-24`)
  is the smallest of `MemAvailable`, the room under every visible cgroup limit, and `--memory-limit-mib`;
  nothing swap-related is in it. The page file enters only the **Windows** commit check
  (`generate.cpp:9296-9310`, `host_limits::commit_verdict(cs.ullAvailPageFile, …)` — Windows commit
  accounting is virtual by construction). So `auto_conversation_cache_mib` answers 0 at this box's
  numbers whatever the swap file is doing: `free -m` said 3,452 MiB available with the 0.1.52 engine
  loaded, `3452 − 2560 = 892`, `892 × 2/5 = 356`, under the 1024 floor → 0.
- **Parking itself is refused on the same floor, not only its sizing.** `docs/DETAILS.md` (the
  `--conversation-cache-min-free-mib` row): "Parking and session files are skipped when less than the
  snapshot plus the floor is available", against that same RAM figure. An explicit
  `--conversation-cache-mib N` therefore parks nothing here until the **floor** is lowered too — and
  lowering the floor is a different argument than "use swap": it spends the box's last GiB, the one
  that keeps journald from its "Under memory pressure, flushing caches" state.
- **Swapping the cache is a re-read on the prompt path.** The parked bytes are host segments of 16 MiB
  (`include/strata/core/conversation_buffer.hpp:21`, pageable) and a restore reads them back per
  resume. The box already pays swap for the *arena* — `src/core/pinned.cu:267` reserves it as
  `MAP_PRIVATE | MAP_ANONYMOUS`, so on Linux "page-locked when possible" is Windows' wording: the
  22.78 GiB resident expert copy is swappable, and 2,229 MiB of the swap file was in use at the 0.1.52
  start while the log said `resident RAM: 22.78 GiB of experts in RAM … 0 blob reads from the file`.
  Upstream's cure for the parking stall (`docs/DETAILS.md:398`) is "a smaller `--resident-budget-gib`,
  a smaller `--conversation-cache-mib`, or the pagefile on an SSD" — ours already **is** that SSD file
  (`/var/swap/services2`), so the only lever left in that list is size.
- **The number that settles it: this workload already reuses 93 % of its prompt tokens without the
  cache.** `/metrics` at the 0.1.52 start reads `conversation_cache/requests = 37`,
  `requests_reused = 33`, `reused_tokens = 2,743,518` of `prompt_tokens = 2,945,937` (last request
  69,578 of 69,761) — that is `--prompt-cache`, default 6 checkpoints (`generate.cpp:805`), already on.
  VRAM cannot take the park either: `engine/vram_free_mib = 408` with the expert cache at 3,754 MiB,
  and upstream's `LOW: requests may stall; add --vram-reserve-mib` line is 256 MiB.
- **If it is ever wanted, open RAM rather than borrow it:** `--resident-budget-gib N` (`generate.cpp:621-623`,
  "the N GiB of experts the GPU cache does not hold that the expert profile ranks hottest are copied
  into RAM, the rest are read from the files in place") trades measurable file cost for headroom —
  today the tier line reads `files 0 blobs 3939.5 MB read (the GGUF in place)` — then set
  `--conversation-cache-mib` and a lower `--conversation-cache-min-free-mib` together, and read the
  result off `/metrics`' `conversation_cache` card (`serve/server.py:834`, upstream `#596`: parks,
  restores, evictions, bytes) and the engine's `conversation cache: (parked|skipped|restored)` lines.
  Size the ask before setting it: a parked conversation is its K/V, ~1.8 GiB at 131072 with `--kv int8`
  (the table above), so four full-length slots are ~7 GiB, against upstream's cap of 8192 MiB
  ("four 100K-token agent conversations were parked in 8 GiB in the report", `conversation_memory.hpp:68`).

## One-time model preparation (outside the store)

The GGUFs are already at
`/home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-Coder/`. The pack and the
MTP draft runtime are prepared by hand before the unit will start (it has
`ConditionPathExists` on the pack index, so it will not crash-loop before then).
Run with the engine's own wrapper:

```bash
# From the checkout. Do NOT use `nix build --impure --expr 'let f = builtins.getFlake
# (toString /home/leigh-admin/Projects/infrastructure-nix-flake); in ...'`: a path ref
# copies the whole working tree (build trees + .git, 6.7 GB+) into the store on every
# call, ~5 min each time — see docs/nix-dev-workflow.md.
STRATA=$(cd ~/Projects/infrastructure-nix-flake && nix build .#strata-cuda --no-link --print-out-paths)

# 0. check the card before preparing anything
$STRATA/bin/strata-device --list-devices   # expect sm_86 / RTX 3060
$STRATA/bin/strata-device --selftest

# 1. the pack (index + dense tensors + tokenizer, ~1.5 GB)
mkdir -p /home/leigh-admin/Strata/pack/coder-iq1m
$STRATA/bin/strata-prep iq_pack.py \
  --gguf /home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-Coder/Qwen3.8-Flash-Next-GSQ-RCO-IQ1_M-00001-of-00002.gguf \
  --out /home/leigh-admin/Strata/pack/coder-iq1m

# 2. the MTP draft runtime (~6 GB; the GSQ-RCO GGUF ships no MTP head, so the
#    31 mtp.* tensors come from the BF16 checkpoint)
$STRATA/bin/strata-prep mtp_fetch.py fetch  --out /home/leigh-admin/Strata/mtp
$STRATA/bin/strata-prep mtp_fetch.py verify --out /home/leigh-admin/Strata/mtp
$STRATA/bin/strata-prep mtp_pack.py --src /home/leigh-admin/Strata/mtp \
  --experts q2_0 --out /home/leigh-admin/Strata/mtp/mtp-q2_0.gguf
$STRATA/bin/strata-prep mtp_rt.py --gguf /home/leigh-admin/Strata/mtp/mtp-q2_0.gguf \
  --out /home/leigh-admin/Strata/mtp/rt
cp $STRATA/share/strata/data/draft_vocab.bin /home/leigh-admin/Strata/mtp/rt/

systemctl start strata
```

## Building the engine

```bash
nix build .#strata-cuda     # the same derivation the service installs
```

The source is the flake's **`strata` input, tracked on upstream `main`** (the
point of "bleeding edge"): `nix flake update strata` moves it, and the service
and the `.#strata-cuda` package rebuild from the same tree. The input is
`flake = false` — upstream deleted its root `flake.nix` in the 0.2.16 layout move,
so it is a source and not a flake (see the 2026-11-21 update below) — and the
llama.cpp revision the engine builds is now **vendored inside that tree** at
`engine/third_party/strata-ggml` (`UPSTREAM.md` names `3cf03257f219…`, the same
pin `third_party/ggml/VERSION.txt` carries), so the ggml side is reproducible from
the one lock rather than from a second `fetchFromGitHub`.

The CUDA build is `STRATA_ENABLE_CUDA=ON`, `-DCMAKE_CUDA_ARCHITECTURES=86`, and
CUDA 13.0 (`cudaPackages_13_0`), configured in `engine/` by `cmakeDir`. The
`-DSTRATA_PREFILL_MMQ=ON` that aibox used to pass is **gone from both packages**:
0.2.16 removed the option and llama.cpp's MMQ prompt kernels with it, so Strata's
own prompt kernels are the whole engine now — neither copy the flag from the other
package, nor expect it in a `cmake --help-string`.

### Updating the engine (what a bump actually touches)

`nix flake update strata` **is** the bump: the input tracks upstream `main` and
nothing anywhere pins a version, so the locked rev is the whole contract and the
first thing it changes is behaviour nobody reviewed.

**The one place the input *does* state its version is the CMake project line** —
`CMakeLists.txt:11 project(strata VERSION 0.1.52 LANGUAGES CXX)` (0.1.48.1 at the
previous input), which becomes the `STRATA_VERSION` macro (`add_compile_definitions`,
line 23) and is what `strata --version` / `-V` prints (`src/program/generate.cpp:2873`,
`std::printf("strata %s\n", STRATA_VERSION)`) before any GPU is touched. **So the
installed binary names the rev it was built from: `$STRATA/bin/strata --version` is the
cheap proof of which engine a switch landed**, and `--families` next to it prints the
GGUF architectures the build runs (`qwen4exp qwen35moe` when `STRATA_HAVE_QWEN35` is
compiled in, `CMakeLists.txt:873` `target_compile_definitions(strata PRIVATE STRATA_HAVE_QWEN35=1)`).
What has no marker is the *app* layer — `setup.py` and the feed files carry no version
strings, and the Python tree is copied rather than versioned — which is why the
check below the ritual is `--version` and not a grep.

**The Python layer is copied by hand, and that is where a bump bites.** The
install phase of `strata-package.nix` copies `serve/`, the engine's helper
packages, `tools/`, `data/` and the pinned `gguf-py` into `share/strata`, and
`strata-server` is a `python -m serve.server` wrapper with only that tree on
`PYTHONPATH`. Upstream restructures that layer without warning: 0.1.46 renamed
`apps/cli` to `strata_cli` and moved the process/config helpers into a new
`strata_control`, which `serve/lifeline.py` imports at **module** scope. A tree
whose copy list lags that boots as `ModuleNotFoundError: No module named
'strata_control'` — the service crash-loops every 10 s (never-give-up) and the
switch exits 4 on the unit's failed first start. So `strata-package.nix` now
imports `serve.server`, `strata_control.procs` and `strata_cli.client` at
*install* time with only the installed tree on the path: a copy list that lags
upstream fails the build in two seconds instead of eating a three-minute model
load on a box with 30 GiB of RAM.

Two contracts, checked against the new source before deploying, both of which
have already moved once:

- the engine flags in `strata.nix`'s `engineArgs` — parsed by
  `src/program/generate.cpp` (`--pack --native --ple-gguf --expert-profile
  --expert-cache --prefill --spec --spec-min-p --mtp --max-context --kv
  --resident-experts`);
- the JSON keys of `configFile` (`exe args cwd tokenizer model_name log
  allowed_hosts api_key backend lib_dirs env`) — read by `serve/server.py`.

**Build the CUDA engine here and the HIP engine on aibox — and the reason is a
measurement, not a preference (2026-11-21).** What this file used to say, "build it
on aibox", is right about *competition for the engine's RAM* and wrong about the
CUDA engine: aibox's store has never held services2's toolchain (services2 has no
builder and has always built its own toplevel), so `nix build .#strata-cuda` there
spends its first 25 derivations **compiling the CUDA 13.0 toolkit from source** —
it was still on `cuda_nvcc` and `libcublas` after ten minutes, before it had read a
line of Strata. services2 has that whole toolchain warm, and the same engine built
here in **under five minutes**, which is why the copy step below is not part of a
CUDA bump at all (it is what the 2026-11-18 bump used, and it cost nothing then
only because the path was already in the store). The HIP half is the other way
round: aibox has ROCm warm and builds `.#strata` in ~5 min, so build the HIP engine
there — `machines/aibox/strata-package.nix`.

```bash
# the CUDA engine: here, where the toolchain is
nix build .#strata-cuda --no-link --print-out-paths
just switch            # builds the toplevel from the store, activates, confirms

# the HIP engine: on aibox, where ROCm is
ssh 10.3.1.32 'cd ~/Projects/infrastructure-nix-flake && nix build .#strata --no-link --print-out-paths'

# and *if* a CUDA engine ever has to be built on aibox, warm it first — this copies
# the toolchain (1.4 GB) instead of compiling it, and it does not build Strata itself:
nix copy --to 'ssh://leigh-admin@10.3.1.32?ssh-key=/home/leigh-admin/.ssh/agent-hop-key' '.#strata-cuda^*'
# on services2, as root: `trusted-users = root`, and the path is unsigned because
# aibox's daemon signs nothing it builds, so --no-check-sigs (only a trusted
# client may ask the daemon to add an unsigned path)
sudo nix copy --no-check-sigs \
  --from 'ssh://leigh-admin@10.3.1.32?ssh-key=/home/leigh-admin/.ssh/agent-hop-key' \
  /nix/store/<new strata-unstable>
```

**Do not confirm a failed activation.** This is AGENTS.md's rule, and this box is
where it bit (2026-10-20): the first start of the new unit failed, so
`switch-to-configuration` exited 4 *after* `/run/current-system` had flipped, with
`system-good` still on the old generation and `auto-rollback` armed for five
minutes later. A `nixos-confirm` at that moment marks the half-activated system
good and stops the timer — the only undo disappears — and on a shared box that is
not a hypothetical: while this one sat in that state, a second session confirmed
it, so `system-good` moved to the generation whose activation had failed and the
timer stopped (`systemctl show auto-rollback -p ExecStart` timestamps are the
receipt). Fix forward instead: keep the tree dirty-but-good, re-switch, and confirm
*that* (which is what happened here; the fixed engine came up 52 s after the new
unit started).

### Update: engine 0.1.61 → **0.1.63** (app 0.2.17), **the layout move**, deployed on services2 — 2026-11-21

`nix flake update strata`: `c77572806` (2026-11-16) → `3142b66b9` (2026-11-20),
**1005 commits**, and this input moves both engines (the `flake.nix` header). It
also refused to update at all until the flake changed, which is the headline.

- **Upstream moved its whole tree, and that broke the input before it broke the
  build.** `4058aa782` "Layout move for 0.2.16: the top level becomes app/, engine/,
  terminal/, docs/, tools/ and .github/" (2026-11-17) **deleted the repository's root
  `flake.nix`** — its devShell lives at `tools/nix/flake.nix` now, still no more than
  that — so as an ordinary flake input this tree cannot be locked:
  `error: path '«github:Niko1221/Strata/3142b66b9…»/flake.nix' does not exist`, and
  `flake.lock` is left untouched, so a `nix flake update strata` in a script reports
  the failure and the bump silently does not happen. The fix is on the input:
  **`flake = false`** (`flake.nix` carries the reasoning). Consequences to remember:
  the input *is* its store path, so `inputs.strata.outPath` is gone (both
  `strata.nix` modules and both `packages.*` now pass `src = strata;`), the
  `inputs.nixpkgs.follows` line went with it — a source has no inputs to pin — and
  `nix flake update` prints `Removed input 'strata/nixpkgs'` as it does it.
- **What the new layout is, and the two lines of packaging it cost.** `engine/` is
  the CMake project, `terminal/` is the Python layer (`serve/`, `strata_control/`,
  `strata_cli/`, `tools/`, `data/`), `app/` the TypeScript host and desktop front
  ends, `tools/` upstream's CI and release scripts. Both packages gained
  **`cmakeDir = "../engine"`** and a copy list that reads `terminal/...`. Two
  details, because both look free and are not: the *whole* input has to stay the
  source root — `engine/cmake/strata_gpu_engine.cmake:495` compiles
  `../terminal/tools/hip/tune_rocblas.cpp`, so a `src` narrowed to `engine/` breaks
  the HIP build — and `cmakeDir` rather than `sourceRoot`, because stdenv
  `chmod -R u+w`s `sourceRoot` after unpack (read-only store subpath) while the
  cmake hook simply configures `$cmakeDir` from inside `build/`.
- **llama.cpp is now vendored at the pin this file already carried**, so the
  `fetchFromGitHub` and `-DSTRATA_GGML_DIR` are gone from **both** packages:
  `engine/third_party/strata-ggml/UPSTREAM.md` pins `3cf03257f219…` (the same
  revision, the same one `third_party/ggml/VERSION.txt` names), byte-identical to
  llama.cpp except for CMake trims, and it ships gguf-py as `strata-gguf-py`.
  Upstream calls that override "kept for two releases, then removed", so this is
  the direction of travel as well as one pin instead of two. `STRATA_GGUF_PY` for
  the prep tools is now `$out/share/strata/third_party/strata-gguf-py`, installed
  exactly where `terminal/tools/_paths.py` looks without being told.
- **The MMQ build option is gone** (0.2.16 removed it; `docs/AMD_HIP_PERFORMANCE.md`
  says so and tells anyone wanting it back to build the `v0.2.15` tag). aibox's
  `-DSTRATA_PREFILL_MMQ=ON` is deleted, so Strata's own prompt kernels are the only
  path there — which is the same direction as the 0.1.61 gfx1030 int8 default, not a
  new axis. `STRATA_EXPERT_I8=0` stays the runtime rollback.
- **Nothing on disk is re-prepared**, checked rather than assumed:
  `expert-profile-coder.bin` `a514661124d5…`, `expert-profile.bin` `8f59b4aa8873…`
  and `draft_vocab.bin` `b1e1d3a7a9e4…` are **byte-identical at both pins**
  (`tar -xzO` of the old rev against the new tree), so the sizing table above and
  the pack under `/home/leigh-admin/Strata` are untouched by the bump.
- **Contracts re-read at the new pin, not trusted.** All 18 flags in `engineArgs`
  are still parsed by `src/program/generate.cpp` — with one trap worth its own
  sentence: **`--pcie-mode` is parsed (`generate.cpp:3989`) but does not appear in
  `--help`**, so `--help | grep` is not a removal test; the removal test is the
  parser. Every `configFile` key (`exe args cwd tokenizer model_name log
  allowed_hosts api_key backend lib_dirs env`) is still read by `terminal/serve/`,
  `serve` is still the thing that imports `strata_control` (`serve/server.py:327`
  onwards, `serve/server_parts/…` is the new split inside `serve/`), and
  `terminal/strata_control/gguf_preflight.py:15` imports `tools.gguf_reader` — which
  is why `terminal/tools` is part of the *serve* layer's copy list now, not just the
  prep tools' directory. The in-build import guard caught nothing this time because
  the copy list was fixed first; it is still the only thing standing between a stale
  list and a crash-loop.
- **What this bump actually changes for sm_86 — one default, and it is not in the
  switch table.** The start block is otherwise the same engine as the one it
  replaced: `sm_86 (Ampere): 8 exact speed switches on by default` with the **same
  eight names** as 0.1.61 (`ADAPT_LAG CPU_IQ_ALL_EXACT1 SIDE_MIXER ATTN_DAILY
  GR_UP75 GU_SWIGLU QSA_ROWS B6_MMVQ_ROWS`), `1533 of the prompt path's 1533
  lendable slots keep their experts in RAM too`, `prompt chunk auto: 8192 tokens`,
  `PCIe 13.4 GB/s`, 2333 experts in the GPU cache, 632 MiB of VRAM free, `strata
  0.1.63`, `--families` still `qwen4exp qwen35moe`. What moved is **route tail
  skip**: rank 7 was the CUDA default since 0.1.42 (+13-16 % decode, measured on an
  RTX 3060 by upstream), 0.1.63 reads an unset value as **off**, and the app/Terminal
  additionally *write* `=0` at every start — though the serve layer this unit uses
  does not, so here it is simply off unless the run config names it. Three arms of
  `strata-exp.sh auto auto` on this pack, cold-to-cold, same prompts:

  | arm | decode median (steady) | prefill-8k | `strata-answer-score.py` |
  | --- | --- | --- | --- |
  | 0.1.61, skip on (the old default) | **45.0** (47.0), spread 43.3-48.9 | 1216.7 | 8/9, `bit` incomplete |
  | 0.1.63, upstream default (off) | **39.3** (40.8), spread 38.1-42.4 | 1202.0 | **9/9** |
  | 0.1.63, `STRATA_ROUTE_TAIL_SKIP=7` | **46.6** (46.7), spread 43.6-46.7 | 1201.4 | **9/9** |

  −13 % decode for the new default, all of it recovered by one `env` line, prefill
  flat, and the answer key does not separate the two 0.1.63 arms — so this box
  **opts out of the new default** (`env.STRATA_ROUTE_TAIL_SKIP = "7"` in
  `machines/services2/strata.nix`, receipts in its comment). Note the skip only
  applies while under 90 % of the experts are cached, and this box's decode hit rate
  through those arms was 58-79 %, so it applies here; on a box that keeps its whole
  expert set resident the two arms would be identical. That the baseline scored 8/9
  and one item *incomplete* is the harness's own noise, not a regression: the item
  (`bit`) passed in both 0.1.63 arms.
- **The harness could not run until it was fixed.** `strata-exp.sh` refuses a bench
  it cannot `test -x`, and four of this directory's benches (`strata-bench.py`,
  `strata-bench-fast.py`, `strata-answer-check.py`, `strata-answer-score.py`) are
  mode **644** in git while aibox's equivalents are 755 — so the documented arm
  invocation died at once with `no such bench script`, having measured nothing, which
  is the failure mode the script's own header says it was written to prevent. They
  are +x now.
- **Build and deploy receipts.** `nix build .#strata-cuda` here: **under 5 min**
  (toolchain warm — see the rewritten "Build it on aibox" above); `nix build
  .#strata` on aibox, from a copy of the tracked tree (`rsync --files-from=` fed by
  `git ls-files`, because the bump was uncommitted): **~5 min, one derivation**.
  `just check-eval` green (assertions, doc pointers, source sizes, alejandra).
  `just switch` → generation 57, `SWITCH_EXIT=0`/`CONFIRM_EXIT=0` in one invocation,
  `readlink -f /run/current-system /nix/var/nix/profiles/system-good` equal, unit
  active with 0 restarts, `/health` 200, ready 59 s after the unit started. Two
  unrelated `gw` commits (`ce1d5aa`, `45270cb` — the v6 ACL and a dnsmasq RA fix)
  rode along, because this checkout was fast-forwarded first, as rule 20 says to do;
  gen 56 is the rollback for both.
- **Not done:** **aibox is still running 0.1.59.1**, so it is now three engine
  releases behind this input rather than one. Its HIP engine **builds** at the new
  pin (verified above, including the MMQ flag's removal and the `terminal/` copy
  list), and the read it owes is a bigger one than last time — gfx1030's int8
  prompt-expert default (0.1.61), MMQ gone (0.2.16), `87c9821bb` "HIP Linux: keep
  the kernel's NUMA balancing off the engine's memory", `8d92f0a38` per-card arch
  choices for the prompt path and the drafters. Open item and the order to do it
  in: `docs/follow-ups.md`.

### Update: engine 0.1.59.1 → **0.1.61** (app 0.2.15), deployed — 2026-11-18

`nix flake update strata`: `13eedf552` (2026-11-12) → `c77572806` (2026-11-16), **787
commits, 2140 files differing**, and the one input moves both engines (the header in
`flake.nix` is why).  **This switch took services2's CUDA engine; aibox's HIP engine
is not deployed yet** — that is deliberate, it is the open item in
`docs/follow-ups.md`, and the reason it matters is the "what is in this bump for the
other box" bullet below.

- **What the bump does to this card, read off the start line rather than the
  changelog**: `journalctl -u strata | grep "exact speed switches"` compared across
  the two starts is the whole diff that matters.  It went from *9 exact speed
  switches on by default* to *8*: **`STRATA_ADAPT_NOWAIT` is no longer set on
  sm_86** (`src/core/nv_defaults.cpp:33`, "repeatable output": with it, a verify
  window ran a swapped-in expert on the GPU or the CPU by its copy's timing, so two
  starts of the same request differed — on an RTX A6000 with a fixed draft table,
  7 of 24 requests identical with it, 24 of 24 in 3 of 3 starts without; its price
  there was decode ×0.988).  `ADAPT_LAG=2` stays, so the copies still get two
  windows to land, and `STRATA_EXPERT_I8`-style rollback applies to the AMD side
  only.  Every other number in the start block is the same engine it replaced: free
  before the cache 4.56 → 4.58 GiB, cache 1770 → 1781 slots (rounding), draft layer
  952 MiB, native Q5_K head 521,472,000 B, PCIe 13.4 GB/s, PLE probe 61,551 →
  61,312 rows/s, `session is up (engine 0.1.61)`.
- **Nothing on disk needs re-preparing**, checked rather than assumed:
  `data/expert-profile-coder.bin` and `data/draft_vocab.bin` are **byte-identical at
  both pins** (compared with `git show <pin>:data/… | sha256sum`), and the only
  change to the preparation tools in range (`237ecb176`) moves them onto
  `strata_gguf`/`strata_quants`, it is not a format change.
- **The negative result, and it is the interesting one.** That default was removed
  to make greedy output repeatable, and here it did not: `strata-answer-check.py 96
  3` against the live unit answers **`stable 0/3`** (fib, sortdoc and arith each gave
  two distinct hashes across three runs — *worse* than the 2026-11-14 record, which
  at least had `arith` stable).  So the retraction in
  `machines/services2/STRATA-TUNE-2026-11-14.md` stands: **the fingerprint gate is
  still unusable on this box and only `strata-answer-score.py`'s answer key may carry
  a before/after claim.**  The reading is that there were at least two causes and
  upstream closed one of them: here the expert tier's *placement* is timing-dependent
  and the engine says at every start that a cached expert is computed on the GPU,
  which "rounds differently from the CPU".  Upstream's own repeatability run fixed the
  expert set (`--adapt-swaps 0`) before it could see the effect; this box cannot fix
  it without giving up the tier it exists for.
- **Speed: neutral, to within the harness's noise**, which is what the start line
  predicts once you know the bump's sm_86 work is tile rules for cards with **80+ SMs**
  (this GA106 has 28).  Same config either side (131072/int8): 8k 1066.0/1064.0 →
  1066.5/1065.2, 28k 1196.1/1193.3 → 1194.9/1190.5, short decode 39.5 → 39.7 tok/s.
- **What is in this bump for the *other* engine, which is why aibox's turn is not
  routine**: `9783e09f9` makes the RX 6000 series (**gfx1030 / 1031 / 1032**) read its
  prompt experts on Strata's own int8 kernels **by default**, for expert groups of
  256 entries and up (~512 tokens and more) — measured on an RX 6700 XT at **1.10 to
  1.35 × MMQ** at 2,048 / 8,192 / 20,000 tokens for IQ2_S and IQ2_XXS gate/up with a
  Q2_0 down, which is precisely aibox's pack and its common case.  Upstream's own
  caveat, quoted so nobody skips it: *"No end-to-end run on a gfx103x card with a
  model has been made (needs the lender's word)."*  So when aibox takes this pin: the
  start line must name the new default, compare **cold-to-cold** as
  `machines/aibox/STRATA.md` insists (its plateau is 225–231 ms per window), and
  `STRATA_EXPERT_I8=0` is the one-line rollback.
- **Retired in this range, not ours**: DeepSeek-V4.1-Flash.  Its engine of its own is
  gone and a DeepSeek config is now refused before anything starts
  (`strata_control/retired.py`).  Nothing on this network runs it; recorded so that a
  refusal on some future box reads as a retirement rather than a breakage.
- **New model surfaces, none of them forced on us.**  A person reading this range
  will meet: `Gyro-S` / `Gyro-M` (agentionai's trellis quants, 27.6 / 35 GB of
  VRAM+RAM together, the n-gram table inside the one file — upstream's own summary is
  "Gyro is not a default, by our measurements"), `Swift 1.5` (a Qwen3.8-Flash-Next
  fine-tune pitched at −63 % thinking tokens, which is a latency claim this fleet
  cares about), the NVFP4 packs (68 GiB of experts, i.e. another box), and
  Qwen3.6-35B-A3B (`qwen35moe`: no pack, no expert profile, no separate MTP layer —
  the family `llm.ai.chrisdell.info` already serves).  **Our pack's entry is
  byte-identical at both pins** (`"IQ1_M"`, family `coder`, `ram_gb 32 / arena_gb
  23.4`), which is the fact that says *no re-download and no re-pack*, and the sizing
  table above is unchanged by the bump for the same reason.
- **How it was built, with the number, because the advice above is not enforced by
  anything.**  `nix build .#strata-cuda` here: **15 derivations, ~30 min, on this box,
  with the engine live on the same 16 cores** — services2 has not had a builder since
  2026-10-17 (`/etc/nix/nix.conf` read `builders = ` then too), and since 2026-11-23 no
  box has: nothing in the tree sets `nix.buildMachines` or `nix.distributedBuilds`, so
  **every build is local on this box, bounded by nothing** (no `nix-daemon` cap is declared
  for services2 or services3 — see `common/nix-gc.nix`'s header for who is capped and why).
  That makes the numbers here the whole cost, and it makes "stop `strata` first" the
  difference between a slow bump and an OOM: ~23 GiB of Coder arena against 30 GiB of RAM
  is not a margin a 15-derivation engine build respects.  `just build-remote` is deleted;
  the idiom it used (`--max-jobs 0`) is now only a way to forbid yourself work.
  The switch itself built 434
  derivations, because this checkout was 27 commits behind `origin/main` (rule 20 —
  fast-forward it first; the extra 419 were that, not the bump).
- **Gates**: `just check` was red on arrival, and not from this change —
  `sops-secret-pg_pass.drv is not valid`, the GC'd build-time-secret class whose
  remedy is the eval loop in `docs/nix-dev-workflow.md` ("After a GC, `nix flake check
  --no-build` fails one build-time secret at a time"); ten seconds of it per machine
  and the check is green, warning-free.  Then `just switch` + `nixos-confirm` chained
  in one invocation, and `readlink -f /run/current-system
  /nix/var/nix/profiles/system-good` equal.
- **The end of the chain is the discovery, not the switch**: `just pi-agent-sync`
  re-read `n_ctx` from `/v1/models` and `pi-agent-status` now reports
  `strata-services2 … ctx=262144`.  No file anywhere was told that number (rule 31);
  the engine is what changed, and the tool noticed.

### Update: engine 0.1.56 → **0.1.59.1** (app 0.2.13), deployed — 2026-11-14

`nix flake update strata` moved the shared input `ffa80d73133ea5c548aefe57889a16e8085a938f`
→ **`13eedf552417a6a791a7d13c6597e24300ee3a09`** (engine **0.1.59.1**: `CMakeLists.txt:13
project(strata VERSION 0.1.59.1 LANGUAGES CXX)`; app **0.2.13**: `apps/desktop/package.json:4`
`"version": "0.2.13"`; upstream's head commit is authored 2026-11-11, pushed 2026-11-12).
**682 commits in twelve days** — the GitHub compare API answers `total_commits` 682 for
`ffa80d73…...13eedf55…`, and since it caps the response's `commits` list at 250 and its
`files` list at 300, the commit subjects were paged from `GET /repos/Niko1221/Strata/commits?sha=…&per_page=100&page=N`
(700 fetched, the old pin at index 682). **The newest release-notes file upstream writes is
still `docs/release-notes-0.2.7.md`**, so four app revisions moved with no notes and the
version line is the CMake `project(… VERSION …)` plus the app's `package.json`, as before.

**Measured on the two input trees** (`diff -rq`; the old tree fetched as a tarball into
`/tmp/oldtree/old`, the new one read out of the store — `/nix/store/8vqfvqhsai202gkilv7mj3a9csaiy3m0-source`, found
by grepping `project(strata VERSION` across `/nix/store/*-source`): **606 files differ, 299
entries appear only in the new tree, and — for the first time in this file's history — 115
entries are *gone***. Every removal is scratch the build does not read: `apps/host/docs/**` (63 files)
and `apps/host/audit-fixes/**` (39), `docs/work/**`, the one-off `tools/dgx_*.py`
probe scripts, `chat.py` and `highlight.py` (moved to `tools/`), `third_party/ggml-tq`, and
`packaging/acceptance_matrix.py`. **Nothing inside `serve/`, `strata_control/`, `strata_cli/`,
`tools/`'s prep scripts or `data/` was removed**, which is what the install phase copies.
Biggest changed groups: `apps/host` (157, not built here), `tests/terminal` (46), `include/strata`
(32), `sycl/src` (31), `src/kernels` (21), `src/split` (19), `src/core` (16), `src/prefill` (11).

**The build system was restructured underneath us, and the package needed no edit.**
`CMakeLists.txt` went **2,950 → 645 lines**, split into `cmake/{strata_geometry,strata_ggml,
strata_ggml_tq_overlay,strata_gpu_engine,hip_backend}.cmake` and `cmake/backends/{cpu,cuda,hip,
sycl,metal}.cmake` (upstream `8b1667701`, `508e6999a` "own-stack phase 1A"). `add_executable(strata
src/program/generate.cpp)` is now at `cmake/strata_gpu_engine.cmake:483` and `strata-device` at `:23`
— the install lines still name the same built artefacts, so they stand. The same commit vendored
the ggml/MMQ sources as **`third_party/strata-ggml`, "frozen at the pin"** — `third_party/strata-ggml/UPSTREAM.md:8`
names `3cf03257f219afbe7334045ff7c6a06ac68c627d`, the revision `strata-package.nix` already fetches —
so `-DSTRATA_GGML_DIR=${llama}` has become an override ("kept for two releases", `CMakeLists.txt:429`)
rather than the only way to feed ggml, and `STRATA_MMQ_TQ_OVERLAY` (ON by default) no longer wants
patched headers: with a `STRATA_GGML_DIR` that has no `ggml/src/ggml-cuda/tq.cuh` the overlay branch is
skipped (`CMakeLists.txt:543`). Upstream's own release scripts still pass the flag.

**The contracts, re-checked against the new tree before deploying** — all pass, none needed a change:

- **All 13 flag pairs in `engineArgs` still parse** in `src/program/generate.cpp` at the new rev
  (`--ple-gguf` 3479, `--ple-io` 3481, `--kv` 3486, `--kv-resident` 3487, `--expert-cache` 3564,
  `--spec-min-p` 3633, `--pcie-mode` 3639, `--conversation-cache-slots` 3644 and
  `--conversation-cache-min-tokens` 3645, `--resident-experts` 3773; `--pack`, `--native`, `--prefill`,
  `--spec`, `--mtp`, `--max-context`, `--expert-profile` and `--conversation-cache-mib` each have their
  `a == "…"` branch in the same block, one hit apiece except `-mib` (two, shared with `-slots`)).
- **`--resident-experts` still means the same plan**, and its block is one line different from
  0.1.56's: `o.resident_pin = strata::core::resident_pin_from_env(getenv("STRATA_RESIDENT_PIN"))`
  replaces the inline `== "0"` test, and `resident_pin_from_env` (`include/strata/core/resident_ram_copy.hpp:13`)
  returns pinned unless the value is exactly `"0"` — same default, same A/B, same 4 GiB headroom.
  `--resident-budget-gib` gained the same helper and nothing else.
- **The `configFile` keys are all still read**, `serve/server.py` still takes `--engine --config
  --host --port`, still serves `/v1/messages` (line 6) and `reasoning_content` (11 occurrences),
  and the Host check is still `if svc.api_key or host_allowed(host, svc.host_names, "*" in
  svc.allowed_hosts)` (`serve/server.py:7788`) — so the `allowed_hosts` / public-name reasoning
  under "Differences from aibox" still describes the code.
- **The install phase's copy list still covers upstream, and the guard exercised it**: the
  transitive first-party import closure resolved with `ast` over `serve/`, `strata_cli/`,
  `strata_control/` is **274 modules, 0 resolving outside the copied set** (new submodules this
  bump: `serve/replicas`, `serve/split`, `serve/responses`, `serve/dsml`, `serve/ds_pictures`,
  `strata_cli/launch/*`, `strata_control/tune_*`). All 115 of the `serve/test_*.py` files upstream
  added are now copied into the package too — dead bytes in the store, no runtime effect. The
  guard's three modules all exist, and **the guard ran on both engines' builds**, which is the only
  way a `serve/` rename gets caught before a model load.
- **The pinned `llama.cpp` is unchanged**: `setup.py:241 LLAMA_CPP_COMMIT =
  "3cf03257f219afbe7334045ff7c6a06ac68c627d"`, `third_party/ggml/VERSION.txt` names the same rev,
  and `strata-package.nix`'s `fetchFromGitHub` is untouched.
- **The data files the unit points at are unchanged**, third bump running: `data/draft_vocab.bin`
  `4135aad364e18d71caec866dc37d34e7`, `data/expert-profile-coder.bin`
  `a7ea5acbdff82c1e73ee023c0e818c6b` — identical between the trees (and `data/expert-profile.bin`,
  aibox's, `ff1bc6544d3f2f0013e3e376632d33a8`, likewise). **No re-prep** of the pack, the MTP
  runtime or the coder profile. `data/` grew 2.3 MB → 3.2 MB, all of it vocabularies,
  `settings-register.json` and `experimental-speed-projection/`; `data/dflash2-tuned/` ships only
  `LICENSE` + `NOTICE.md` (the tuned drafter itself is fetched).
- **IQ1_M is still readable**: `include/strata/artifact/gguf_reader.hpp` is **byte-identical**
  between the trees with `case 29: // IQ1_M` at 223, and `include/strata/kernels/cpu/iq_avx2.hpp:28`
  still names it. The pack prepared 2026-10-22 is still the right shape.
- **`just check-build`-shaped proof, cheap and first**: both engines built from this lock with **no
  change to either `strata-package.nix`** — see the deploy record below.

**One new default was offered to this card and declined by this box's RAM, and a second one moved
its decode.** Both are in the engine's own words in `/var/lib/strata/strata.log`, which is the only
honest way to know, since neither is set by a flag of ours:

| Switch | Gate, as coded | What this box's start actually did |
|---|---|---|
| `STRATA_NV_CHUNK` — a bare `--prefill auto` chunk ceiling **8192 → 32768** on one NVIDIA card of cc ≥ 8.0 (`include/strata/core/nv_chunk.hpp`, `nv_chunk_card_ok(min_major) = min_major >= 8`; the same #282 story as gfx12) | Upstream (A6000, streaming half of UD-Q4_K_XL's experts): 20K prompt **+43.4%**, 32K +22.5%, 64K +21.3%, 8K 0.0%; flat on cards that hold every expert. "changes the rounding slightly on CUDA (first-token KL against 8192 about 1e-4)". Off: `STRATA_NV_CHUNK=0` (or `=16`), or `STRATA_DEFAULTS=026` | **Offered and declined here, by the RAM guard (#2090)**, in the log: `NVIDIA: prompt chunks stay at 16384: 32768 would lend 5 more cache slots (12 MiB more held in RAM during a prompt), and 28147 MiB of RAM is available for 24973 MiB of resident experts plus that (#2090; STRATA_NV_CHUNK=1 takes the default ceiling anyway)` — and the same line again for 8192. The operative chunk is still the small-cache one, `strata serve: prompt chunk auto: 6656 tokens, a 96-slot ring`, identical at both revisions. **This bump therefore does not change prefill here**, and `STRATA_NV_CHUNK=1` would be a RAM decision before a speed one |
| `STRATA_PCIE_K_RULE` / `STRATA_PCIE_SHARE_MAX` — the auto PCIe-share rule (`include/strata/core/pcie_share_rule.hpp`, new file this bump) now uses `k = 1.85` at a pool rate of 30 GB/s falling linearly to **`0.8` at 12 GB/s**, and a ceiling of **0.95** instead of 0.60, on a *discrete* card | Upstream's table: "one card with 4 pool workers 56 → 79 tok/s", "output changes in the last bits". Off: `STRATA_PCIE_K_RULE=0` **and** `STRATA_PCIE_SHARE_MAX=0.60`, or `STRATA_PCIE_FRAC_DEFAULT=old` (start-up rule), or `--pcie-frac N` (pins it) | **This is the one that moved this box's decode.** Same box, same probe (13.4 GB/s), pool ~7 GB/s both times. At 0.1.56 the refinement walked `0.20 → 0.50`, `0.45`, `0.50` (`k=1.85`: 13.4/(13.4+1.85×7.4)=0.50, ceiling 0.60); at 0.1.59.1 it walked `0.20 → **0.70**` (`k=0.8`: 13.4/(13.4+0.8×7.0)=0.705). Verbatim: `strata serve: PCIe share: the CPU pool takes 290 us per missed expert (7.0 GB/s of expert bytes), the link 13.4 GB/s -> pcie_frac 0.70 (was 0.20; --pcie-frac N fixes it, STRATA_PCIE_FRAC_DEFAULT=old keeps the start-up rule)`. Here: decode 29.6 → **30.9** tok/s, one warm arm each — direction, not proof |

**Two more defaults turn on and do nothing here, recorded so the next grep does not mistake them for ours:**
`STRATA_MMQ_KQ=1` (new this bump for **cc ≥ 8.0**, `nv_defaults.cpp:nv_kq_default_env`) — the experts of
*Unsloth K-quant* files only; an IQ1_M pack "never reads the variable", but the engine prints the line
anyway: `strata: a model with K-quant experts (Unsloth UD-Q4/Q5/Q6_K_XL) reads its prompt on llama.cpp's
MMQ kernels (STRATA_MMQ_KQ=0 or STRATA_NV_KQ_DEFAULT=0 turns it off)`. And `serve_stop_bound` is now on
on every backend (upstream `13eedf552`, #4677): it limits which accepted rows enter the parked
conversation — "the tokens a request emits are the same either way", `STRATA_SERVE_STOP_BOUND=0` off.
`sm120_series.hpp` (route prior, prompt-attn IMMA, GR_V3) is **byte-identical between the two input
trees**, so 0.1.56's reading of those three gates stands word for word, and the sm_86 exact-perf table is
unchanged too — the banner in both sessions reads `strata: sm_86 (Ampere): 9 exact speed switches on by
default (STRATA_SM86_PERF=0 turns them off; a switch you set is kept): ADAPT_NOWAIT ADAPT_LAG
CPU_IQ_ALL_EXACT1 SIDE_MIXER ATTN_DAILY GR_UP75 GU_SWIGLU QSA_ROWS B6_MMVQ_ROWS`.

**A negative result, bought with one deploy, that saves the next person the experiment: `STRATA_ADAPT_FETCH`
is refused on any box in the resident-RAM mode.** DETAILS.md prices it at decode **+10.2% (`=2`) / +5.0%
(`=1`)** — "IQ3_S on an RTX PRO 5000 with a 3,896-slot cache (16% of the experts) and `--pcie-frac 0.25`, …
4 rounds, same-arm spread -0.8%", hit rate 54-61% → 71-78% — and this box's cache-to-arena ratio (1,770 of
~12,288) and share (0.37) looked like the same shape, so I put `env.STRATA_ADAPT_FETCH = "2"` in
`configFile`, switched, and read back the engine's verdict: **`strata serve: STRATA_ADAPT_FETCH is off (the
resident RAM mode)`**. `--resident-experts` — the mode this box runs because 22.9 GiB of experts have to
live in RAM — is the disqualifier. Reverted; the line is not in the module, and the final generation is
byte-identical to the first 0.1.59.1 one (`iys76bc0gdfv3flkrsps0i5gyhcsan4f…`). **Do not re-arm it here**;
it would work on a box whose whole arena is resident. And read the same DETAILS row's other disqualifiers
before trying it anywhere in this network — no `--batch`, no `--pipeline-windows`, no `--adapt-async`, no
`--pcie-mode dma|direct`, and no `STRATA_ADAPT_OVERLAP=1` — which is a second, independent reason it cannot
run on aibox either (ours is `--pcie-mode direct`, and the services2 start logs `adaptive overlap: off`, so
the only gate this box failed was the resident one).

**What else this bump brings that is *not* this card, each gate read where it is decided** (so a
`STRATA_*` grep of the new tree does not read as ours): `STRATA_HC_Q8` — the commit says "on CUDA", the
**code** says otherwise: `native_dense.cpp:hc_q8_env()` is on for HIP gfx12 / gfx1151 and `return false`
in the `#else` (CUDA) branch, so off on both of this network's boxes; `STRATA_IQ4_W4A8` / `STRATA_W4A8_*`
(CUDA, but **IQ4_XS / IQ4_NL experts only**, not ours); `--prefill auto`'s 32768 on **gfx12**, the gfx12
staging ring (`STRATA_GFX12_RING`), `STRATA_PF_FUSED_Q50`/`STRATA_PF_DM16`/`STRATA_EXPERT_V2K` (gfx1151
Windows), `STRATA_GDN_CHUNKED=1` by default (Linux, all cards gfx12), `STRATA_VNNI_TIER` (opt-in: the
tier below AVX-512-VBMI rounds differently — "by default every CPU keeps its output"), `STRATA_MMQ_TAB`
(OFF: costs 36-38 MiB of VRAM per GPU), `strata-split` (`STRATA_BUILD_SPLIT`, OFF), `src/dsv41`
(`STRATA_BUILD_DSV41`, OFF), `STRATA_SPEC_ADAPT_SM86` (still opt-in — the owner's 3060 A/B is still
length-confounded, unchanged text in `apply_spec_adapt_default`), and the sm_121 / GB10 table. The bump's
other new instrument, `gpu_pause_time.hpp` (the KFD `evicted_ms` driver-pause counter), is **HIP-only in
the code** — "`generate.cpp` calls it only in HIP builds" — so it is aibox's, not this card's, and
`machines/aibox/STRATA.md` is where it is recorded. `--lookup-max-t`
is **not** new here: the default is 12 on an 8-row CUDA build at both revisions and both sessions print
`strata verify: window up to 12 tokens, 109.5 MiB of device buffers`.

**The box, not the engine: this bump's start saw less free RAM than the last one, and said so twice.**
The 0.1.56 start logged `18.78 GiB really free for a 22.84 GiB page-locked complement` → `resident 22.84
GiB, pinned 22.84 GiB … page-locked and mapped` and `1473 of the prompt path's 1473 lendable slots keep
their experts in RAM too`. The first 0.1.59.1 start logged `16.69 GiB really free for a 22.73 GiB
complement`, then two warnings this box had never printed:

```
FileExpertSource: WARNING: only 22.00 of 22.73 GiB could be page-locked (the RAM left would fall under
  the reserve); the rest is resident but pageable, so its copies to the GPU go through the CPU (#1250)
FileExpertSource: WARNING: 62 of 1473 lendable slots (4%) will read their experts from the pack during a
  long prompt; expect a slower prompt read on 50K+ token prompts (one measurement: 18% uncovered cost
  26%). Free RAM or lower STRATA_RESIDENT_HEADROOM_GIB (now the RAM left free at start) and restart;
  the line above should read N of N.
```

That is six days of Prometheus on a box sized to 28 of its 30 GiB, not a code change — but the two
lines are now the thing to read at the top of every start, and the same day's *next* start (after the
page cache had been reclaimed) got `resident 22.85 GiB, pinned 22.85 GiB in 45 s; 22.85 GiB page-locked
after taking the pages in steps` and `1473 of 1473`. The engine's new "page-locking it in steps, as far
as the RAM allows" path recovers what the plain allocation refused; the sizing table above is still the
reason it is close, and `--conversation-cache-mib 0` is still what keeps the floor.

**Optimisations now available on this box, in the order I would try them.** All are `configFile.env`
lines, so each costs a switch and a ~90 s reload, not a rebuild — and to correct two sentences in the
section below: **there is no `extraEnv` option in this module**; these dials go in `configFile.env` in
`machines/services2/strata.nix`, which is a rebuild-and-switch and takes the engine down while it reloads.

1. **Nothing.** The one default that would have sped up prompts here (32768 chunks) is declined by the
   RAM guard, and the one that moved decode did so by itself (PCIe share 0.50 → 0.70). If a future
   answer-quality complaint traces to the share, the revert is `STRATA_PCIE_K_RULE=0` +
   `STRATA_PCIE_SHARE_MAX=0.60`, not a different model.
2. **`STRATA_EMB_REUSE_ACCOUNT=1`** (`src/prefill/prefill.cpp:413`, opt-in, CUDA): lets the chunk planner
   count the VRAM freed by the half-output buffer aliasing the embedding buffer, "Prompt about **+3.9%**
   on an RTX 3060, no KL measurement yet" — the only upstream figure in the bump measured on this card
   class. It spends VRAM to earn prompt, and the card has 630 MiB free with everything loaded, so the
   honest expectation is "does nothing". Not armed.
3. **`STRATA_SELECT_TWO_PASS=1`** (`include/strata/kernels/qsa_two_pass.hpp`, opt-in, sm_80+, prompt path
   only): score every block once in single-pass TF32 with an error bound and re-score in 3xTF32 only where
   the bound cannot settle the query's top-k cut, so **the selected cells are the 3xTF32 scorer's** — the
   rare kind of prompt win that is not a rounding trade. Upstream RTX 3090: 256K **+5.2%**, 128K +2.2%,
   32K unchanged; only calls whose last query has ≥ `STRATA_SELECT_TWO_PASS_MIN` (default 4096) blocks take
   it. This box's real prompts top out near 70K tokens, so expect the 2%-or-none end. `STRATA_SELECT_TWO_PASS_CHECK=1`
   is the (slow) arm that proves the ids match.
4. **Not on this card, at all**: `STRATA_ADAPT_FETCH` (refused above), `--draft dflash2` (needs
   `--draft-model` plus ~1.2 GiB of VRAM and `--vram-reserve-mib 1900`; aibox is where the room is, and its
   tuned drafter `data/dflash2-tuned/` is fetched, not shipped), `STRATA_MMQ_TAB`/`STRATA_MMQ_DIRECT=1`
   (36-38 MiB of the VRAM the arena wants), `STRATA_IQ4_W4A8` (not our quant), and `STRATA_ROUTE_PRIOR`
   (0 by default at 0.1.56 and still 0: `sm120_series.hpp` did not move).

**Measurements, before and after, through the front door** (`https://strata.int.leighhack.org`, which
injects the key, so the bench holds no secret). New in the repo with this bump:
**`machines/services2/strata-tune/strata-bench.py`** — the CUDA side of the ritual now has a script, which
the 2026-11-04 record had to note was missing. It asks in *fresh-variant* prompts (a random marker line per
arm): this box runs `--conversation-cache-mib 0`, so a repeat would be re-read here, but the same script
crossed to aibox — which does not — and answered a repeat with `prompt_n=5`. Do not let a bench depend on
which cache dial the box under test has. It measures 8k **and** 28k because those are the two chunk regimes,
and it prints `prompt_n` first for exactly that reason.

| arm | 0.1.56 (baseline, warm) | 0.1.59.1 (warm) |
|---|---|---|
| short-decode, 200 tokens | **29.6** tok/s, draft 106/156 | **30.9** tok/s, draft 103/157 |
| prefill 8,089 tok ×2 | 1,058.6 / 1,053.2 tok/s | 1,037.2 / 1,040.6 tok/s |
| prefill 28,084 tok ×2 | 1,140.3 / 1,137.4 tok/s | 1,135.6 / 1,132.9 tok/s |

Decode +4%, prompt −0.4 to −1.5% — i.e. one warm arm each inside a spread this box has always had, with
the PCIe-share change the only mechanism that moved. **Method, because the first reading was wrong by
30%:** the pass run *immediately* after a load read 21.2 tok/s decode and 952 tok/s prompt — the engine had
yet to capture its per-window CUDA graphs (`strata verify: capturing the 1..12-token window`) and its page
cache had just been released (`the model files' cached pages were released`). Only the second pass is
comparable, and the baseline was measured the same way (one cold pass discarded, one warm pass kept).

**Corrected 2026-11-14, later the same day:** a 23-arm sweep ran against this same binary and pack
(`STRATA-TUNE-2026-11-14.md`), and four *control* repeats of its own harness bracket the noise at
**31.2 / 31.9 / 32.0 / 32.6 tok/s** decode and **1,048-1,056 tok/s** of 8k prompt, with one control pass
at 807. So the "+4 %" above is inside the box's own spread and is *not* a demonstrated gain — and neither
is any single-digit claim from here on. The sweep's value is what it ruled out: the card is at maximum
clock (1,900-1,950 MHz) in both phases, spends ~1.25 s of ~176 s in `SW Power Capping`, and reaches its
full 26 TFLOP/s bf16 under cuBLAS at only 95 W, so the 157 W of prefill is a bill for dequantising and
moving IQ1_M, not a ceiling. Cache size, `--pcie-frac`, `--spec`, `--spec-min-p`, `--aux-cpus`,
`--pipeline-windows`, `STRATA_PREFILL_CPU_SHARE=0`, the resident-headroom and prompt-path envs: all
measured, all flat or worse.

**Deploy record, 2026-11-14 (what this bump cost, and the fleet's share of it)**

- Built on aibox at `ebf1491` (the lock commit alone; the engines needed no source change, so the lock is
  all aibox had to move). `systemctl stop strata` first — 57 GiB available then, ~7-10 GiB with the engine
  loaded. One detached unit (`systemd-run --no-block --unit=aibox-strata-build`, `--uid=leigh-admin`,
  log `/var/tmp/aibox-strata-build-20261114.log`) built **both** engines: exit 0, **02:29:47 → 02:39:54 =
  10 min 7 s wall, 26 derivations** — the CUDA 13.0 set again (aibox's GC had collected it since 11-04,
  so each box pays it after a quiet fortnight) plus the two engines, which compile in parallel. HIP:
  `/nix/store/97dahyfvskvdgq559bbk3d63cs62402r-strata-unstable`; CUDA:
  `/nix/store/h40ij73r2r34wgagk1hs6a6zr55yyac8-strata-unstable` — distinguished without running either:
  the HIP one's `bin/` installs `strata-alias-check`, the CUDA one's installs `libstrata_kq.so`. Both
  packages install everything into `bin/` and ship no `lib/`, and `libstrata_kq.so` is **not** new to
  this bump: all eight `-strata-unstable` paths still in this box's store (all CUDA, the only kind this
  box holds) carry it.
- `sudo nix copy --no-check-sigs --from 'ssh://leigh-admin@10.3.1.32?ssh-key=…'` of the CUDA out path took
  **1.4 s**, not minutes: the engine's own output is **138 MB** and every other path of its 1.5 GiB closure
  was already in services2's store. `nix path-info -S` prints the closure, which is where the 1.5 GiB figure
  this file quotes for the build's *output* comes from — it was never what crosses the wire.
- `just switch` here, chained confirm, `readlink -f` equal. Three switches in total (the bump, the
  `STRATA_ADAPT_FETCH` arm, the revert); the last generation equals the first bump generation by store
  path, which is the proof the revert is complete. No failed units; `/health` `loaded: true`,
  `api_key: true`, `max_context: 131072`.
- State after the switch, for the next person to compare against: cgroup `MemoryCurrent` 29,403,131,904 B
  (**27.4 GiB**), `free` 3 GiB available, swap 1.3 GiB used, `nvidia-smi` 11,279 / 12,288 MiB, and the
  engine's own `632 MiB of VRAM free with everything loaded`, `expert cache auto: 4.56 GiB free, 700 MiB
  reserved (+220 MiB for the draft head, +256 MiB for what is allocated after the cache) -> 1375 slots`
  → `expert cache 1770 slots, 3.41 GiB of VRAM` (0.1.56's start: 1376 → 1772).
- **What gated it**: `just check-eval` — the four assertion evals, `just check-source-size`, alejandra —
  passes at this lock, and aibox's `system.build.toplevel.drvPath` evaluates clean. `just check`'s flake
  step still dies exactly where the 2026-11-04 record says it died:
  `error: path 'w1qfxxfc0wwjzfqivfnqfqjf60dily0h-sops-secret-pg_pass.drv' is not valid`, in a trace whose
  last frame is `machines/services1/services/outline.nix` → `services.outline.databaseUrl` — an eval-time
  secret read on a machine this bump does not touch. `nix build` of that `.drv` answers *"don't know how to
  build these paths"*: it is not merely unrealised, it is not in this store at all, so the follow-up's
  "went green by being built" fix is a services1-side evaluation, not something this box can do.
- **aibox took the same input the same day** — its HIP/gfx1030 side, the control measurement against the
  plateau band, the new AMD `evicted_ms` driver-pause counter, and the fact that an aibox switch needs the
  netboot umount dance before it and not after: `machines/aibox/STRATA.md`, "Update: engine 0.1.59.1
  (app 0.2.13), deployed — 2026-11-14".

### Update: engine 0.1.56 (app 0.2.10) — 2026-11-04

`nix flake update strata` moved the input `5e8a4cca7d690ac60ca37eb09e7d0e010099ad6d`
→ **`ffa80d73133ea5c548aefe57889a16e8085a938f`** (engine **0.1.56**: `CMakeLists.txt:11
project(strata VERSION 0.1.56 LANGUAGES CXX)`; app **0.2.10**: `apps/desktop/package.json:4`
`"version": "0.2.10"`). **1,074 commits in six days** (`gh api repos/Niko1221/Strata/compare/5e8a4cca7…...ffa80d731…`:
`total_commits` 1,074, base 2026-10-27, head 2026-11-02; GitHub's `files` list caps at
300, so it is a floor, not a count) — **four app revisions in one bump** (there is no
`release-notes-0.2.8/9/10` file yet; the newest notes file in the tree is still
`docs/release-notes-0.2.7.md`, so upstream writes the notes late and the release line is
the CMake `project(… VERSION …)` plus the app's `package.json`). **Measured on the two
input trees** (`diff -rq` between the store source the deployed engine was built from and
the new tree): **569 files differ, 408 entries appear only in the new tree (two of them
directories), 0 entries are removed anywhere** — the biggest groups are `apps/` (the
desktop app, not built here), `tests/terminal`, `sycl/`, `src/kernels/cuda`,
`strata_control`, `serve`, `src/core`, `tools/`, and `src/dsv41` (the DeepSeek-V4.1
engine, compiled only under `STRATA_BUILD_DSV41`, `CMakeLists.txt:934`, **OFF by
default**, and we do not pass it).

**One default that applies to this card changed, and it changed *off*:**

| Switch | Gate, as coded at the new rev | What upstream measured / decided |
|---|---|---|
| `STRATA_ROUTE_PRIOR` — **now OFF on every card.** It was **on** here at the previous pin (`sm120_series.hpp:78` used to answer `kRoutePriorSmallCard = 1.5f` for one discrete NVIDIA card ≤13 GiB — this 12 GB RTX 3060 is that card class) | `sm120_series.hpp:78` `route_prior_default(…) { return 0.0f; }` for every card; the header keeps `kRoutePriorSmallCard` (line 73) and says "**OFF by default again**… **It stays an opt-in**" | Their owner decision of 2026-10-29, issue **#4402**: repetition loops and weaker results in long agent sessions; their 0.2.7 agent check passed **4.5 of 10 with it on against 5.6 of 10 with it off**. The speed it bought on that same card was story +15% / **code +37%** (`docs/release-notes-0.2.7.md`), i.e. this reversal gives that back and takes the reliability instead — which is the direction wanted *here*, since this box is the one pi answers from (`PI_PROVIDER=strata-services2`) |
| `STRATA_PROMPT_ATTN_IMMA` — **unchanged, still on** for sm_86 | `sm120_series.hpp:96` `prompt_attn_imma_default(…) { return !v026 && !hip && cc >= 80; }`, same as the previous pin | +1.1 to +1.8% prompt; **another summation order**, so answers move at a near-tie |

To pay the route prior back, the dial is `STRATA_ROUTE_PRIOR = "1.5"` in the unit's
`extraEnv` — no rebuild, and answers move (upstream's own words: "any set value is the
user's own and wins on every card", `sm120_series.hpp:82 route_prior_resolve`). Nobody
has re-run either number on **this** pack (IQ1_M, 256 of 512 experts) at either revision;
the figures above are upstream's, on their quant, and this box's own are the switch's
journal.

**What is new at 0.1.56 but does *not* apply here**, so a later `STRATA_*` grep does not
mistake it for ours, each gate read where it is decided: the **sm_121 (GB10 / DGX Spark)
perf table** (`nv_defaults.cpp:53`, `mj == 12 && mn == 1` — ours is `mj == 8 && mn == 6`);
the **sm_86 exact-perf table is byte-identical** between the two input trees
(`nv_defaults.cpp:26-40`, the same nine rows, same `STRATA_SM86_PERF` master switch), so
what this box prints as "exact speed switches" does not move in the bump; `STRATA_GR_V3`
still answers only for a **sm_120-class card above 13 GiB** (`sm120_series.hpp:89`);
`STRATA_MMVQ_IL_89` is still `cc == 89` only; the **AMD and gfx12 work — new in
this bump: `include/strata/core/amd_community.hpp` and `include/strata/core/gfx12_extra.hpp`
(and their two tests) appear only in the new tree** (aibox's side, not this one); `--draft dflash2` (the DFlash2 block drafter
for Qwen3.8-Flash-Next, **opt-in**: `--draft` is unset here and its usage line
`generate.cpp:1157` says "the draft source (**default mtp**")", so `--mtp DIR` below still
selects the MTP drafter, and DFlash2 would additionally need `--draft-model PACK`
built by `tools/dflash2_pack.py`); `--resident-budget-gib N` and `--ram-keep-gib W` (both
new, both inert at their defaults — `ram_keep_from_gib(0) = 0` and the fallback predicate
`arena_ram_fallback_allowed(…)` answers *allowed* when its variable is unset, which is the
old plan plus one extra pass, below); `strata-split` (`STRATA_BUILD_SPLIT`,
`CMakeLists.txt:2947`, **OFF**).

**The new memory mechanism is the one code path this box's start now takes differently.**
`include/strata/core/ple_residency.hpp` grew a RAM verdict for the expert arena —
`:58 arena_ram_verdict` (`Load` / `FallBack` / `Warn` / `Refuse`), `:68
arena_ram_fallback_allowed`, `:109 fallback_headroom` — and `generate.cpp` now runs it as
two passes at most: **load the arena, and if the RAM is still short after the wait and the
give-back, read the experts from the file for that run instead** and say so
(`strata generate: experts via mmap (not enough RAM for the expert arena: the same source
as --mmap-experts; …)`). Its own reason is upstream's **#4846 / #4051**: an arena loaded
into an already-full host pushed a 60 GiB machine into reclaim and it had to be power-cycled;
the engine still starts, never refuses. A cgroup limit below the arena is the only
`Refuse`. `STRATA_ARENA_RAM_FALLBACK = "0"` is the previous behaviour, and
`STRATA_RAM_KEEP_GIB` / `--ram-keep-gib` trade RAM the owner wants left free for the
arena's presence. `:43 ple_lock_blocks_cache` (#4518) is the other half: the PLE table's
*mlock* is now given back when giving it back makes the automatic conversation cache
bigger — inert here, because `--conversation-cache-mib 0` is pinned (0.1.52's note above
is why) and an explicit 0 is never resized. This box is where the difference is visible:
it has always been the short-of-RAM case, and the journal now carries the verdict line.

**The two cmake dials that decide what a CUDA build of 0.1.56 *is*, and neither is ours:**
`STRATA_WINDOW_MAX_T` defaults to `auto` = **8 rows for CUDA** and 16 for HIP
(`CMakeLists.txt`, and its comment gives the reason: a CUDA engine loads every kernel of
its binary at start, so the wide instantiations "would take VRAM from the expert cache of
every card") — same window as before, so no flag is needed and nothing moved; and
`STRATA_AMD_COMMUNITY_TUNING` (**ON by default**, `CMakeLists.txt:98`), which compiles the
community-made **gfx1010 SDWA dot product** into *this* CUDA build as well (only
`-DSTRATA_AMD_COMMUNITY_TUNING=OFF` puts the portable signed-byte dot back) — that switch's
*runtime* half, `STRATA_AMD_COMMUNITY_TUNING=0`, is aibox's gfx1030 rocBLAS tables.

**The contracts, re-checked against the new tree before deploying** (the two named in the
ritual above, both of which have moved once, plus the four the install phase assumes):

- **All 13 flag pairs in `engineArgs` still parse** at 0.1.56 in `src/program/generate.cpp`:
  `--pack` 3302, `--max-context` 3322, `--ple-gguf` 3350, `--kv` 3357, `--native` 3377,
  `--expert-cache` 3435, `--prefill` 3451, `--spec` 3485, `--mtp` 3500, `--spec-min-p` 3507,
  `--conversation-cache-mib` 3518 (now one branch with `--conversation-cache-slots`),
  `--expert-profile` 3639, `--resident-experts` 3647 — and `--resident-experts`'s own
  block is **byte-identical** to 0.1.52's (`mmap_experts = resident_cpu_experts =
  resident_pin = resident_soft = true`, `resident_headroom = 4 GiB`, same two A/B
  variables), so the low-RAM mode this box runs means the same thing it meant before.
- **The `configFile` keys are all still read** by `serve/server.py`, which still takes
  `--engine` / `--config` / `--host` / `--port` (its argparse, lines 9504-9520).
- **The API pi dials is still there**: `POST /v1/messages` (the Anthropic dialect,
  `serve/server.py:6` endpoint list, 7828 handles the `?beta=true` form) and
  `reasoning_content` (11 occurrences) are both still served, and `/health` still
  answers `loaded` / `engine` / `cache_max_tokens`.
- **The install phase's copy list still covers upstream**, and not by directory luck:
  the transitive first-party import closure of `serve/server.py` — resolved against the
  new tree with `ast` — is **15 files, all inside `serve/`, `strata_cli/` and
  `strata_control/`**, and **no first-party name resolves outside the copied set**,
  including the new `serve/webfiles`, `serve/mcp`, `serve/prometheus` and the
  `strata_control` rows (`tls`, `vram_fit`, `vision_reader`, `gpu_roles`) that 0.2.8-0.2.10
  added. The build-time guard's three modules (`serve/server.py`,
  `strata_control/procs.py`, `strata_cli/client.py`) all exist, so a rename in the next
  bump still fails the build in seconds rather than the model load.
- **The installed binaries still exist**: `add_executable(strata src/program/generate.cpp)`
  at `CMakeLists.txt:893` and `add_executable(strata-device …)` at 446.
- **The pinned `llama.cpp` is unchanged**: `setup.py:237 LLAMA_CPP_COMMIT =
  "3cf03257f219afbe7334045ff7c6a06ac68c627d"`, and `third_party/ggml/VERSION.txt` in the
  new tree carries the same hash — so the vendored ggml half of the build is byte-for-byte
  what it was at the previous pin, and `strata-package.nix:44` needs no move.
- **IQ1_M is still readable at runtime**, which is what the hand-prepared pack of 2026-10-22
  depends on: `include/strata/artifact/gguf_reader.hpp:223` still has `case 29: // IQ1_M`,
  `include/strata/kernels/cpu/iq_avx2.hpp:28` still names it, and `setup.py:415` still says
  "the Coder's IQ1_M shards … same source and form". What moved is the *native pack build*
  for **DeepSeek-V4.1** (`IQ1_M` → `IQ1_T_M`, `IQ3_XXS` → `IQ3_S` as the native class), not
  the Coder's read path — re-pack only if a later rev drops the read path, not on this one.
- **The data files the unit points at are unchanged**: `data/draft_vocab.bin` md5
  `4135aad364e18d71caec866dc37d34e7` and `data/expert-profile-coder.bin` md5
  `a7ea5acbdff82c1e73ee023c0e818c6b`, **equal between the two input trees** (and equal to
  the values recorded at the two earlier bumps), and `diff -rq data/` reports **three
  entries, all inside the DeepSeek-V4.1's own `data/dsv41`** (`coder-set.json`,
  `profile-coder.bin`, `profile-coder.bin.sha256` — not ours). So the pack, the MTP runtime
  and the coder profile still mean the same thing.

**The same input move dirties aibox's engine too** — `machines/aibox/strata-package.nix`
takes this input, so its HIP/gfx1030 engine recompiles on the next switch there, and
0.2.8-0.2.10 is where the **AMD** work landed (the gfx1010 SDWA dot above, the gfx12
Wave32 decode switches, HIP's window of 16). Read `machines/aibox/STRATA.md` before that
switch; the CUDA engine this box needs is built on aibox either way, so the two jobs share
the checkout.

**Landed on aibox the same day, 2026-11-04.** The HIP side of this bump — what turned on for
its gfx1030 (the community-tuning log line, the simt2 QSA selector default, the 8→16
compile-time verify window that costs nothing at `--spec 4`), the control measurement against
the plateau band, and the deploy record including the fact that a plain `just switch` there
fails on the busy netboot bind mount and rolls the box back — are in
`machines/aibox/STRATA.md`, "Update: engine 0.1.52 → **0.1.56** (the flake input), deployed
— 2026-11-04".

What gated it, and what the switch showed: see the paragraph below the deploy — the
four assertion evals, `just fmt-check` and `just check-source-size` pass, and `nix flake
check --no-build` at this lock still dies with `sops-secret-pg_pass.drv is not valid`
(the eval-time secret read, verified against a clean HEAD, so it is not this change).

`nix flake update strata` moved the input `7ddc17a` (0.1.48.1 / app 0.2.6) →
**`5e8a4cca7d690ac60ca37eb09e7d0e010099ad6d`** (Control and every engine package
**0.1.52**, app **0.2.7**: `docs/release-notes-0.2.7.md:3` "Control and every
engine package (Qwen and DeepSeek) are 0.1.52, because both engines changed"; the
app's own marker is `apps/desktop/package.json:4` `"version": "0.2.7"`). **813
commits in four days** (`gh api repos/Niko1221/Strata/compare/7ddc17a3…...5e8a4cca7…`:
`total_commits` 813, base 2026-10-23, head 2026-10-27; GitHub's `files` list caps at
300, so it is a floor, not a count). **This is the current lock**, and unlike the
bump above it is *not* a hotfix release: 0.2.7 rewrote the Qwen engine and the
DeepSeek engine, added a third engine (`src/dsv41`, DeepSeek-V4.1 from Project
Maya), and grew the app layer. **Measured on the two input trees** (`diff -rq` old-vs-new:
**384 files differ, 248 new files appear, 0 files are removed anywhere**) — the biggest
new groups are `serve/web` (+9 files, the web chat page) and `strata_control` (the
speed-switch table and the app's own settings), and the CUDA engine's own sources moved
far less than that number suggests, but they moved, which is why this bump rebuilds the
engine at all (the 2026-10-23 bump did not: 38 s, engine byte-identical) and at **8 min on
aibox** (`systemd-run` unit `strata-engine`, 16:06:11 → 16:14:06, 25 derivations: the CUDA
13.0 package set re-created around the new engine plus the engine itself) — the ~7 min
figure in the ritual above was already the right order of magnitude. The app layer is not built by
anything here (`strata-package.nix` compiles the CUDA engine and the install phase's
Python layer only), so the app and the Terminal travel into the store and run on nothing.

**Two of 0.2.7's new defaults apply to this very card class, and both change answers.**
Not adopted by a flag of ours — they turn on because the *rules* say so. The rules
are pure functions in `include/strata/core/sm120_series.hpp` (gates read there),
upstream's words in `docs/release-notes-0.2.7.md` and the switch table in
`docs/DETAILS.md:1042`:

| New default | Gate, as coded | What upstream measured |
|---|---|---|
| `STRATA_ROUTE_PRIOR=1.5` (`kRoutePriorSmallCard`) | **one discrete NVIDIA card ≤13 GiB** — ours is a 12 GB RTX 3060, so it is **on**. Larger NVIDIA, Radeon and Strix Halo are not changed | On an **RTX 3060 12 GB** (Qwen3.8-Flash-Next **IQ3_XXS**, five paired runs against the same build with it off): story 46.2 → 53.1 tok/s (+15%), **code 45.7 → 62.5 tok/s (+37%)**. It keeps the experts already on the card, so it page-ins less from RAM — and **picks a slightly different set of experts, so the wording of answers changes**. Quality was checked on that card with 80 tasks, no copy or code defects at 1.5 (a 2.0 setting *did* show defects, which is why the default is 1.5). **Agent loops may finish slightly less often: 4.5/10 at 1.5 over 4 runs against 5.6/7 with it off, and the off runs alone ranged 4–6** — inside the run-to-run spread |
| `STRATA_PROMPT_ATTN_IMMA` | **every CUDA card of cc ≥ 80** — sm_86 is 86, so it is **on** (off for Turing/Volta/Pascal) | +1.1% to +1.8% prompt at 8K/16K on an RTX PRO 5000 and 2× RTX A4000; `=0` puts the 0.2.6 kernel back. **Another summation order**, so answers move at a near-tie |

**Read the agent-loop column before calling the first one a free win.** Upstream
measured both halves of that sentence on this card class: +37% on code, and fewer
agent loops finishing — and this box *is* the one pi answers from
(`PI_PROVIDER=strata-services2`). Their sample is 10 tasks over 4 runs on **IQ3_XXS**,
ours is IQ1_M with 256 of 512 experts, so neither number transfers; but the *speed*
win and the *task-completion* loss came from the same card, so if agent turns start
failing more often, the dial is `STRATA_ROUTE_PRIOR=0` (in `extraEnv`, no rebuild —
it costs the speed back), not a different flag. Nobody has run either number on this
pack yet; the journal's own line is what says what resolved (below).

**What 0.2.7 turns on that does *not* apply here**, so a later `STRATA_*` grep does
not mistake these for ours, each gate read in the file that decides it: `STRATA_GR_V3`
(stream-split hyper-connection read, another summation order — `sm120_series.hpp:70`
`gr_v3_default`, **sm_120-class cards above 13 GiB**, and measured *worse* on sm_86 in
its own A/B: −3.0%/−5.9% on 2× A4000 and +1.0%/−3.4% on an RTX 3060, "so not those");
`STRATA_GR_UP_WIDE` (bitwise the old kernel, **sm_120-class only**); `STRATA_MMVQ_IL_89`
(#3453's decode row tables, **`cc == 89` only** — an Ada card, not this one, and not yet
measured on Ada by upstream); the two **sm_80-exactly** defaults of #3779 (`STRATA_DEQUANT_WIDE`
and `STRATA_MMVQ_IL_SM80`, both bitwise the old output, gated `major==8 && minor==0` at
`src/kernels/cuda/dequant_bf16.cu:301` and `cc[dev] == 80` at `native_mmvq.cu:3813` —
sm_86 is 86, so they are off here, and measured only on a CMP 170HX); the gfx12 decode
switches of the arch table; `STRATA_PLE_AUTO_RAM` (**the PLE-table residency policy is on
by default as of 0.2.7**, `ple_residency.hpp:32`, and `src/program/generate.cpp:4674` only
admits the 28.8 GB table through `ple_resident_admit` = `conversation_memory_admit(available,
table + arena, 8 GiB)` — ours is 29.97 GiB of RAM with a ~23.4 GiB IQ1_M arena to fill, so
it declines and keeps `--ple-io direct`. Upstream's own reason for the policy is one line
above that block: on the 64 GB RTX 5070 PC the table + 8 GiB headroom *did* admit it, the
table's lock took the lockable memory and the expert load then died with **error 1453
(ERROR_WORKING_SET_QUOTA)** — this box is 29.97 GiB, further from the bar, but if that ever
changes the engine prints its numbers (`--ple-io ram chosen by the residency policy`,
`generate.cpp:4758`), which is the journal line to look for); every DeepSeek (DS4) change;
the Metal and SYCL ports; and the Radeon / Strix Halo / gfx12 work. The **sm_86 exact-perf table** (`nv_defaults.hpp`,
`STRATA_SM86_PERF`, the "4 exact speed switches on by default" line this box prints today)
is **byte-identical between the two input trees** — that table is not new in this bump.
`STRATA_SPEC_ADAPT_SM86` stays opt-in (upstream's own 3060 A/B was length-confounded).

**`docs/DETAILS.md` carries 0.2.7's measurements in tables, and it says which card each
figure was taken on** — "We have no sm_80 card and did not measure them" on the #3779 rows,
"not yet measured on an Ada card by us" on `STRATA_MMVQ_IL_89` (`docs/DETAILS.md:1215`).
Read those lines, not a summary, before turning on anything measured on a *different* card:
the figures above are upstream's RTX 3060 rows, on their quant, and nobody has run this
pack's numbers.

**The one-step rollback for all of it, with no rebuild: `STRATA_DEFAULTS=026`**
(`include/strata/core/defaults_026.hpp`, and the notes' "Qwen: 0.2.6 behaviour" row
in the app). It puts back every Qwen default 0.2.7 changed — route prior, the wide
hyper-connection read, the stream-split GR read, int8 prompt attention, the four
gfx12 decode switches, the PLE residency policy, the gfx12 miss split — and a
variable *we* set still wins over it. `DS4_DEFAULTS=026` is the DeepSeek engine's
half (not ours). **KVA (`STRATA_KVA=1`) is off by default in 0.2.7 and stays off**:
it reads the last 12 layers of a long prompt approximately, and upstream's own
numbers were up to **−28% loss rate on agent prompts at 2048 tokens** — off, and to
stay off here.

**The contracts, re-checked against the new tree before deploying** (the two named
above, both of which have moved once before, plus the two the install phase assumes):

- **All 13 flag *pairs* in `engineArgs` still parse** at 0.1.52 in `src/program/generate.cpp`
  (`a == "--pack"`, `"--native"`, `"--ple-gguf"`, `"--expert-profile"`, `"--expert-cache"`,
  `"--prefill"`, `"--spec"`, `"--spec-min-p"`, `"--mtp"`, `"--max-context"`, `"--kv"`,
  `"--conversation-cache-mib"`, `"--resident-experts"` — every one present; `--resident-experts`
  is passed in `machines/aibox/strata.nix`, not in services2's, which is where its low-RAM
  block is). **`--spec 4 --spec-min-p 0.5` is still the default depth** at 0.2.7
  (`docs/DETAILS.md:1215` "`--spec 4 --spec-min-p 0.5` stays the default depth") — which is
  exactly what `strata.nix` passes, so our `--spec` line is upstream's own current default,
  not a pin.
- **All `configFile` keys are still read** by `serve/server.py` (`exe args cwd tokenizer
  model_name log allowed_hosts api_key backend lib_dirs env`).
- **The install phase's copy list still covers upstream**: `serve/`, `strata_control`,
  `strata_cli`, `tools/`, `data/` all exist as directories, and the build-time guard's
  three modules (`serve/server.py`, `strata_control/procs.py`, `strata_cli/client.py`)
  all exist — the guard is what would have failed the build if a rename had come again
  (0.1.46's rename is the finding above).
- **The install phase's binaries still exist**: `add_executable(strata …)` and
  `add_executable(strata-device …)` are both still in `CMakeLists.txt`. The new
  `STRATA_BUILD_DSV41`, **OFF by default** ("OFF by default: it changes no existing
  target"), and we do not pass it — so the CUDA build does not start compiling `src/dsv41/*.cu`.
  `add_executable(strata-gguf …)` and friends are built but not installed by us.
- **The pinned `llama.cpp` is unchanged**: `setup.py:222 LLAMA_CPP_COMMIT = "3cf03257f219afbe7334045ff7c6a06ac68c627d"` at the new rev, same as the rev `strata-package.nix:44` pins and same as `third_party/ggml/VERSION.txt` in the new tree — so the **vendored ggml** side of this build is byte-for-byte what 0.1.46 built. **What is *not* unchanged is the engine's own sources** (384 files differ, above), so the engine derivation is dirtied and recompiles: this bump's engine build on aibox measured **8 min** (25 derivations, the CUDA 13.0 set included) where the 2026-10-23 bump (engine byte-identical, app-only change) took 38 s. The same is true of the *other* box: `machines/aibox/strata-package.nix` takes the same input, so aibox's HIP/gfx1030 engine recompiles on the next switch there too — read `machines/aibox/STRATA.md` before that one, since 0.2.7 also rewrote gfx12-class behaviour there.
- **The data files the unit points at are unchanged**: `data/draft_vocab.bin` md5
  `4135aad364e18d71caec866dc37d34e7` and `data/expert-profile-coder.bin` md5
  `a7ea5acbdff82c1e73ee023c0e818c6b`, **equal between the two input trees** (and equal to
  the values recorded at the 2026-10-23 bump), so the hand-prepared pack, the MTP runtime
  and the `expert-profile-coder.bin` the engine reads inside the package still mean the
  same thing. `diff -rq data/` between the two trees reports **one entry**: the new
  `data/dsv41` (DeepSeek-V4.1's own routing profiles — not ours). The model files, the
  feed index and `SHA256SUMS.txt` are not in this input at all (they are fetched into the
  container at runtime, per the finding above), so nothing in this bump touches them.
- **`--conversation-cache-mib 0` stays pinned, and its reason is now different.** 0.2.7
  fixes the bug the pin was written against ("a conversation cache no longer reuses
  another conversation's keys and values", #1503), so the pin is no longer "we have a
  bug upstream" but "a feature this box has never run". It stays 0: the auto path would
  park ~2 GiB of host RAM on a box already at 28.3 GiB RSS of 30 GiB, and the sizing
  table above is the reason — re-read it before turning it on, as that comment says.

What gated it: the four assertion evals, `just fmt-check` and `just check-source-size`
pass; `nix flake check --no-build` dies as it does at this lock for every tree
(`sops-secret-pg_pass.drv is not valid`, the eval-time secret read — verified against
a clean HEAD, so it is not this change). **The bump was not measured on this pack**
before deploying: the route prior's and the int8-attention numbers above are
upstream's, on their quant, and this box's first numbers are the switch's own journal.

### Update: engine 0.1.48.1 — 2026-10-23

`nix flake update strata` moved the input `9ae93f7` (Control 0.1.46.1) →
`7ddc17a` (Control 0.1.48.1: engine 0.1.48 plus the Control-only hotfix, whose
whole change is Windows long-path handling). What the bump does **for us**, read
off `git diff 9ae93f7 7ddc17a` before deploying:

- **One new default can move us on memory, and it sits right on our threshold.**
  `--conversation-cache-mib` went from `0` (off, opt-in) to `-1` = "`--serve`
  sizes a host-RAM cache for parked conversations from the RAM still free once
  the model is loaded": 40 % of (available − `conversation_cache_min_free_mib`,
  default 2560 MiB), in 256 MiB steps, at most 8192 MiB, off below 1024
  (`auto_conversation_cache_mib`, upstream #3493; `prompt_cache` 6 and 4 slots are
  the defaults on both revs, so the path fires). `free -m` here says 3556 MiB
  available with nothing loaded — 996 usable, i.e. under the 1024 floor, so the
  rule would answer 0 today — but that is a reading of free RAM at engine start on
  a box that runs at 28.3 GiB RSS of 30 GiB, and the next 2 GiB of it is OOM
  beside Prometheus. So `strata.nix` now passes **`--conversation-cache-mib 0`
  explicitly**: the old behaviour, spelled, and upstream's own comment says an
  explicit value "is never touched". The engine prints **nothing** about it then —
  the `conversation cache off: not enough free RAM…` line is the *auto* path's refusal
  (`generate.cpp:9385`), not what an explicit 0 says — so the proof is the rendered
  config and the running engine's own argv, not a log line.
- **The copy list still covers upstream.** `serve/`, `strata_control`, `strata_cli`,
  `tools/` and `data/` are what the runtime layer needs: at 0.1.48 nothing in those
  three packages imports a top-level package outside them (an `ast` sweep of every
  module-scope `import`/`from` in `serve`, `strata_control`, `strata_cli` found
  none), and the build-time guard imports all three anyway.
- **The engine flags in `engineArgs` still parse**: all of
  `--pack --native --ple-gguf --expert-profile --expert-cache --prefill --spec
  --spec-min-p --mtp --max-context --kv --resident-experts` are still in
  `src/program/generate.cpp` at the new rev. `--spec-min-p` is on its way out
  (`--draft-conf` replaces it) but only for `--draft dflash2`, which we do not use;
  we stay on MTP.
- **The `configFile` keys still read**: `exe args cwd tokenizer model_name log
  allowed_hosts api_key backend lib_dirs env` all appear in `serve/server.py`, and
  nothing in the new server requires a key we do not set.
- **The model data on disk stays valid.** In `data/` only `gguf_fingerprints.json`
  changed, and `draft_vocab.bin` (425196 bytes) and `expert-profile-coder.bin`
  (98328) are byte-identical across the two revs — same md5 at `9ae93f7` and at
  `7ddc17a` — so the hand-prepared pack, the MTP runtime and the `draft_vocab.bin`
  copy under `~/Strata/mtp/rt` need nothing redone. `tools/iq_pack.py` grew a
  resume path for interrupted writes; the index format (`index.txt`'s 19-field rows,
  `src/core/weights.cpp`) is untouched.
- **Nothing new phones home.** The runtime layer's URLs are all `127.0.0.1`, test
  fixtures (`evil.example…`) and doc strings; `batch_telemetry` is still opt-in and
  is an engine log line, not a network call; `telemetry_read_key` is a local read
  endpoint.
- **Two answer-changing defaults** (upstream's own words: "Answers are not bit-identical…
  near-tie differences at default settings") — prefill scheduling and draft
  selection — so the Coder's answers move at the edges of ties. No flag of ours
  changed meaning.
- **The other Strix-Halo defaults in 0.1.48/0.1.48.1 are gated where we do not
  compile them**: `STRATA_PF_PAD` refuses itself on non-gfx11 cards, `STRATA_HC_Q8`
  is an opt-in experiment (`docs/HIP_FUSED_GR.md` names its 0.25 GB/layer trade), the
  8192-prompt chunk is Windows, and the new integrated-GPU `expert-cache auto` guard
  (`uma_cache.hpp`) is UMA — it is aibox's change, not ours.

The one thing this bump does **not** buy us: Gyro — the `gyro` family, `--ple-mode
hybrid` (Cantiga/Fora/Gyro), and DFlash2 — none of it is this model or this card
(DFlash2 wants ~1.2 GiB of VRAM and `--vram-reserve-mib 1900`, which the 12 GB card
here has already spent on the arena and the cache).

What gated the bump: the four assertion evals and `just fmt-check` pass; `nix flake
check --no-build` dies as it does at this lock for every tree (`sops-secret-pg_pass.drv
is not valid` — the eval-time secret read, `docs/follow-ups.md`, "`just check` is
machine-dependent"), and `git stash` shows the same death at clean HEAD, so it is not
this change.

**What the deploy proved**, 2026-10-23:

- Built on aibox (its checkout at `affbbbd`): `nix build .#strata-cuda --no-link
  --print-out-paths` → **`/nix/store/8ksf1q4s986ii6m44i86cvi0a22y35r4-strata-unstable`**,
  exit 0, 1,506,771,880 bytes. It led with ~2.5 min of CUDA package builds
  (`cuda_cudart-13.0.96`, `cuda_nvcc-13.0.88`, `cuda_nvrtc`, `libcublas-13.1.1.3`…):
  those outputs were in **neither** store at this lock (aibox and services2 both), so a
  bump pays them once and then ~7.5 min of engine.
- Copied back with **`?ssh-key=`**: `sudo nix copy --no-check-sigs --from
  'ssh://leigh-admin@10.3.1.32?ssh-key=/home/leigh-admin/.ssh/agent-hop-key' <path>`.
  Without the key parameter nix's ssh gets `Too many authentication failures` — there is
  no agent on these boxes, so the bare sshurl in the cutover block above is not enough
  from a shell.
- `just switch` on services2: generation 19 → 20, activated at 19:50:53, `nixos-confirm`
  chained and `system-good` == `/run/current-system` (the GOLDEN RULE, and the box
  survived: `Result=success`, `RestartCount=0`). Activation's own words were
  `stopping the following units: strata.service` then `starting the following units:
  strata.service` — the engine reload is part of the activation, as it has always been.
- The new engine is the one running: log banner `strata generate: session is up (engine
  0.1.48.1)` (the newest of ten sessions in `/var/lib/strata/strata.log`), and its argv
  carries the pin — `… --max-context 131072 --kv int8 --conversation-cache-mib 0
  --resident-experts` — read off `/proc`, and `"--conversation-cache-mib","0"` appears
  once in the rendered store config `d2qmk8lq…-strata-coder-iq1m.json` that `ExecStart`
  names (rule 10: read the rendered unit *and* the rendered config).
- **Memory came up 1.2 GiB higher than the sizing table's 27.4 GiB RSS**: cgroup
  `MemoryCurrent=29.97 GiB`, `MemoryPeak=30.30 GiB`; `nvidia-smi` 11643 of 12288 MiB
  (was 11499); swap 1949 MiB used (was 2305); `free -m` 3211 available. The arena shape
  is unchanged — the engine's new `prefill plan inputs` line says 23.4 GiB experts, 3.7
  GiB in the GPU caches, 22.8 GiB RAM copy — so the extra ~1.2 GiB is other structures
  around the same arena. Still inside, but tighter, and it is the reason the auto
  conversation cache stays pinned off: the box now has ~1.5 GiB less room than the table
  implies, and `--conversation-cache-mib 0` is what keeps the floor.
- Endpoints, in order: `/health` `loaded: true`, `api_key: true`, max_context 131072;
  `/v1/models` 200 with the key and **401** without; the LAN `Host` trick
  (`curl -H 'Host: strata.int.leighhack.org' http://10.3.1.21:8080/v1/models`) answers
  **401 now, not 403** — the boundary is the same refusal, 0.1.48 words it as the
  authentication error; `strata.int.leighhack.org` 200 with no key (nginx injects),
  `strata.leighhack.org` 401 without a key and 200 with one. A chat completion answers
  and puts the thinking in **`reasoning_content`** with `content: null` while the
  completion is cut mid-reasoning (`finish_reason: length`) — this is **not** new in this
  bump (`reasoning_content` appears 7× in `serve/server.py` at both revs), but nothing in
  this file said it, and a consumer that reads only `content` loses every truncated
  answer from a thinking model. Ask for enough tokens, or read both fields.
- **aibox needed nothing** (`machines/aibox/strata.nix` is PARKED, so the bump does not
  touch a deployed engine there), but its store gained the CUDA set and the new engine,
  which is what the next services1/services2 bump will substitute instead of rebuilding.

One slip to keep out of the habit: checking the vhost boundary by grepping services1's
rendered `/etc/nginx/nginx.conf` printed the **injected `Authorization` header value**
into the session transcript (2026-10-23). That value was already a world-readable store
path (`strata_api_key` is in `infra.buildTimeSecrets`, `common/sops.nix`), so
nothing new was exposed, but AGENTS.md rule 13 is the rule that was broken: read a
header block by *name*, not with a grep that prints its value, and if a secret has to be
compared, compare hashes.

## Differences from aibox worth knowing

- **Backend is `cuda`** and the config sets `lib_dirs = [ "/run/opengl-driver/lib" ]`
  so `serve/server.py` puts the driver's `libcuda.so.1` on the engine's
  `LD_LIBRARY_PATH`; the toolkit libraries are on the binary's RUNPATH.
- **No `HSA_*` / `STRATA_HIP_*` workarounds** — those are ROCm/gfx1030-specific.
- **No nginx wiring** on this box. aibox serves its instance through its own
  nginx (`aibox.int.leighhack.org/llm`); services2 has no nginx. The vhost lives
  on the front door instead — the `strata.int`/`strata.leighhack.org` rows in
  `common/front-door.nix`.  The public `strata.leighhack.org` record is hand-made
  in the DigitalOcean zone: a CNAME to `nginx.leighhack.org`, the same trick every
  other public name here uses, because services2 has no public IP of its own.
  **A LAN vhost and this file's `allowed_hosts` move together**: upstream's
  server 403s any `Host` it does not answer to (DNS-rebinding protection,
  `host_allowed()` in `serve/server.py`) and nginx forwards the incoming header
  upstream, so a name that is served but not listed answers every request
  `Host '…' is not allowed (DNS rebinding protection)`.  **0.1.48 words both that
  refusal and the missing-key refusal as the authentication error, so the LAN `Host`
  trick and a keyless public call answer 401, not the 403 this paragraph describes
  (measured 2026-10-23 at `0.1.48.1`): the boundary is the same refusal, the code changed
  shape.**  That check runs **only
  while the engine has no API key** (`if svc.api_key or host_allowed(...)`), so
  the key is what lets the public name through without listing it — and taking
  the key out would make the public vhost 403 rather than expose an
  unauthenticated engine.  Keep it that way: do not add
  `strata.leighhack.org` to `allowed_hosts`.

## The Vega 8 (Cezanne, gfx90c) is not the expert tier

Asked on 2026-10-24 whether this box's iGPU could take the CPU's share of the experts
beside the 3060 — a co-processor, the thing aibox's whole page is about. It cannot, and
the answer is three mechanisms and a number, all of it re-verified rather than argued
from plausibility. **Do not re-explore it from the flag list**: upstream closed exactly
this request, and the reason is memory, not a missing flag.

- **Every second-device role enumerates the backend's own device list, and this box has
  one CUDA device.** The shipped binary (`strata --version` → `strata 0.1.48.1` as read
  before the 2026-10-27 bump; the new rev's binary prints `strata 0.1.52` because that
  number is `CMakeLists.txt:11`'s `project(strata VERSION 0.1.52)` — see the version-marker
  note at the top of this file) offers
  `--expert-cache-device1/2/3` as *`fill CUDA1 with N experts`*, `--peer-device N` as
  *a second GPU as an adaptive expert-cache tier (rows over P2P; it also computes its
  experts' prompt rows)* and `--layer-split`; all three take indices into the backend's
  device list, and `strata-device --list-devices` on this box prints `device 0: NVIDIA
  GeForce RTX 3060` and nothing else. The Vega 8 is not in that list, so no role can
  name it: an engine binary compiles **one** backend (`CMakeLists.txt` makes the SYCL
  build exclusive of CUDA/HIP with a `FATAL_ERROR`), so there is no CUDA-plus-HIP
  process that could hold both cards.
- **The card-fit rule deletes integrated chips before any arithmetic** (`gpu_fit.py`,
  upstream at the 0.1.48.1 revision): `MIN_CARD_GB = 4.0  # under this a card cannot
  hold the dense weights: ignored (an iGPU, a display card)`, and the sentence it shows
  the user is `it shares the system memory (an integrated graphics chip), so it adds
  nothing`. `docs/MULTI_GPU.md:91` states the rule in prose — *not an integrated chip
  beside a real card* — and the not-supported list there includes *a mix of NVIDIA and
  AMD cards*; `gpu_fit.py:38` keeps `one engine, one vendor`.
- **gfx90c is in no HIP architecture list**, so the iGPU could not be an AMD engine
  device even alone: `cmake/hip_backend.cmake` builds wave32 RDNA2/3/4 — validated
  `gfx1100 gfx1201`, community `gfx1101 gfx1200`, opt-in list `gfx1010 … gfx1152`
  (which includes `gfx1103`, a 780M iGPU, and `gfx1151`, Strix Halo) and `FATAL_ERROR`s
  anything else. The GCN family is the separate wave64 `STRATA_HIP_GFX906` compat build
  (`include/strata/platform/hip_compat/strata_hip.h:7`: every Strata kernel is written
  for a 32-lane warp, and gfx906 runs 64-lane wavefronts) whose startup check refuses a
  wave32 card (#1728). Cezanne is neither. Its own identity is on the box: `/sys/class/kfd/kfd/topology/nodes/1/properties` → `gfx_target_version 90012` (gfx90c), `simd_count 32` at `simd_per_cu 4` (8 CUs). Upstream's own detector agrees with the KFD, not with the engine: `hip_gpu_archs()` in the tree at the 0.1.48.1 revision answers `{0: 'gfx90c'}` on this box (it reads those same nodes), so the card is *identified* and still offered to nothing — the arch list above is the reason, not a missing probe.
- **Upstream closed precisely this request**: issue **#514**, *Feature Request: use AMD
  iGPU instead of CPU for offloading* (closed as not planned, 2026-10-03) — "Running CUDA
  and HIP in one process would mean two backends side by side and a hand-off between them
  for every layer. Also, the offloaded experts are mostly limited by memory bandwidth,
  and an iGPU reads the same system RAM as the CPU, so the gain would be small next to
  the cost. **Image generation is compute-heavy, which is why the iGPU helped there.**"
  aibox's 2026-10-08 UMA deep dive (in git history) already separates that issue (an iGPU as
  a *second* device beside CUDA) from aibox's own case (the iGPU as the *primary* device),
  so the closing reason is not a reading of a different question.
- **And the number says it would not win anyway.** The nearest measured analogue is aibox's
  iGPU — a *different, newer* ISA (RDNA2 gfx1035), and **smaller** than this box's: 6 CUs against
  Cezanne's 8. There the engine on the iGPU is compute-bound on those CUs: prefill is 6-CU compute-bound
  at ~45 tok/s, and decode is DRAM-bandwidth-bound, ~2.7 GB/token at ~50 GB/s → ~18 tok/s theoretical,
  "we sit at 9-12" (`machines/aibox/STRATA.md:78-80`). This box's unit logs **26.4-28.2 tok/s end to
  end** on its `done:` lines (`journalctl -u strata.service`, read 2026-10-24), so a same-or-smaller
  chip on an older ISA has to beat a ~3x margin before it is worth a line of config — and aibox's own
  reasoning says the direction cannot pay: putting experts on the GPU "moves expert work onto the same
  6 CUs that are already the dense-compute bottleneck, instead of the idle CPU cores" (`:74-76`), and moving
  them "changes *who computes*, not the DRAM traffic" (`:78-80`). An iGPU is a third reader of the CPU's DDR5,
  not a new fast one.
- **It is also a hazard rather than an idle resource**: the iGPU's GTT *is* this box's RAM.
  The engine is already **25.7 GiB resident** of this box's ~30 GiB RAM (`ps -eo rss,comm` on
  `strata-host`, 2026-10-24, with `--expert-cache --resident-experts --expert-profile` in the
  rendered config), and the iGPU's GTT is that same RAM — `machines/services2/README.md:59`
  keeps the memory-pressure and OOM story for whatever else competes with it. `src/core/expert_cache.cpp:30-34`
  prices the expert cache against UMA-aware `device_free_bytes()`, which asks the *current* device
  whether it is integrated — the comment there says a box can mix an APU with a discrete card, and
  the answer is per device — and that measurement states what it means:
  on UMA a cache is only worth having if reading it is faster than reading the arena — it isn't, it
  is the same DRAM.

Where an AMD APU does earn a seat, per the same maintainer sentence and `tools/image/qimage.py:74-96` (whose APU list is `gfx902, gfx909, gfx90c` — Raven/Renoir/Cezanne — and whose AMD APU backend is `vulkan`), is the **image path**, not the expert tier; what that needs on this box is an open item in `docs/follow-ups.md`. The upstream file:line citations above are to the tree at the 0.1.48.1 revision — the same version this box's engine reports — read from the upstream clone `setup.sh --clone` puts in `/tmp/strata-up/up` (transient: `/tmp` is cleared at boot here, so re-clone before checking a line number).

## The card's half-precision tensor ceiling is a *driver* limit, and the kmod that opens it is prefill-only (2026-10-25)

Asked, while pi is being served by this very unit, "can `git@github.com:Zagitta/gpu-unlock.git` speed up strata?". **Nothing was tried on this box** — the answer is the one that has to be read before anyone tries it, because the mechanism is boot-critical and the win is not in the tokens this box spends time in. Upstream = `github.com/Zagitta/gpu-unlock`; its issue/PR numbers below are read at its `main` on 2026-10-25.

**What is capped, on this card.** `lspci -nn` reads `NVIDIA Corporation GA106 [GeForce RTX 3060 Lite Hash Rate] [10de:2504]`, `nvidia-smi --query-gpu=pci.bus_id,vbios_version` reads `00000000:01:00.0`, VBIOS `94.06.25.00.87`, and the power limit is the **170.00 W default** (`nvidia-smi -q -d POWER`: `Current = Requested = Default = 170.00 W`). NVIDIA's GA10x-era VBIOS ships two speed-requirement selectors, `SS0`/`SS1`; the driver programs them per product class and on a desktop GA106 leaves FP2048 (half-precision tensor FMA) at **1 FMA2 per clock per SM** against FP64's 2 and FP1 (integer DP4A) at 4, so BF16/FP16 tensor math issues at **half rate**. This is a driver-modeled knob, not a hardware flaw: there is no NVIDIA setting that raises it, and `gpu-unlock`'s README states NVIDIA's DLA forbids changing or interfering with it.

**What the kmod does.** An out-of-tree module boots the card's *own* GSP **before `nvidia.ko` binds it**, runs Falcon/V67 on it, writes `SS0=0x88888888` / `SS1=0x8`, then performs FLR so the **unmodified driver binds normally** afterwards: no VBIOS write, no driver patch, no signing-key bypass, and the report is read back through `journalctl -k -b | grep gpu_unlock` (must print `unlocked`, and `nvidia` must bind after it). It is state, not a hack — and unlocking does not survive a card power-off, which is why it must load from the initrd on every boot (`gpu_unlock.resume=0` is the flag that keeps that honest).

**Why it does not pay here — measured upstream, on this card family, not argued:**

- **Strata is on the throttled path, so unlike llama.cpp it does gain.** Upstream issue #31 is "4x RTX 3060 12GB running Qwen3.8 Flash Next IQ4_XS on **Strata**. Prefill increased from 450 to 600, but decode performance barely changed" — same engine, same card family, same model family: **+33% prefill, 0% decode**. llama.cpp's own null is a *different* path: its quantized prefill rides INT8 MMQ, never throttled (issue #13's practical note measured llama-bench pp512 1460.5 -> 1467.1, tg128 43.63 -> 43.65 — nothing). Do not carry that null across to strata.
- **The gain is power-capped, not the 1.85x of the microbenchmark.** Upstream PR #13 is the GA106 report (`10de:2504`, `10de:2507`, `10de:26a0`): dense GEMM 25.8-26.9 -> 47.6-49.0 TFLOPS, FP32 bit-identical — so the cap is live on our exact PCI ID. Upstream issue #75 (a 3090, an LLM) then reports why the microbenchmark does not translate: decode is "strictly GDDR6X bandwidth-bound" (0%) and prefill moved only +9.3% under a 240 W cap, because doubling dispatch doubles the card's capacitive load. The nearest measured analogue to *our* workload is upstream issue #78: an RTX 3060 12 GB LHR `10de:2504` on **NixOS**, driver **595.99.02 open kernel modules** (we run `595.104.02`, `hardware.nvidia.open = true`), the same **170 W** default, reading **1.20-1.32x on real inference** (Immich's ViT encoder) against 1.75-1.80x on a bare GEMM. So: 1.2-1.3x on GPU-bound compute is the honest ceiling here.
- **And this box's GPU-bound compute is a minority of the wall.** This session's engine line read `done: 2972 tokens in 140 s (27.2 tok/s) … expert cache 74.1% hit (+15.6% of the routed experts over PCIe)`; `nvidia-smi dmon -s cptu` during generation reads `sm 0%` at 36 W while the 5 s power sample peaks at 146 W of 170; and the experts are 22.8 GiB of **RAM** streaming over PCIe (this doc's sizing table, and the `expert cache` hit in every engine line). So the unlock buys low single digits to ~15% on a prefill-heavy call and **0% on the ~27 tok/s of decode that a turn actually spends its time in** — this box's logged decode is 26.4-28.2 tok/s (`:400`).

**What it would cost, which is why it is parked.** Every half of it is a repo rule, and none of it is free:

- an out-of-tree module built against our kernel (the boot here is `initrd-linux-6.18.56-initrd.efi`) and loaded **in the initrd**, before `nvidia` binds. Neither half exists today: the initrd at `/boot/EFI/nixos/c2zwr2j9fqcfslx70cxi2pzb351j43xk-initrd-linux-6.18.56-initrd.efi` — an *uncompressed* newc cpio (`07070100` magic), so greppable — has zero hits for `gpu_unlock` and zero for `lib/firmware`, and `machines/services2/hardware-configuration.nix:26` is `boot.initrd.kernelModules = []`. The good news is that `nvidia` is **not** in the initrd either (`boot.initrd.kernelModules` does not name it, nothing pulled it), so the ordering upstream requires holds today; upstream's issues #11 and #76 are exactly "`nvidia.ko` in the initramfs binds the card before gpu_unlock runs", so anything that ever pulls `nvidia` into the initrd kills this silently.
- a kernel command line: `gpu_unlock.targets=0000:01:00.0` — the module is **inert without the flag**, which is the redeeming feature; that list already exists at `machines/services2/hardware-configuration.nix:35`.
- its blobs are not in the repo it points at: `make firmware` downloads NVIDIA's own kernel-module source archive and extracts the booter images and `gsp_ga10x.bin` out of **CachyOS's proprietary `nvidia-x86_64-610.43.03` package**, and `firmware/NOTICE.md` says NVIDIA's redistribution is not approved — "Personal local packaging is the initial delivery target". So it cannot be a flake *input*, and this flake's every-input-is-a-network-source property is what breaks first. The `610.43.03` pin (`gpu-unlock`'s `Makefile:3`) is the **bootstrap** generation, not the driver's: #78 ran it against `595.99.02`, so that pairing already has one report on this card family; every new NVIDIA branch is unverified until someone tests it.
- the failure mode is boot-adjacent and bad: the module claims the card's memory BAR and runs GSP/Falcon/SEC2 before the driver, and a death after it takes BAR leaves `nvidia.ko` failing `request_mem_region`, i.e. **the card has no driver and `strata.service` has no GPU** (upstream issues #21/#43/#67; #21 is a GA106 LHR reporter who got `unlock failed (-110)` at the older commit `fc83321`, with two 3060 LHRs — the merged PR #13 plus #35/#70/#72 is what a 3060 LHR needs, and #78 confirms that combination works on a 3060 LHR). A bad module in the initrd is an unbootable box, recovered only from an older generation, on the box that is serving the session being written.

**The one measurement that decides it, without touching boot:** a dense BF16/FP16 cuBLAS GEMM on the card using the CUDA toolchain `packages.strata-cuda` already puts on this box. Stock reads ~26 TFLOPS => the ceiling is live and the only open question is strata's GPU share of prefill, which the engine's own `prefill plan inputs` line answers; stock reads ~48 => the card is not paying the cap and there is nothing to win. **Nobody has run one on this box.** Upstream's compatibility-report template asks exactly this (cublas GEMM, the `1.8x` test, the driver line) — its "check what you have" one-liner for the cap itself is `nvidia-smi dmon -s c`, which is *not* usable here: this box's `nvidia-smi` (595.104.02) prints only `mclk`/`pclk` for `-s c`, no `SS0`/`SS1`.

**The free half of the finding, worth keeping.** The cap covers *only* half-precision tensor math — FP32 FFMA and integer DP4A never pay it (that is why llama.cpp's INT8 MMQ measures ~35 TFLOPS stock). If Strata grows an integer/DP4A prefill path upstream, it buys the same thing with **zero boot risk**; nothing in `engineArgs` gives it today. Kept as an open item in `docs/follow-ups.md`.

## Operational notes

- **A killed engine keeps its VRAM.** If an experiment engine is `SIGKILL`ed, the
  next `strata.service` start fails with a `cudaMalloc(...) for the weight arena
  failed (out of memory)`; find and kill the leftover `strata` process, wait a
  few seconds, then start the unit. The unit's never-give-up policy keeps
  retrying in the meantime.
- **The arms table for this box is `STRATA-TUNE-2026-11-14.md`** — read it before adding
  an arm, because most of the obvious dials are already measured flat (and it carries the
  noise floor a claim has to clear).  Its one win is **deployed**:
  `STRATA_PF_FUSED=1` in `strata.nix` — +5 to +11 % at 8k, +1 to +3 % at 28k, decode
  unmoved.  Upstream lists it among the switches that change answers, and my first
  measurement of that (a greedy fingerprint per config) turned out to be measuring this
  box's own non-reproducibility — **three runs of one config, three different answers for
  two of three probes** — so the decision rests on `strata-answer-score.py`: nine greedy
  items with an answer key, **8/8 correct before it and 8/8 after**.  Revert = delete the
  line, `just switch`.
- **Greedy output on this engine is not reproducible run to run.**  Ask "did this change
  what the model says?" by running each side N times (`strata-answer-check.py` prints
  `stable N/M`) or by scoring against an answer key; one run per config attributes noise to
  the change, in both directions — it produced a false difference *and* a false agreement
  here before either was caught.
- **The `AIBOX` box has no equivalent — re-read aibox's `STRATA.md` before
  touching engine flags.** Every knob there (cache size, `--pcie-frac`,
  `--spec`, `--pool-workers`) was measured on the iGPU and does not transfer
  unchanged to CUDA.
- `system.autoRollback.enable` is on: after any `nixos-rebuild switch` here, run
  `sudo nixos-confirm` (the justfile chains it).
- New files must be `git add`ed — the flake reads its own directory through git.

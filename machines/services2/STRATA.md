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
| KV int8 @ 131072 | ~1.8 GiB | VRAM |
| PLE table | 0 | read from NVMe (`--ple-io direct`) |

The model's own RAM figure (32 GiB) is above this box's, so the engine runs its
**low-RAM mode**: `--resident-experts` copies the experts the GPU cache does not
hold into RAM once, instead of the full arena being resident. `setup.py` picks
exactly this variant when the arena does not fit but the non-GPU part plus
~10 GiB of headroom does (`low_ram_needed` / `low_ram_resident`). The alternative
is `--mmap-experts`, which reads them through the OS file cache; it is faster in
some reports but lets RAM fill to zero under long prompts, which is not wanted
next to Prometheus.

KV is **not** streamed to RAM: `setup.py`'s rule (`kv_streaming_wanted`) needs
~34.8 GiB of RAM at 131072 with 8-bit KV, and this box does not have it, so the
KV stays in VRAM and costs expert-cache slots. A longer context is possible
(`--max-context`), but every extra 32K of context is another ~0.44 GiB of VRAM
taken from the cache; 131072 is the balance chosen here.

**Swap is a 16 GiB file, `/var/swap/services2`** (`./hardware-configuration.nix`; there is
no swap partition on this NVMe). It is not there for the model: the arena is page-locked where
it can be (`LimitMEMLOCK = infinity` in `./strata.nix`), so what swaps is the overflow. It is
there because strata measured **28.3 GiB RSS** against 30 GiB of RAM, which left ~1.5 GiB for
Prometheus/Grafana/gatus/exporters and for the nix builds this box was the network's build
machine for until 2026-10-17 (`common/build-farm.nix`, `maxJobs 8` then) — without it the box
went unresponsive
(journald: "Under memory pressure, flushing caches") and a build here was OOM material. If
`--resident-experts` is ever lowered, or a bigger pack lands here, re-read this table: the
swap file is headroom, not a licence to size the arena above RAM.

**`--expert-cache` is `auto`, not a number.** On this discrete card `auto` reads
dedicated VRAM (aibox's iGPU warning — that `auto` reads GTT and fills the
machine — does not apply). If the engine's log shows VRAM pressure, set an
explicit smaller number, as aibox does.

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
and the `.#strata-cuda` package rebuild from the same tree. The llama.cpp
revision the engine vendors is pinned separately in `strata-package.nix` (from
`setup.py`'s `LLAMA_CPP_COMMIT`), so the ggml side is reproducible.

The CUDA build is `STRATA_ENABLE_CUDA=ON`, `-DCMAKE_CUDA_ARCHITECTURES=86`, and
CUDA 13.0 (`cudaPackages_13_0`). Note `STRATA_PREFILL_MMQ=ON` is HIP-only
(CMake fails the configure if CUDA is also on), so it is **not** passed here —
do not copy it from `machines/aibox/strata-package.nix`.

### Updating the engine (what a bump actually touches)

`nix flake update strata` **is** the bump: the input tracks upstream `main` and
nothing anywhere pins a version, so the locked rev is the whole contract and the
first thing it changes is behaviour nobody reviewed.

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

**Build it on aibox, not here.** Any edit to `strata-package.nix` or a new rev
invalidates the whole engine derivation: ~7 min at `-j6` on aibox, ~1.5 GiB of
output, and a build on services2 competes with the 27 GiB engine for the memory
that made this box the network's builder's opposite (see the sizing table).

```bash
# on aibox, with the bump (and the fixed copy list) in its checkout
nix build .#strata-cuda --no-link --print-out-paths
# on services2, as root: `trusted-users = root`, and the path is unsigned because
# aibox's daemon signs nothing it builds, so --no-check-sigs (only a trusted
# client may ask the daemon to add an unsigned path)
sudo nix copy --no-check-sigs \
  --from 'ssh://leigh-admin@10.3.1.32?ssh-key=/home/leigh-admin/.ssh/agent-hop-key' \
  /nix/store/<new strata-unstable>
just switch     # builds the toplevel from the store, activates, confirms
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
path (`docs/flake-simplification-audit-2026-10-22.md`, the `strata_api_key` bullet), so
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
  one CUDA device.** The shipped binary (`strata --version` → `strata 0.1.48.1`) offers
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
  `machines/aibox/strata-uma-2026-10-08.md:46-48` already separates that issue (an iGPU as
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
  the answer is per device — and `machines/aibox/strata-uma-2026-10-08.md:31` states what it means:
  on UMA a cache is only worth having if reading it is faster than reading the arena — it isn't, it
  is the same DRAM.

Where an AMD APU does earn a seat, per the same maintainer sentence and `tools/image/qimage.py:74-96` (whose APU list is `gfx902, gfx909, gfx90c` — Raven/Renoir/Cezanne — and whose AMD APU backend is `vulkan`), is the **image path**, not the expert tier; what that needs on this box is an open item in `docs/follow-ups.md`. The upstream file:line citations above are to the tree at the 0.1.48.1 revision — the same version this box's engine reports — read from the upstream clone `setup.sh --clone` puts in `/tmp/strata-up/up` (transient: `/tmp` is cleared at boot here, so re-clone before checking a line number).

## Operational notes

- **A killed engine keeps its VRAM.** If an experiment engine is `SIGKILL`ed, the
  next `strata.service` start fails with a `cudaMalloc(...) for the weight arena
  failed (out of memory)`; find and kill the leftover `strata` process, wait a
  few seconds, then start the unit. The unit's never-give-up policy keeps
  retrying in the meantime.
- **The `AIBOX` box has no equivalent — re-read aibox's `STRATA.md` before
  touching engine flags.** Every knob there (cache size, `--pcie-frac`,
  `--spec`, `--pool-workers`) was measured on the iGPU and does not transfer
  unchanged to CUDA.
- `system.autoRollback.enable` is on: after any `nixos-rebuild switch` here, run
  `sudo nixos-confirm` (the justfile chains it).
- New files must be `git add`ed — the flake reads its own directory through git.

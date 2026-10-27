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

### Update: engine 0.1.52 (app 0.2.7) — 2026-10-27

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
far less than that number suggests, but they moved, which is why this bump's engine build
costs ~35 min on aibox and the 2026-10-23 bump cost 38 s. The app layer is not built by
anything here (`strata-package.nix` compiles the CUDA engine and the install phase's
Python layer only), so the app and the Terminal travel into the store and run on nothing.
(`strata-package.nix` compiles the CUDA engine only), so the app and the Terminal
travel into the store and run on nothing.

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
- **The pinned `llama.cpp` is unchanged**: `setup.py:222 LLAMA_CPP_COMMIT = "3cf03257f219afbe7334045ff7c6a06ac68c627d"` at the new rev, same as the rev `strata-package.nix:44` pins and same as `third_party/ggml/VERSION.txt` in the new tree — so the **vendored ggml** side of this build is byte-for-byte what 0.1.46 built. **What is *not* unchanged is the engine's own sources** (384 files differ, above), so the engine derivation is dirtied and recompiles: this bump's engine build on aibox measured ~35 min where the 2026-10-23 bump (engine byte-identical, app-only change) took 38 s. The same is true of the *other* box: `machines/aibox/strata-package.nix` takes the same input, so aibox's HIP/gfx1030 engine recompiles on the next switch there too — read `machines/aibox/STRATA.md` before that one, since 0.2.7 also rewrote gfx12-class behaviour there.
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
- **The `AIBOX` box has no equivalent — re-read aibox's `STRATA.md` before
  touching engine flags.** Every knob there (cache size, `--pcie-frac`,
  `--spec`, `--pool-workers`) was measured on the iGPU and does not transfer
  unchanged to CUDA.
- `system.autoRollback.enable` is on: after any `nixos-rebuild switch` here, run
  `sudo nixos-confirm` (the justfile chains it).
- New files must be `git add`ed — the flake reads its own directory through git.

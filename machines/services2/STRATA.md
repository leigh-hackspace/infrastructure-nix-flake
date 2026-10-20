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
  use that name with the port; services1 also terminates TLS for it as
  `https://strata.int.leighhack.org` and — since 2026-10-18 — publicly as
  `https://strata.leighhack.org` (`machines/services1/services/strata-proxy.nix`).
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

## Differences from aibox worth knowing

- **Backend is `cuda`** and the config sets `lib_dirs = [ "/run/opengl-driver/lib" ]`
  so `serve/server.py` puts the driver's `libcuda.so.1` on the engine's
  `LD_LIBRARY_PATH`; the toolkit libraries are on the binary's RUNPATH.
- **No `HSA_*` / `STRATA_HIP_*` workarounds** — those are ROCm/gfx1030-specific.
- **No nginx wiring** on this box. aibox serves its instance through its own
  nginx (`aibox.int.leighhack.org/llm`); services2 has no nginx. The vhost lives
  on services1 instead — `machines/services1/services/strata-proxy.nix`, added
  2026-10-15, and `just dns-sync-sync` gave it its `*.int` record.  The public
  `strata.leighhack.org` record is hand-made in the DigitalOcean zone (dns-sync
  manages only `*.int`): a CNAME to `nginx.leighhack.org`, the same trick every
  other public name here uses, because services2 has no public IP of its own.
  **A LAN vhost and this file's `allowed_hosts` move together**: upstream's
  server 403s any `Host` it does not answer to (DNS-rebinding protection,
  `host_allowed()` in `serve/server.py`) and nginx forwards the incoming header
  upstream, so a name that is served but not listed answers every request
  `Host '…' is not allowed (DNS rebinding protection)`.  That check runs **only
  while the engine has no API key** (`if svc.api_key or host_allowed(...)`), so
  the key is what lets the public name through without listing it — and taking
  the key out would make the public vhost 403 rather than expose an
  unauthenticated engine.  Keep it that way: do not add
  `strata.leighhack.org` to `allowed_hosts`.

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

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
  use that name with the port; there is no services1 vhost for it yet.
- Logs: `journalctl -u strata -f`, `/var/lib/strata/strata.log`.

The aibox instance (`machines/aibox/strata.nix`, `machines/aibox/STRATA.md`) runs
the **full 125B model on the Radeon iGPU**. That doc's measurements (decode vs
prefill, why the expert cache is a compute tier, the UMA reality) are the
background reading; this file is what is different on a real, discrete 12 GB
card and a 30 GB box.

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
Prometheus/Grafana/gatus/exporters and for the nix builds this box is the network's build
machine for (`common/build-farm.nix`, `maxJobs 8`) — without it the box went unresponsive
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
STRATA=$(nix build --impure --expr \
  'let f = builtins.getFlake (toString /home/leigh-admin/Projects/infrastructure-nix-flake);
   in f.packages.x86_64-linux.strata-cuda' --no-link --print-out-paths)

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

## Differences from aibox worth knowing

- **Backend is `cuda`** and the config sets `lib_dirs = [ "/run/opengl-driver/lib" ]`
  so `serve/server.py` puts the driver's `libcuda.so.1` on the engine's
  `LD_LIBRARY_PATH`; the toolkit libraries are on the binary's RUNPATH.
- **No `HSA_*` / `STRATA_HIP_*` workarounds** — those are ROCm/gfx1030-specific.
- **No nginx wiring** on this box. aibox serves its instance through its own
  nginx (`aibox.int.leighhack.org/llm`); services2 has no nginx. If this should
  be a named vhost, add it on services1 (the network's TLS terminator) and run
  `just dns-sync-sync`; that is deliberately not done here.

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

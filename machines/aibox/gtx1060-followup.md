# GTX 1060 follow-up: capabilities, draft verdict, and the Whisper.cpp plan (aibox)

Date: 2026-08-26
Scope: live verification on the box + source analysis of the pinned llama.cpp
(flake.lock rev `f280b26983ad0fdb705a0d9ebf0503e76f2899b0`) and pinned nixpkgs
(`nixos-26.05`, rev `a3b98866eecd08edac6e61a3081e69540a35020f`). Follows
`gtx1060-draft-report.md` (2026-08-19) and `gtx1060-troubleshooting.md`
(2026-08-22).

## TL;DR

- **llama.cpp runs entirely on the AMD iGPU.** Measured live: during a 250-token
  generation the iGPU was pegged at 100% while the 1060 sat at 0% util / 0%
  memory bandwidth for 30 consecutive samples. The 1060 holds ~1.1GB of parked
  (idle) allocations that never move — it contributes zero compute today.
- **The 1060 is compute-healthy** (much healthier than 2026-08-22): driver loads
  clean, zero `RmInitAdapter` failures this boot, Vulkan compute pipelines
  (incl. flash-attn) create and run, 1.4–2.5GB allocations succeed.
- **The 1060 can never be llama.cpp's speculative-draft device on this box** —
  two independent, verified blockers (see §3).
- **The 1060 is a good fit for Whisper.cpp instead** (Vulkan backend): models
  fit in 3GB, it avoids the KV-cache allocation path that breaks on the card,
  and the CUDA backend is a dead end on Pascal with the pinned nixpkgs (§4).
  Recommended next step; not yet implemented.

## 1. Setup recap

- aibox: Ryzen 5 6600H, Radeon 660M iGPU (RADV, `Vulkan0`, ~60GB UMA/GTT) +
  GTX 1060 3GB (`Vulkan1`) attached via an M.2→USB3 adapter: PCIe link
  negotiated **2.5 GT/s × 1 (~250 MB/s)**, card's own capability is 8 GT/s × 16
  (`LnkCap` vs `LnkSta`). VRAM BAR is only 256MB.
- llama.cpp: `llama-cpp-vulkan` build, single `llama-server` router on
  `10.3.1.32:8081`, `--models-dir /home/leigh-admin/Models`, `--models-max 1`,
  `-dev Vulkan0 -ngl all --ctx-size 262144 --flash-attn on`.
- Active model: `Tiel-Coder-35B-A3B-GGUF-MTP/Tiel-Coder-35B-A3B-MTP-UD-Q5_K_XL.gguf`
  (26GB, 35B-A3B MoE, Qwen3-family) with `--spec-type draft-mtp` self-drafting.
  ~15–19 t/s with draft acceptance 0.5–0.75.

## 2. Verified: llama uses only the iGPU

Live probe (this date): 250-token generation via the running instance while
sampling both GPUs at 1 Hz.

| Metric | During generation |
|---|---|
| AMD iGPU `gpu_busy_percent` | 100% (whole run) |
| GTX 1060 SM/util (30 samples) | 0% every sample |
| GTX 1060 memory bandwidth | 0% every sample |
| Decode speed | 19.3 t/s (15–19 t/s session-wide) |

The 1060 shows `1122MiB` used at rest (≈1115 child + 1 router) but that memory
is never read during decode. Exact identity of the parked buffer unconfirmed
(most consistent with the mmproj + buffers; it is *not* model layers — the
model fits on the iGPU with `-dev Vulkan0`). Either way: not in the per-token
path.

## 3. Why it cannot be a speculative-draft device

Draft model used for the experiment: `ggml-org/Qwen3-1.7B-GGUF`
`Qwen3-1.7B-Q4_K_M.gguf` (1.2GB). Standalone on the 1060 at `-c 2048` it loads
and decodes at **53 t/s** — the card is fast enough. The blockers are
elsewhere:

### 3.1 Draft context is forced to the target's context (source-verified)

`common/speculative.cpp` (`common_speculative_init_result` ctor) at the pinned
rev:

```cpp
// the draft context holds as many tokens per sequence as the target context
cparams.n_ctx = llama_n_ctx(ctx_tgt);
```

The draft model's context is hardcoded to the target's 262144 tokens, with no
CLI flag to cap it. A 1.7B draft (28 layers, 4 KV heads, f16) needs
≈ 28 × 4 × 128 × 2B × 262144 ≈ **15 GB of KV cache** — 5× the whole card.
`q8_0` halves it, `q4_0` ≈ 3.7GB: none fit. (This is also why `draft-mtp`
works at 262144: the MTP draft is a single `nextn` layer, so its KV is tiny.)

### 3.2 The card cannot allocate the KV buffer at all (empirically confirmed)

`llama-server -m qwen3-1.7b-q4_k_m.gguf -dev Vulkan1 -ngl all -fa on -c 262144`
failed at context init with ~3GB free on the card:

```
ggml_vulkan: Device memory allocation of size 1073741824 failed.
ggml_vulkan: vk::Device::allocateMemory: ErrorOutOfDeviceMemory
E alloc_tensor_range: failed to allocate Vulkan1 buffer of size 1073741824
E llama_init_from_model: failed to initialize the context: failed to allocate buffer for kv cache
```

Same 1GB-allocation failure signature as 2026-08-22
(`Device memory allocation of size 1050944000 failed`). The card happily
allocates weight tensors and compute buffers (1.4GB tests passed) but the
KV-cache buffer path fails on it. So even a hypothetical small draft ctx would
not load.

**Verdict: do not pursue the 1060 as a llama.cpp draft device at this context
size.** Keep `spec-type = draft-mtp`.

## 4. Whisper.cpp on the 1060 — viable, recommended

Assessment based on package inspection (not yet built/tested on the card):

| Factor | Finding |
|---|---|
| Models fit 3GB | `tiny` 75MB / `base` 142MB / `small` 466MB / `medium` 1.5GB / `large-v3-turbo` 1.6GB; `large-v3` ~2.9GB borderline |
| Allocation profile | weights + graph buffers + tiny encoder/decoder KV over ~1500 frames — i.e. the path that **works** on this card (§3.2), not the broken KV-cache path |
| Backend | `whisper-cpp` 1.8.4 in pinned nixpkgs with `vulkanSupport` (same ggml Vulkan as llama) |
| CUDA | dead end: nixpkgs 26.05 defaults to CUDA 13, which dropped compute capability < 7.5 (Pascal sm_61 not supported) — Vulkan is the only GPU path |
| Link | irrelevant: model resident in VRAM, audio input ~2MB/min at 16kHz f32 |
| Expected perf | `medium` on the 1060 ≈ 5–15× realtime; frees the iGPU for llama and CPU for everything else |
| Fallback | whisper.cpp CPU backend works fine; if the card flakes mid-job the workload degrades gracefully (restart/retry) — low stakes vs a live llama session |

### Plan (not yet executed)

1. Add `whisper-cpp` with `vulkanSupport = true` (and `withFFmpegSupport = true`
   for `whisper-server`'s ffmpeg wrapper) to aibox — no new flake input.
2. Download `ggml-medium.bin` (~1.5GB) to `/home/leigh-admin/Models/whisper/`
   (package ships a patched `whisper-cpp-download-ggml-model` script that
   writes to the current directory).
3. New `machines/aibox/whisper.nix`: `whisper-server` systemd unit,
   `--device Vulkan1` (pin away from the iGPU which serves llama), port 8082,
   `Restart = always`, `after/wants = wait-for-network.service` (house style).
4. Build locally (`nixos-rebuild build --flake .#aibox --impure`), run a
   30-second load + transcription test on the card *before* deploying (same
   pattern as §5 below); if the allocation quirk bites, fall back to CPU or the
   iGPU.

## 5. Card health check results (2026-08-26, live)

- Driver 580.173.02 (legacy_580, patched for kernel 7.x strncpy) loads clean;
  **zero `RmInitAdapter failed`** in dmesg for the whole boot, including after
  Vulkan compute use (the 2026-08-22 storm did not reappear).
- Vulkan enumeration stable: `Vulkan0: AMD Radeon 660M`, `Vulkan1: NVIDIA GTX
  1060 3GB`.
- Compute verified via forced `-dev Vulkan1 -ngl 4 -fa on`:
  - `ornith-1.0-9b-Q4_K_M` (Gated Delta Net): fused GDN op assigned to Vulkan1,
    generation OK (4.05 t/s, mostly CPU as only 4 layers offloaded).
  - `Qwen3-VL-8B-Instruct-Q8_0` (transformer): flash-attn compute pipelines
    created fine — the exact `flash_attn_f32_f16_aligned` failure from 2026-08-22
    did **not** reproduce; two back-to-back generations OK (~6.1 t/s, GPU util
    7% mid-decode).
- Memory: held 1.4–2.5GB of allocations across tests (vs the 1GB OOM on
  2026-08-22).
- Caveats: only partial (4-layer) offload was testable (no model ≤3GB on the
  box); long-term/reboot stability unproven, but every 2026-08-22 failure mode
  failed to reproduce.

## 6. Router/preset mechanics learned (for future config work)

- `--models-preset` is an **INI** file (help says INI, not TOML); section name
  = model name (becomes `--alias`); keys map 1:1 to CLI long names
  (`spec-type`, `spec-draft-device`, `spec-draft-ngl`, `spec-draft-n-max`,
  `model`, `mmproj`, ...). Unknown keys throw at parse.
- `--models-dir` auto-generates presets per GGUF; companion files named
  `mtp-` / `dspark-` / `dflash-` are excluded from the model list and
  auto-wired as that model's **draft** (`LLAMA_ARG_SPEC_DRAFT_MODEL`). Do not
  drop such a file into a model dir unless the model's preset deliberately uses
  `draft-simple` — it silently changes the load config.
- Preset cascade: cached models < models-dir < custom INI, then the router's
  own CLI args merge on top of every model (so `-dev Vulkan0 -ngl all
  --ctx-size 262144 ...` reach all children).
- The router binds the `--host` IP (`10.3.1.32:8081`), **not** 127.0.0.1 —
  earlier "empty /v1/models" observations were curl connection-refused
  artifacts, not a router bug. Child instances bind 127.0.0.1 on ephemeral
  ports.

## 7. Operational notes / quirks observed

- The production Tiel child was gracefully unloaded at 18:20:07 by the router
  (`stopping model instance`, exit status 0) with no logged trigger. No crash,
  no GPU error. Models reload lazily on the next request (~1–2 min for the
  26GB load); expect that latency after any unload/reboot.
- Scratch `llama-server` test instances survived `timeout`'s SIGTERM (stuck in
  Vulkan teardown, orphaned when the ssh session ended). Use
  `timeout -s KILL` or kill explicitly; always verify with
  `nvidia-smi --query-compute-apps=pid,used_memory --format=csv`.
- `/tmp` on aibox is on the NVMe, not tmpfs.
- Useful live checks (run as `leigh-admin`):
  `nvidia-smi`, `nvidia-smi dmon -s u -d 1`,
  `cat /sys/class/drm/card0/device/gpu_busy_percent`,
  `llama-server --list-devices`, `sudo dmesg | grep -i RmInit`,
  `sudo journalctl -u llama-server -f`.

## 8. Recommendations

1. Keep `spec-type = draft-mtp` for Tiel/Ornith — the external-draft idea is
   closed for this box.
2. Give the 1060 to Whisper.cpp (Vulkan, pinned to `Vulkan1`, `medium` model,
   port 8082) — see plan §4. Test on-card before committing to it.
3. Remaining llama throughput levers (no hardware): IQ2_M quant, or
   `-ctk q8_0 -ctv q8_0` to halve KV bytes for long-context validation.
4. If the 1060's allocation quirk ever shows up elsewhere, remember the card
   does weights+buffers fine but KV-cache buffers fail — a hard constraint for
   any future use.

## Open questions

- Identity of the ~1.1GB parked allocation on the 1060 (mmproj most likely;
  never confirmed).
- Whether whisper.cpp's Vulkan path hits the same KV-buffer allocation failure
  (expected not; the §4 plan tests it first).
- Long-term stability of the 1060 through the M.2→USB3 adapter (currently
  healthy, historically intermittent).

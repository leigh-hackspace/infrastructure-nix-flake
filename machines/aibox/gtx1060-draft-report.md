# GTX 1060 3GB as a speculative-draft device for Laguna-S-2.1 (aibox)

Date: 2026-08-19
Scope: analysis only — no config changes made. Question: *"If I add a spare GTX 1060 3GB to this
machine and use it as a draft model device, can I get more than 8 tps on Laguna-S-2.1?"*

## TL;DR

**Probably yes — expect roughly 12-17 tps (~1.6-2x), not a dramatic jump, and the gain shrinks
as the 64K context fills up.**

- The pinned llama.cpp (flake.lock rev `9731ad3f`, Aug 2026) **already supports pinning the
  draft model to a different GPU** than the main model: `--spec-draft-device` / `-devd`.
  No llama.cpp upgrade or fork needed.
- The 1060 is a well-matched draft device: draft decoding is batch-1 / bandwidth-bound, and
  the card's 192 GB/s is ~2.5x the iGPU's ~77 GB/s UMA bandwidth. A 1-3B Q4 draft fits in 3GB.
- The binding constraint is the iGPU's UMA bandwidth for the **target** validation pass
  (~125ms/token at 8 tps). Speculative decoding amortises one target pass over ~2.3-3 accepted
  tokens, hence ~1.6-2x, not 3x+.
- Main uncertainty: the 64K context. The batched validation pass reads the KV cache once per
  validated position, so at long occupied contexts the KV bandwidth eats most of the gain.
  Measure at your real workload before buying anything.

## 1. Current state (verified on this machine)

- This machine **is** the aibox. `llama-server --list-devices` reports:
  `Vulkan0: AMD Radeon 660M (RADV REMBRANDT) (60416 MiB, 10402 MiB free)`.
- Laguna-S-2.1 (118B-A8B MoE, 8B active per token) runs at `UD-IQ3_S` (48.4GB), `-ngl all`,
  `-fa on`, `--ctx-size 65536`, `-t 12`, via the Vulkan build of llama.cpp on the iGPU's UMA
  (`amdgpu.gttsize=57344` in `hardware-configuration.nix`). Served by llama-swap at
  `10.3.1.32:8081` (`machines/aibox/ai.nix`).
- User-reported decode speed: ~8 tps (~125ms per token pass).

### Relevant hardware numbers

| Item | Value |
|---|---|
| Ryzen 5 6600H iGPU (Radeon 660M, RDNA2, 6 CU) | UMA, 56GB GTT, ~77 GB/s (DDR5-4800 dual channel; ~102 GB/s if LPDDR5-6400) |
| GTX 1060 3GB (Pascal) | 1152 CUDA cores, 192 GB/s GDDR5, ~3.9 TFLOPS, 120W TDP, 1x 6-pin, PCIe 3.0 x16, Vulkan 1.2+ |
| Active weights read per target pass | ~4GB at IQ3_S (8B active), i.e. ~52ms of pure UMA bandwidth → decode is not purely weight-bandwidth-bound; the rest of the 125ms is KV reads + compute |

## 2. Verified llama.cpp capabilities (pinned build)

Source: `llama-server --help` from the currently built
`llama-cpp-vulkan-0.0.0` (flake.lock rev `9731ad3f29da96f588711a0d1eb08cf210721e16`).

Speculative decoding is fully present, including per-device draft placement:

- `-dev, --device <dev1,dev2,..>` — devices for the **main** model (default: auto).
- `--spec-draft-device, -devd, --device-draft <dev1,dev2,..>` — devices for the **draft** model.
- `-md, --spec-draft-model FNAME` — draft model path.
- `--spec-draft-n-max N` (default **3**), `--spec-draft-n-min N`.
- `-ngld, --gpu-layers-draft N` — draft GPU layers.
- `-ctkd/-ctvd` — KV cache types for the draft (e.g. `q8_0` to shrink it).
- `--spec-type` — `draft-simple`, `draft-eagle3`, `draft-mtp`, `draft-dflash`, `draft-dspark`,
  `ngram-simple`, `ngram-map-k`, `ngram-map-k4v`, `ngram-mod`, `ngram-cache`.
- `--fit [on|off]` — default `on`; auto-shrinks ctx to fit device memory (safety net for 3GB).
- CPU-side draft knobs: `-td` threads, cpu-mask, prio, poll (only relevant for draft-on-CPU).

Note: once the 1060 is installed, **pin the main model to the iGPU** (`-dev Vulkan0`); device
auto-selection might otherwise try to use the new card for the main model.

## 3. Performance model

At 8 tps each target pass costs ~125ms. With a draft, per step: `K * t_draft + t_target_pass`,
producing `1 + E[accepted]` tokens, where `E[accepted] = α(1-α^K)/(1-α)`.

Draft on the 1060 (e.g. 1.7B Q4_K_M, ~1.1GB): ~80-150 tps → ~7-12ms per draft token.

| Scenario | t_draft | α (acceptance) | K | Est. tps |
|---|---|---|---|---|
| Conservative | 12ms | 0.55 | 5 | ~12 |
| Typical | 10ms | 0.60 | 5 | ~14 |
| Good (smaller draft) | 7ms | 0.70 | 5 | ~17 |

(`--spec-draft-n-max` default is 3; with t_draft ≈ 10ms, K=5 is about the sweet spot — the
acceptance probability drops off geometrically, so larger K mostly adds draft time.)

### The 64K-context caveat

The target validation pass reads the KV cache once **per validated position**, so its cost is
roughly `model_read + (K+1) * KV_read`. On ~77 GB/s UMA with a 48GB model, KV reads can rival
the weight read once the context is long and mostly occupied — the known weakness of
speculative decoding at long context. Implications:

- Typical agentic turn (short-ish ctx): ~1.6-2x win, i.e. 13-17 tps.
- Full 64K occupied context: the win largely evaporates; possibly a wash vs 8 tps.
- This is the biggest unknown and the cheapest thing to measure (see test plan).

## 4. What would need to change

### `machines/aibox/ai.nix` — add draft flags to the Laguna entry

```nix
"[Coding] Laguna-S-2.1-UD-IQ3_S" = {
  cmd = "${llamaCmdVulkan} -m ${modelsPath}/Laguna-S-2.1-UD-IQ3_S.gguf \
    -ngl all -fa on --ctx-size 65536 --metrics \
    --chat-template-kwargs '{\"enable_thinking\":true}' \
    -dev Vulkan0 \
    -md ${modelsPath}/<draft>.gguf -devd Vulkan1 -ngld 99 --spec-draft-n-max 5";
};
```

### NixOS side (aibox)

- NVIDIA proprietary driver alongside amdgpu (both coexist fine on NixOS). **Pascal requires
  the closed kernel module**: `hardware.nvidia.open = false`; enable modesetting. The NVIDIA
  Vulkan ICD is what llama.cpp will enumerate.
- The kernel module list already includes `thunderbolt` → the card presumably attaches via a
  TB3/TB4 eGPU enclosure. That's fine: the draft weights fit in the 1060's VRAM, so the
  enclosure's PCIe link (~2.75 GB/s) only carries per-token KV/token traffic, not per-token
  weight reads.
- Verify both devices show up: `llama-server --list-devices` should then list
  `Vulkan0: AMD Radeon 660M` and `Vulkan1: NVIDIA GeForce GTX 1060` (order may vary — use the
  names as printed).

### Draft model choice

- Must share Laguna's tokenizer (llama.cpp enforces this; it errors/warns on mismatch).
- If Laguna is Qwen3-family (Qwen3-Next is already in the config): Qwen3-0.6B / 1.7B / 4B
  Q4_K_M (0.4 / 1.1 / 2.3GB). 1.7B is the safe sweet spot; 4B squeezes in with q8 KV and
  `--fit on` auto-shrinking the draft ctx.
- Per-request startup cost: the draft must prompt-process the (potentially 64K) context once
  per new request — a few seconds of latency on top of the existing model load. Steady-state
  decode is unaffected.

## 5. Cheaper alternatives (no new hardware)

1. **IQ2_M quant** (37.3GB, already commented in ai.nix): ~30% faster target passes → ~10 tps
   baseline; stacks with a draft.
2. **`-ctk q8_0 -ctv q8_0`** on the main model: halves KV bytes, directly attacking the
   long-context problem above (small quality cost).
3. **CPU draft** (`-md <draft> -ngld 0 -td 4`): expected ~9-11 tps — marginal; the CPU is
   mostly idle during decode so it costs little to try.
4. **`--spec-type ngram-simple`**: CPU n-gram draft, zero extra model, free to try today.
5. If the Laguna GGUF ships **MTP modules** (DeepSeek-style), `--spec-type draft-mtp` is a
   built-in draft on the same device — free, but shares the iGPU bandwidth.

## 6. Recommended test plan

1. **Baseline**: note current `--metrics` tg tokens/s at your real ctx length.
2. **Free experiments first**: CPU draft, ngram draft, IQ2_M, q8 KV. Each is a llama-swap
   config tweak only.
3. **When the 1060 is installed**: driver + `--list-devices` check, then A/B:
   `-dev Vulkan0 -devd Vulkan1 -ngld 99 --spec-draft-n-max 5`.
4. Compare at **short and long** contexts — the gap between the two tells you how much of the
   win survives your real workloads.

Operational note: GTT is ~56GB with ~10GB free; the 48GB model can't be loaded twice, so A/B
tests must not run concurrently with production llama-swap usage (or use a scratch
`llama-server` on a different port).

## 7. Risks / open questions

- KV-read cost at 64K is estimated, not measured — it drives whether the win is 1.6-2x or a
  wash at your longest contexts. Measure before buying.
- Draft acceptance α=0.55-0.7 is a guess for this model pairing; small-draft acceptance on
  an 8B-active MoE is typically in this band, but it depends on the draft's quality and the
  token distribution (thinking blocks tend to accept well).
- NVIDIA driver coexistence with the current RADV setup is routine but untested on this box.
- eGPU availability/BIOS quirks are unknown (the box exposes thunderbolt in the kernel config,
  but no eGPU has been attached yet).

# GTX 1060 troubleshooting findings (2026-08-22)

Follow-up to `gtx1060-draft-report.md`. Driver bring-up succeeded (legacy_580 /
PRIME offload / kernel-7.2 strncpy patch — see git history for
`machines/aibox/nvidia.nix`), but the card is **not usable for compute yet**.
This documents the investigation and the one config fix applied.

## Symptom

"llama.cpp which used to use Vulkan on the AMD iGPU is now using CPU only."

## Short answer

Not a device *enumeration* problem. When the card is healthy,
`llama-server --list-devices` lists both devices:

```
Vulkan0: AMD Radeon 660M (RADV REMBRANDT) (60416 MiB, 60386 MiB free)
Vulkan1: NVIDIA GeForce GTX 1060 3GB (3318 MiB, 3242 MiB free)
```

The model failure is caused by **auto device selection splitting the main
model onto the 1060**, plus the 1060's Vulkan/driver state being **broken and
intermittent**. Details below.

## Root causes (evidence)

### 1. Auto device selection puts layers on the 1060 → model load fails

With two Vulkan devices and no `-dev` flag, llama.cpp distributes the model's
layers across both GPUs. A ~1.0 GB tensor lands on the 1060 and fails:

```
ggml_vulkan: Device memory allocation of size 1050944000 failed.
ggml_vulkan: vk::Device::allocateMemory: ErrorOutOfDeviceMemory
E alloc_tensor_range: failed to allocate Vulkan1 buffer of size 1050944000
E llama_model_load: error loading model: unable to allocate Vulkan1 buffer
```

This is the exact trap the draft report warned about: *"once the 1060 is
installed, pin the main model to the iGPU (`-dev Vulkan0`); device
auto-selection might otherwise try to use the new card for the main model."*

The 1060 exposes a proper 3 GiB `DEVICE_LOCAL` heap (memory type 7) and
`maxMemoryAllocationSize` ≈ 4 GiB, yet a 1 GB allocation fails — i.e. the
failure is not a clean heap-size limit; it's the flaky driver state (below).

### 2. The 1060's Vulkan compute is broken even when targeted directly

Forced onto the 1060 alone (`-dev Vulkan1 -ngl 4`, small model, `-fa on`):

```
ggml_vulkan: Compute pipeline creation failed for flash_attn_f32_f16_aligned
ggml_vulkan: vk::Device::createComputePipeline: ErrorUnknown
```

A subsequent run reported `invalid device: Vulkan1` — because the card had
meanwhile dropped out of enumeration entirely (next item).

### 3. The 1060 intermittently falls off the driver

After the first Vulkan use, dmesg fills with:

```
NVRM: GPU 0000:05:00.0: RmInitAdapter failed! (0x22:0x56:897)
```

and eventually `nvidia-smi` returns **"No devices were found"** while the
kernel modules stay loaded. Vulkan enumeration then shows only the AMD device.
This is why behaviour looked intermittent: sometimes the card enumerates
(split → load fails), sometimes it doesn't (load proceeds on the iGPU).

### 4. Hardware context (likely why the driver is unstable)

- **PCIe link: 2.5 GT/s (gen1) × width 1** — i.e. **PCIe 1.0 x1, ~250 MB/s**,
  not the TB3/TB4 x4 (~2.75 GB/s) the draft report assumed. The card is
  evidently not in a x16 slot / proper enclosure.
- **VRAM BAR is only 256 MB** (`Region 1: [size=256M]`); the driver exposes a
  separate 246 MiB device-local "BAR" Vulkan heap alongside the 3 GiB heap.
- Boot-time dmesg already showed the display engine failing:
  `nvidia-modeset: Display engine push buffer channel allocation failed`
  → `Failed to allocate NvKmsKapiDevice` (benign for compute on its own, but a
  sign the card's init is marginal).
- The `RmInitAdapter 0x22:0x56:897` failure plus the above strongly suggests a
  hardware/integration problem (slot/adapter/riser/contact), not a config bug.

## Fix applied

`machines/aibox/ai.nix`: the router's llama-server command now pins the main
model to the iGPU:

```
llama-server ... -t 12 -dev Vulkan0 ...
```

Verified the per-model spawn args inherit it (`"--device","Vulkan0"`). Draft
models should later use `-devd Vulkan1` (a separate flag), so this pin doesn't
conflict with the draft plan.

## Open issue (at wrap-up): llama-swap router

After the switch, llama-swap returned `{"error": "no router for requested
model"}` and its `/v1/models` lists only the `router` entry, even though the
router upstream (port 5800, `--models-dir` mode) comes up, passes health, and
lists the models at `/upstream/router/v1/models`. The models sync that
normally populates llama-swap's list didn't happen after the health check
(pre-change logs show `GET /upstream/router/models` right after health passes;
post-change it's absent). Likely disrupted by the RmInitAdapter storm while
the router server was starting (Vulkan init enumerates the NVIDIA ICD). Needs
a llama-swap debug run (`logLevel: debug`) or a look at
`internal/router/*.go` to confirm. A `systemctl restart llama-swap` did not
resolve it within the observed window.

## Unrelated

- `nixos-rebuild switch` exited non-zero because `libvirtd` failed to start
  (`Failed to unseal secret using TPM2: No locks available`) — pre-existing
  TPM2 issue on this box, unrelated to the GPU work.

## Recommendations

1. **Physical check of the 1060**: it is running at PCIe gen1 x1 with a 256 MB
   BAR — reseat / move to a real x16 slot / verify the riser or enclosure.
   Until the link and BAR are sane, the card can't be relied on for even the
   draft model (Vulkan compute pipeline creation fails and RM init drops the
   card). This is the gating item for the draft experiment.
2. **Main model**: keep `-dev Vulkan0` — it restores iGPU-only behaviour.
3. **llama-swap router**: investigate the models sync (debug logging) or
   restart the service once the 1060 is stable; the main-model regression and
   the router issue are independent.
4. If the card cannot be made stable, drop it — the draft experiment has free
   fallbacks in the report (CPU draft, ngram draft, IQ2_M, q8 KV).

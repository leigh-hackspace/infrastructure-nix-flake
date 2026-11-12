# llama.cpp (Vulkan) serving **Ternary-Bonsai-2-9B-Q2_0** and its vision projector on the
# GTX 1080 Ti, and the network's third inference endpoint.
#
# ## Served since 2026-11-12, and what this card can actually hold
#
# Chosen against what the card holds at this pin, not against a leaderboard.  The engine's
# own `/v1/models` on the live unit says what is in the file: `n_params 8,953,803,264`,
# `ftype Q2_0`, `size 2,545,600,512`, `n_ctx 262144 = n_ctx_train`, `n_embd 4096`,
# `capabilities: completion, multimodal`.
#
# **`Q2_0` is a name that means two different things in this family, and only one loads.**
# prism-ml publishes each Bonsai-2 size in a pruned/ternary build (`-PTQ1_0`,
# `-PQ2_0-MTP-Q8_0`) and a plain build (`-Q2_0`); the plain one loads at this pin and the
# others are refused at load.  So ask the loader, not the filename: its banner prints
# `ftype : Q2_0`, and 2,545,600,512 bytes over 8,953,803,264 parameters is 2.27 bits/weight,
# i.e. the file *is* carrying ternary blocks; what it does not carry is the pruned layer
# geometry the other builds trade for that name.  Same class of refusal, measured 2026-11-12:
# `--spec-draft-model` on `Ternary-Bonsai-2-9B-DFlash2-Q8_0.gguf` answers
# `failed to load draft model`, because that file's header says `general.architecture = dflash`
# and 0.9.0 has no such arch — which is also why **no spec-decoding flag is shipped here**:
# `--spec-type ngram-simple` was measured too, and bought nothing (62.8 t/s vs 64.0 plain).
#
# Everything below was measured on this card 2026-11-12 with the unit's own flags (`-ngl all`,
# q8_0 KV, `--flash-attn on`, 8 threads) and the whole card to itself (`llama-server` stopped
# first — a probe that runs beside the served model measures the *probe's* headroom minus a
# model, which is a different question):
#
#   | what | measured |
#   | ---- | -------: |
#   | model buffers | 2,154.86 MiB on Vulkan0 + 272.81 MiB CPU-mapped (`offloaded 33/33 layers`) |
#   | KV at 262,144 cells | **4,352.00 MiB** — 8 full-attention layers of 32, K q8_0 2,176 + V q8_0 2,176 = **16.6 KiB/token** |
#   | recurrent state | 50.25 MiB **per slot**, not per token (32 layers: S 48.00 + R 2.25) |
#   | compute buffers | 360.02 MiB Vulkan0 + 288.02 MiB host |
#   | peak VRAM, shipped flags **including** `mmproj` | **8,005 MiB of 11,264 MiB** — 3.26 GiB free |
#   | decode | **63.1–64.0 tok/s** steady (`-n 128`/`-n 256`, `graphs reused = 22`) |
#   | prefill | **305.6 tok/s** over a 1,650-token prompt |
#
# Two things that fact buys.  The served model keeps its **trained** 262,144-token context
# (the KV slope is 16.6 KiB/token against ~3.4 GiB of free card → the arithmetic's next
# doubling will not fit; the ladder below is LFM3's, see "Sizing") and it decodes at
# **63–64 t/s, the same speed as the LFM3 it replaced** — the live journal of the old
# configuration read 60.1–66.3 t/s — so this is a 5× bigger model at the same tokens per
# second, not the 2.6× decode penalty a dense model of this size would pay.  Hybrid
# (SSM) models on this card run at ~100 GB/s *effective* (LFM3 measured 111 GB/s, the dense
# Qwen2.5-VL at 290 GB/s), which is why the SSM layers cost about the same per token
# whatever the width, and why per-token speed barely moved.
#
# **Two flags are a pair, and the pair is load-bearing.** `--flash-attn on` is what lets
# `--cache-type-v q8_0` exist at all: with `--flash-attn off` this engine refuses to start,
# `llama_init_from_model: quantized V cache requires flash_attn to be enabled` (measured
# 2026-11-12).  And `on` is honoured only halfway, in the two wordings this box prints: the
# **cli** answers `resolve_fused_ops: layer N is assigned to device Vulkan0 but Flash Attention
# over K/V rows is assigned to device CPU (usually due to missing support)` and the **server**
# — what actually runs here — answers the shorter `resolve_fused_ops: Flash Attention over K/V
# rows not supported, set to disabled`, both measured 2026-11-12 and both the same class of line
# the Qwen2.5-VL run printed on this box.  Either way: the flag is accepted, and the K/V
# reduction is not a Vulkan graph, so flash attention is not what makes this model fast on this
# card.  Removing `--flash-attn on` therefore means removing the q8_0 V cache in the same change,
# not just the flag.
#
# The bigger member of the family is on the shelf beside it and was measured too, because
# "advanced" was the question and this is the boundary. `Ternary-Bonsai-2-27B-Q2_0` (7,626,008,928 B
# + its 629,246,976 B `mmproj`) puts **6,921.11 MiB of weights on the card** (`offloaded 65/65
# layers`, CPU-mapped 341.02), its 16 full-attention layers of the 64 make KV **33.0 KiB/token**
# (65,536 cells = 2,176 MiB), and its recurrent state is **149.62 MiB per slot** — all four
# slots' state is 598 MiB. `-c 131072` **fails** on this card: `ggml_vulkan: Device memory
# allocation of size 570425344 failed`, `llama_init_from_model: failed to initialize the
# context: failed to allocate buffer for kv cache`. At `-c 65536` it starts and decodes at
# **24.8–26.7 tok/s**. So shipping it is one filename plus one `ctxSize`, and the trade is
# 3× the parameters for 2.4× slower decode and 4× less context; what it is *not* is a
# stronger model per second of wall clock, which nobody here has measured (an eval, not a
# guess — `machines/services3/README.md` has the VLM eval harness for the image side of it).
#
# What is **not** on the shelf, and why: every Bonsai-2 size above 27B and the dense Qwen3.5
# quants (Q3_K_S 11.45 GiB, IQ4_XS 13.95 GiB) do not fit 11,017 MiB of usable card at all, and
# CPU spill is not a fallback here — four Skylake cores on a 12 GB model decode at single-digit
# tok/s, which is a chat timeout rather than a slow answer.
#
# ## Why a third engine, and why this llama.cpp
#
# services2 runs Strata's CUDA engine (`strata.int`), aibox runs Strata's HIP engine
# (`strata2.int`) and a llama.cpp build of the K2 fork behind `ai.*`.  None of those can
# load this model: it is a **Liquid LFM3**, and its GGUF header says
# `general.architecture = lfm3` — read out of the file's own KV section, not inferred
# from the filename — and `lfm3` is in neither of the llama.cpp trees this flake could
# otherwise build from (the fork input is 0.3.0, nixpkgs' `llama-cpp-vulkan` at this lock
# is 0.6.0; both know only `lfm2`/`lfm2moe`).  The engine here therefore comes from the
# upstream input `llama-cpp-upstream`, whose header in ../..//flake.nix carries the whole
# argument.  If that input is ever moved or replaced, `llama-server --model <file>` here
# fails with an unknown-architecture error at startup, not quietly.
#
# ## The model file is runtime data, outside the store
#
# The served file is `/srv/models/Ternary-Bonsai-2-9B-Q2_0.gguf` — **2,556,640,128 bytes**,
# fetched 2026-11-12 from `prism-ml/Ternary-Bonsai-2-9B-gguf` on Hugging Face together with
# `Ternary-Bonsai-2-9B-mmproj-Q8_0.gguf` (624,229,664 B), into a directory the flake creates
# (`systemd.tmpfiles.rules` below) and nobody else populates.  LFM3's file (1,099,627,808 B)
# is no longer served and is still where it was: `/home/leigh-admin/Models/`, itself copied
# 2026-11-11 from `/home/cjdell/Models/`, where the original remains.  Two changes came from
# the same measurement pass: the model moved, **and** its home moved out of `/home/leigh-admin`
# (whose mode is `0700`) into `/srv/models` (`0755`, root-owned, files `0644`).  This is the
# same arrangement both Strata engines document: the weights are not built by the flake, are not re-prepared by an engine
# bump, and are the one thing a switch cannot fetch for you.  The service therefore
# points at a path and will restart forever if it is missing (`INFRA.mkNeverGiveUp`) —
# which is the right failure mode for a bench box someone moves disks around on, and the
# wrong one for anything important: `journalctl -u llama-server` says
# `failed to load model` in that case, and the door answers 502.
#
# ## Sizing, and what was measured rather than guessed
#
# 8 threads = the i7-6700's SMT pair count. `-ngl all` fits easily: the whole model is
# 1.1 GB against an 11 GB card, so **the context is what decides VRAM here** — and the ladder
# below is measured on this card, not modelled: same flags as the unit, one probe per row, the
# peak `nvidia-smi` reading while the server was listening (2026-11-11; the recipe is in
# `README.md`).
#
#   | `--ctx-size` |                  peak VRAM |
#   | ------------ | --------------------------: |
#   | 32,768       |             1,368 MiB (shipped value for the first day) |
#   | 65,536       |                    1,610 MiB |
#   | 131,072      |                    2,082 MiB |
#   | 196,608      |                    2,554 MiB |
#   | **262,144**  |   **3,026 MiB** ← `ctxSize` below |
#   | 393,216      |                    3,971 MiB |
#
# That ladder is **LFM3's**, kept because it is the measured shape of the old model and the
# story of what capped it.  The served model's slope is **16.6 KiB/token** (the KV rows
# quoted above): one doubling past the trained 262,144 would need ~4 GiB more than the card
# has, which is what makes the trained length the ceiling rather than a generous margin.
# Straight-line on LFM3: ~1,137 MiB fixed plus **7.4 KiB per token**, so even its 393 KiB row
# leaves 7 GiB of the 11,264 MiB card untouched — **memory is not what caps this endpoint**.  What caps
# it is the model: the GGUF's own trained context is **262,144**, which llama.cpp volunteers as
# soon as you ask for more (`ctx_train (262144)`).  So `ctxSize` is exactly that — the biggest
# context this model was *trained* for, on 27% of the card.  Above it is rope extrapolation on a
# model nobody has measured past its trained length, i.e. a bigger number that answers worse; if
# that is ever genuinely wanted the change is `--rope-scaling yarn --yarn-orig-ctx 262144` plus a
# re-measure of retrieval, not a larger `--ctx-size`.  KV stays q8_0 for the same reason: q4_0
# would roughly halve the slope and buy nothing against a ceiling that is not memory.
#
# Two cross-checks on that line, because a table like this is worth doubting: the previous owner
# measured **this file** committing **4,457 MiB** at 262,144 tokens with *default* KV types on a
# 6 GiB GTX 1060 — which is the same intercept with a f16 slope, and it lands within 2% of
# 1,137 + 262,144 × 13.3 KiB.  And 1,368 MiB, the row this file shipped with, is what
# `nvidia-smi --query-compute-apps` reported on the live box that day.
#
# Re-measure after any engine, driver or KV-type change (the slope belongs as much to
# `--flash-attn on` and `q8_0` as it does to the model), with:
#   sudo systemctl stop llama-server && for c in 131072 262144 393216; do … done   # README
#
# ## Reachability, and why the bind is not loopback
#
# The front door is `gw`, another box, on another VLAN: a loopback bind would be
# unreachable from it, and "the app is loopback" is only a guarantee when nothing else
# dials it (AGENTS.md, the `X-WEBAUTH-USER` rule).  So this binds **the box's own
# address, read from `common/hosts.nix`** rather than typed — one fact in one place, and
# it follows the address when the box is racked onto VLAN 225 (`README.md`, "The move to
# VLAN 225").  The door's row is `llama.int.leighhack.org` in `common/front-door.nix`
# with the default LAN ACL; the port is reachable from any host the router routes to
# this VLAN, which is the same exposure aibox's leaf apps have, and it is why this box's
# firewall staying off is a decision recorded in ./networking.nix rather than an accident.
#
# It still runs as **root**, like both Strata engines, but no longer out of a user's home:
# the weights moved to `/srv/models` on 2026-11-12, which the flake creates `0755` root-owned
# and populates by hand.  What that change does **not** buy is the system account — the unit
# is still uid 0, still inherits root's `HOME` unless `StateDirectory` says otherwise (and it
# does, for the Vulkan shader cache), and a `0700` home is still what a fresh box's `~/Models`
# would be.  ./README.md's open items own the rest of that upgrade.
{
  pkgs,
  INFRA,
  flakeInputs,
  ...
}: let
  HOSTS = import ../../common/hosts.nix;

  # Upstream llama.cpp **0.9.0** (the `llama-cpp-upstream` input) — but built with
  # nixpkgs' `llama-cpp` expression rather than llama.cpp's own flake, and with the
  # embedded web UI **on, built from this same tree**.  Both halves of that sentence are
  # load-bearing, and both were chosen by failure rather than taste:
  #
  #   * **Why not upstream's flake package:** its web UI is not optional in practice.
  #     `webui` is an attribute of the `mkDerivation` set, which makes it a
  #     *derivation input* whatever `useWebUi` says, so neither `.override { useWebUi =
  #     false; }` nor a scope override stops the **808 npm tarball derivations** it needs
  #     from being built (measured at this pin: `grep -c "^building .*\.tgz\.drv"` over the
  #     build log) — and then the UI build itself fails in the sandbox:
  #
  #       EACCES: copyfile '/nix/store/…-source/logo/svg/app/web/favicon-dark.svg'
  #               → '/build/source/static/favicon-dark.svg'
  #         (@vite-pwa/assets-generator, via pwa-assets-dark.config.ts, inside
  #         `webui-0.0.0.drv`; `importNpmLock` symlinks the source tree into /build
  #         and the PWA step then tries to write into it)
  #       error: Cannot build '…-llama-cpp-vulkan-0.0.0.drv'.  Reason: 1 dependency failed.
  #
  #     measured on aibox 2026-11-11; the failing drv is nested inside upstream's
  #     `package.nix` (`finalAttrs.webui`) and is unreachable from an override, so the
  #     choice is "carry the bug" or "build the same source elsewhere".
  #   * **Why nixpkgs' expression, and what the override actually does:** nixpkgs' `llama-cpp`
  #     at this lock is 0.6.0, which has no `lfm3` arch, so only its *build* is reused —
  #     `src` becomes the input's tree and the version says so. `patches = []` because nixpkgs'
  #     single `fetchpatch` is a 0.6.0-era fix for test-model generation under
  #     `GGML_BACKEND_DL` — not wanted with dynamic dispatch off, and not applicable to 0.9.0's
  #     tree anyway. `cpuArchDynamicDispatch = false` because this is one known x86-64 CPU, not
  #     a binary distribution. Everything else of nixpkgs' recipe is kept, **including the UI**:
  #     `preConfigure` runs `npm run build` in `tools/ui` against the `node_modules` that
  #     `npmConfigHook` links out of `npmDeps`, and cmake's `scripts/ui-assets.cmake` gzips
  #     whatever ends up in `dist/` into a generated `ui.cpp` that the server then serves.
  #
  # **The UI, and the 404 that hid it.** `https://llama.int.leighhack.org/` answering
  # `{"error":{"message":"File Not Found"}}` under `Server: llama.cpp` was neither nginx nor the
  # door row — it was a binary with an **empty embedded asset table**, and two defaults in
  # `CMakeLists.txt` made it that way quietly: `LLAMA_BUILD_UI` is **OFF** (:137), and
  # `LLAMA_USE_PREBUILT_UI` is **ON** (:138), whose fallback is to *download* the prebuilt UI
  # from a Hugging Face bucket (`ui-assets.cmake`, priority 3) — impossible in a sandbox, and
  # `ui-assets.cmake:144` then generates the empty table **without failing the build**. So the
  # engine compiled clean, started clean, served its API clean and had no UI; the only trace
  # was a `404` at one path nobody had asked for yet. Hence both flags below: build the assets
  # (`LLAMA_BUILD_UI=ON`) and refuse the network fallback (`LLAMA_USE_PREBUILT_UI=OFF`), so the
  # failure cannot be silent again — either the assets are there or the build fails.
  #
  # `npmDepsHash` is deliberately **not** nixpkgs', whose pin is for 0.6.0's lockfile: it is
  # what *this* tree's `tools/ui/package-lock.json` hashes to, read off the mismatch the first
  # attempt reported (`got: sha256-73PXSTOtXjdk97azLoaGUHam7Ua/nC2jMrHjAhLTSXM=` in the
  # `…-llama-cpp-0.9.0-npm-deps` fetch). A `nixpkgs` bump that moves `llama-cpp` past the
  # release that has `lfm3` should delete the whole `overrideAttrs` — see docs/follow-ups.md.
  #
  # Checks, in the order that tells you which of the three stories you are in:
  #
  #   curl -sS http://10.3.2.213:8080/ | head -c 120      # HTML = embedded UI is in the binary
  #   llama-server --help | grep -E '^  --ui'             # runtime switch --ui/--no-ui, on by default
  #   nix build --no-link 'github:ggml-org/llama.cpp#packages.x86_64-linux.vulkan'   # the upstream-flake bug above
  #
  engine =
    (pkgs.llama-cpp.override {
      vulkanSupport = true;
      cpuArchDynamicDispatch = false;
    }).overrideAttrs (final: prev: {
      src = flakeInputs.llama-cpp-upstream.outPath;
      version = "0.9.0";
      patches = [];
      npmDepsHash = "sha256-73PXSTOtXjdk97azLoaGUHam7Ua/nC2jMrHjAhLTSXM=";

      # `npmConfigHook` points `npm_config_cache` at `$npmDeps` (fetcher v1 layout), and npm
      # 11 still wants `_cacache/tmp` in that cache even for an offline install — i.e. it tries
      # to `mkdir` inside a store path: `EACCES … /…-npm-deps/_cacache/tmp`, "Your cache folder
      # contains root-owned files". The hook's own documented answer (its error prints the
      # list) is this flag, which copies the cache into the build tree first.
      makeCacheWritable = true;
      cmakeFlags =
        prev.cmakeFlags
        ++ [
          "-DLLAMA_BUILD_UI=ON"
          "-DLLAMA_USE_PREBUILT_UI=OFF"
        ];

      # nixpkgs runs CTest for this package, and four of 0.9.0's tests cannot pass in a
      # sandbox: `test-download-model` (wants the network), `test-conversion-fp8` and
      # `test-kv-cache-scales` (python behaviour), and `test-llama-context-model`, which
      # CTest reports `Not Run` behind them; the other ~60 behaviour tests pass. That is a
      # suite written for a developer machine, not a defect in this build, so the phase is
      # switched off rather than the tests patched. Re-check on a newer tree with
      # `doCheck = true` if the engine starts misbehaving.
      doCheck = false;
    });

  # Where the served weights live.  `/home/leigh-admin` is `0700`, so a root unit reading a
  # model under it works only because uid 0 ignores the mode bits — the arrangement the
  # README's open items name as the cheap upgrade, taken 2026-11-12: the flake creates the
  # directory (`tmpfiles.rules` below; a `d` rule owns a directory that already holds files
  # without emptying it), and the weights are fetched into it by hand, because no switch can
  # download 2.4 GB of runtime data for you.
  modelsDir = "/srv/models";
  modelName = "Ternary-Bonsai-2-9B-Q2_0.gguf";

  # The vision projector for the model above, loaded by the same `-ngl all` decision.  This
  # is what makes `/v1/models` answer `capabilities: ["completion","multimodal"]` and
  # `input_modalities: ["text","image","video"]`; `--no-mmproj` is the opt-out and the
  # engine also has `--mmproj-offload`.  Measured: +624 MiB of weights, and the shipped
  # context still fits (8,005 MiB of 11,264 MiB).
  mmprojName = "Ternary-Bonsai-2-9B-mmproj-Q8_0.gguf";

  port = 8080;

  # The served model's trained maximum (`/v1/models` reports `n_ctx 262144 = n_ctx_train`),
  # and it does fit here: 8,005 MiB of the 11,264 MiB card including the vision projector,
  # measured 2026-11-12 with the whole card to itself.  LFM3's 3,026 MiB at this same number
  # is history, in the header.  `n_slots = 4`, `kv_unified = 'true'` (the server's own load
  # line), so the 4,352 MiB KV pool is shared, but 4 × 50.25 MiB of recurrent state is not.
  ctxSize = 262144;

  # Which Vulkan device the model lands on.  `--list-devices` on the box is the only
  # honest source for this string, and it is not a constant.  Measured 2026-11-12, it is
  # exactly one device — `Vulkan0: NVIDIA GeForce GTX 1080 Ti (11510 MiB, 8135 MiB free)`
  # — so nothing is currently competing for index 0.  What *would* compete: a second card
  # (DMI says `PCIEX16_2`/`PCIEX16_3` free — see ./README.md, which also answers the
  # recurring "where is the 1060?": it was **aibox's**, pulled 2026-09-02, and is not in
  # this box at all), and Mesa's `llvmpipe`, which `vulkaninfo` does enumerate as a second
  # device even though llama.cpp's own list filters it out.  Order is decided by ICD load
  # order, not by PCI slot.  **The first version of this comment claimed the iGPU was a
  # second Vulkan device and it was wrong** — inherited from the pre-onboarding notes
  # without measuring; `lspci -nn | grep -ciE 'intel.*(vga|display|3d)'` is **0**, the HD
  # Graphics 530 has no display function on this bus at all.  Re-measure after any driver,
  # card or nixpkgs change with:
  #
  #   sudo systemd-run --collect --wait --pty -p Environment=HOME=/root \
  #       -p Environment=LD_LIBRARY_PATH=/run/opengl-driver/lib \
  #       "$(sudo readlink /proc/$(systemctl show llama-server -p MainPID --value)/exe)" --list-devices
  #
  #   …  or without touching systemd at all:  nvidia-smi -L ; vulkaninfo --summary | grep -E 'deviceName|driverName'
  #
  # `--collect --wait` is not decoration.  A `systemd-run --pty` probe **left two failed
  # transient units behind on this box** (2026-11-12, from checking what `--device` accepts),
  # and `is-system-running` then read `degraded` for the rest of the session — the exact
  # state that `docs/follow-ups.md` has a whole section about on services1, manufactured out
  # of a diagnostic.  If a probe does leave one: `systemctl reset-failed`.
  #
  # and if the NVIDIA card is ever *not* `Vulkan0` — a second card fitted, or a driver/ICD
  # change — set the label `--list-devices` prints for it.  Do **not** reach for the device's
  # display name: measured 2026-11-12 on this very engine, `--device "NVIDIA GeForce GTX 1080
  # Ti"` is refused with `invalid device: …`, so the accepted vocabulary is the enumerated
  # labels (`Vulkan0`, `Vulkan1`, …), not names.  That falsified what this comment used to
  # assert ("fix it by name, which `--device` accepts"), which is the same mistake as the
  # iGPU line above: a plausible claim about somebody else's CLI, written without running it.
  vulkanDevice = "Vulkan0";
in {
  # The engine on PATH as well as in the unit, so `llama-server --list-devices` and a
  # one-off `--model … --info` are one command at a console rather than a store-path
  # hunt — which is how the device string above gets re-measured.
  environment.systemPackages = [engine];

  # The weights' home is the flake's; the weights themselves are runtime data.  A `d` rule
  # owns the directory on every activation without touching what is in it, so a reinstalled
  # box gets the path and a moved model file still fails the way the header says it fails: a
  # unit that restarts forever and a door that answers 502, with `failed to load model` in the
  # journal.
  systemd.tmpfiles.rules = ["d ${modelsDir} 0755 root root -"];

  systemd.services.llama-server = INFRA.mkNeverGiveUp {
    description = "llama.cpp (Vulkan) — Ternary-Bonsai-2-9B-Q2_0 + vision on the GTX 1080 Ti, services3";

    after = ["wait-for-network.service"];
    wants = ["wait-for-network.service"];
    wantedBy = ["multi-user.target"];

    serviceConfig = {
      ExecStart = ''
        ${engine}/bin/llama-server \
          --host ${HOSTS.services3} \
          --port ${toString port} \
          --model ${modelsDir}/${modelName} \
          --alias ${builtins.substring 0 (builtins.stringLength modelName - 5) modelName} \
          --mmproj ${modelsDir}/${mmprojName} \
          --device ${vulkanDevice} \
          --n-gpu-layers all \
          --ctx-size ${toString ctxSize} \
          --cache-type-k q8_0 --cache-type-v q8_0 \
          --flash-attn on \
          --threads 8 \
          --reasoning-preserve
      '';

      # The Vulkan pipeline/compile cache wants a writable HOME; without it llama.cpp
      # falls back to whatever HOME the unit inherits (root's, for a root unit) and the
      # cache silently disappears on every restart, which shows up as a slow first
      # request after each switch rather than as an error.
      StateDirectory = "llama-server";
      Environment = [
        "HOME=/var/lib/llama-server"
        "XDG_CACHE_HOME=/var/lib/llama-server/cache"
      ];

      # Deliberately **no** `VK_ICD_FILENAMES` here. It overrides ICD discovery rather
      # than adding to it, so a wrong path does not fail safe — it turns a working box
      # into one where llama.cpp reports no Vulkan device at all. NixOS collects the
      # driver's ICD JSON into `/etc/vulkan/icd.d/` from
      # `hardware.graphics.extraPackages` (./nvidia.nix), and that is the mechanism to
      # trust; the check that it happened is `vulkaninfo --summary` on the box, not a
      # variable set in the unit.
    };
  };
}

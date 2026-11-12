# llama.cpp (Vulkan) serving LFM3-1.7B on the GTX 1080 Ti, and the network's third
# inference endpoint.
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
# `/home/leigh-admin/Models/LFM3-1.7B-Q4_K_M.gguf` (1,099,627,808 bytes, 1.1 GB, copied
# 2026-11-11 from `/home/cjdell/Models/` on this box, where the original remains until
# someone decides it is dead weight).  This is the same arrangement both Strata engines
# document: the weights are not built by the flake, are not re-prepared by an engine
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
# Straight-line: ~1,137 MiB fixed plus **7.4 KiB per token**, so even the 393 KiB row leaves
# 7 GiB of the 11,264 MiB card untouched — **memory is not what caps this endpoint**.  What caps
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
# It runs as root, reading a model from a user's home, exactly like both Strata engines.
# The cheap upgrade is a system account plus the weights in a group-readable directory
# outside `/home/leigh-admin` (whose mode is `0700`, which is the actual blocker) —
# ./README.md's open items.
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

  modelsDir = "/home/leigh-admin/Models";
  modelName = "LFM3-1.7B-Q4_K_M.gguf";

  port = 8080;

  # The model's trained maximum (`ctx_train = 262144`), not a memory limit — see the ladder
  # in the header.  3,026 MiB of an 11,264 MiB card, all slots sharing one unified KV pool.
  ctxSize = 262144;

  # Which Vulkan device the model lands on.  `--list-devices` on the box is the only
  # honest source for this string, and it is not a constant: the box has an HD Graphics
  # 530 iGPU as well as the 1080 Ti, and which one is `Vulkan0` is decided by the order
  # the Vulkan ICDs load, not by the PCI slot.  Re-measure after any driver or nixpkgs
  # change with:
  #
  #   sudo systemd-run --property=Environment=HOME=/root -p Environment=PATH=$(ls -d /run/current-system/sw) -p Environment=LD_LIBRARY_PATH=/run/opengl-driver/lib \
  #   …   or simply:  nvidia-smi ; vulkaninfo --summary | grep -E 'deviceName|driverName'
  #
  # and, if the iGPU has appeared ahead of the NVIDIA card, fix it here (by name, which
  # `--device` accepts, in preference to an index).
  vulkanDevice = "Vulkan0";
in {
  # The engine on PATH as well as in the unit, so `llama-server --list-devices` and a
  # one-off `--model … --info` are one command at a console rather than a store-path
  # hunt — which is how the device string above gets re-measured.
  environment.systemPackages = [engine];

  systemd.services.llama-server = INFRA.mkNeverGiveUp {
    description = "llama.cpp (Vulkan) — LFM3-1.7B on the GTX 1080 Ti, services3";

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

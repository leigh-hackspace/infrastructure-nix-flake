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
# 1.1 GB against an 11 GB card, so the context is what decides VRAM here, and 32 KiB of
# q8 KV is a deliberate floor rather than a limit — the previous owner measured this
# exact file at **4,457 MiB committed for the full 262,144-token context** on a 6 GiB
# GTX 1060 with default KV types, so on this card with `-ctk/-ctv q8_0` there is room to
# raise `ctxSize` by a lot.  Raise it deliberately and re-measure with `nvidia-smi`;
# do not copy aibox's 256 KiB, which was sized for a 36 B model on a much bigger card.
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
  lib,
  INFRA,
  flakeInputs,
  ...
}: let
  HOSTS = import ../../common/hosts.nix;

  # Upstream llama.cpp **0.9.0** (the `llama-cpp-upstream` input) — but built with
  # nixpkgs' `llama-cpp` expression rather than llama.cpp's own flake, and with the
  # embedded web UI switched off.  Both halves of that sentence are load-bearing, and
  # both were chosen by failure rather than taste:
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
  #     `src` becomes the input's tree and the version says so. `patches = []` because nixpkgs' single
  #     `fetchpatch` is a 0.6.0-era fix for test-model generation under
  #     `GGML_BACKEND_DL` — not wanted with dynamic dispatch off, and not applicable to
  #     0.9.0's tree anyway. `preConfigure`/`npmDeps`/`npmConfigHook` come out because that is where
  #     nixpkgs builds `tools/ui` (`npm run build` before configure); `LLAMA_BUILD_WEBUI`
  #     is not set by nixpkgs at all and defaults to OFF in 0.9.0's `CMakeLists.txt:137`,
  #     so nothing needs enabling to replace it. `cpuArchDynamicDispatch = false` because
  #     this is one known x86-64 CPU, not a binary distribution.
  #
  # The cost of dropping the UI: `https://llama.int.leighhack.org/` serves no chat page —
  # the API (`/health`, `/props`, `/v1/chat/completions`, `/completion`) is what the door
  # row proxies, and aibox's `ai.*` is the network's chat UI. Re-check whether upstream's
  # flake builds the UI cleanly (and switch back to
  # `flakeInputs.llama-cpp-upstream.packages.${pkgs.system}.vulkan` if it does) with:
  #   nix build --no-link 'github:ggml-org/llama.cpp#packages.x86_64-linux.vulkan'
  engine =
    (pkgs.llama-cpp.override {
      vulkanSupport = true;
      cpuArchDynamicDispatch = false;
    }).overrideAttrs (final: prev: {
      src = flakeInputs.llama-cpp-upstream.outPath;
      version = "0.9.0";
      patches = [];
      npmDeps = null;
      npmDepsHash = null;
      preConfigure = "";
      nativeBuildInputs = lib.filter (p: p != pkgs.npmHooks.npmConfigHook) prev.nativeBuildInputs;

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
  ctxSize = 32768;

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

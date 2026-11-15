# MODULE — refuse to start a GPU inference unit while the loaded NVIDIA kernel
# module and the NVIDIA user-space this system was built for disagree.
#
# ## Why this exists (measured 2026-11-15, on two boxes in one deploy run)
#
# A `nixpkgs` bump moves the NVIDIA **driver** as well as the kernel. `switch` activates
# the new generation — whose `ldconfig` cache, `/run/opengl-driver` and `libcuda` are the
# **new** driver's — while the kernel still has the **old** module loaded, because only a
# reboot changes that. Activation then restarts the GPU unit into that split, and the
# unit does not fail: it just cannot talk to the card.
#
# `services3` went `active → activating` and stayed there; `llama.int` answered an empty
# `/health` and the front door 502'd; `nvidia-smi` said
# `Failed to initialize NVML: Driver/library version mismatch`. `services2` did the same
# to `strata` a few minutes later. Both generations were perfectly healthy —
# `cur == good`, zero failed units — which is exactly why it reads as a config fault or a
# bad image and is neither. The runbook told the operator to reboot; nothing on the box
# said so.
#
# ## What this does, and what it deliberately does not do
#
# One `ExecStartPre` per guarded unit, comparing the loaded module's version
# (`/sys/module/nvidia/version`, which is the *kernel's* answer) against the version this
# system's user-space was built from (`config.hardware.nvidia.package.version`, known at
# **build** time, so the check costs nothing at run time and needs no `nix` in the unit).
# On a mismatch the unit fails **with the reason in its journal and the fix in the same
# line**, and — because both guarded units already carry the house never-give-up policy
# (`Restart=always`, `StartLimitIntervalSec=0`) — it keeps retrying and **starts by itself
# the moment the machine comes back on the matching kernel**. So a pending reboot stops
# being an unexplained outage and becomes a self-healing wait with a legible cause.
#
# It is *not* a health check and it is *not* a timer, on purpose:
#
#   * It does not make the box `degraded`. `machines/services3/README.md` and commit
#     `d1f561f` already settled that a diagnostic which leaves a box degraded is a false
#     incident; here the *service* was already failing, and the guard only adds the why.
#   * It does not run on a timer or as a separate unit, so it can never flap, and it can
#     never refuse a start that would otherwise have worked: it fires only when both
#     versions are known **and differ**.
#   * If the module is not loaded at all, it says so and gets out of the way — that state
#     has its own, clearer failure downstream in the engine, and a guard that fires for it
#     would be blaming the wrong thing.
#   * In-tree drivers are out of scope: `aibox` runs Strata on `amdgpu`, which is part of
#     the kernel, so its user-space and module cannot split this way. The option defaults
#     to guarding nothing, and only the two NVIDIA boxes turn it on.
#
# ## Re-verify
#
# ```bash
# # the two values the guard compares, on a guarded box:
# cat /sys/module/nvidia/version
# nix eval --raw ".#nixosConfigurations.services2.config.hardware.nvidia.package.version"
# # the guard itself, from a real build (rule 10 — read the rendered unit, do not trust
# # what was typed):
# # grep -A1 ExecStartPre /etc/systemd/system/<unit>.service
# ```
#
# Both branches were exercised before this shipped: the real one (match → exit 0), and a
# copy of the rendered script with the expected version perturbed, which printed the
# message and exited 1.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.infra.nvidiaDriverMatchGuard;

  # The version this system's NVIDIA user-space belongs to. Evaluated at build time, so
  # the script below carries a literal and never asks Nix at run time.
  expected = config.hardware.nvidia.package.version;

  script = pkgs.writeShellScript "nvidia-driver-match" ''
    set -u
    want='${expected}'
    have="$(cat /sys/module/nvidia/version 2>/dev/null || true)"

    if [ -z "$have" ]; then
      echo "nvidia-driver-match: no nvidia module loaded yet; not judging the driver match."
      exit 0
    fi

    if [ "$have" != "$want" ]; then
      echo "nvidia-driver-match: REFUSING to start: this system was built for driver $want,"
      echo "  but the running kernel has module $have loaded."
      echo "  That is the switch-without-reboot state: a nixpkgs bump moved the NVIDIA driver,"
      echo "  'switch' installed the new user-space, and only a reboot changes the kernel module."
      echo "  Fix: reboot this box. The unit is retried and will start on its own when the"
      echo "  matching module is loaded, so no manual start is needed afterwards."
      exit 1
    fi

    echo "nvidia-driver-match: ok (kernel module and user-space are both $have)."
  '';
in {
  options.infra.nvidiaDriverMatchGuard = {
    enable = lib.mkEnableOption ''
      an NVIDIA driver-match guard on named GPU units, so that a unit whose user-space is
      newer than the loaded kernel module refuses to start with the reason (and the reboot
      advice) in its journal instead of failing opaquely'';

    units = lib.mkOption {
      type = with lib.types; listOf str;
      default = [];
      example = ["strata"];
      description = ''
        Names of existing systemd services to guard. Each gets this check prepended to its
        `ExecStartPre`, so it applies on every start and every restart, not only on boot.

        Guard units that actually open the GPU. Guarding anything else just adds a delay
        and a journal line nobody asked for.
      '';
    };
  };

  config = lib.mkIf (cfg.enable && cfg.units != []) {
    assertions = [
      {
        assertion = expected != null && expected != "";
        message = "infra.nvidiaDriverMatchGuard: config.hardware.nvidia.package.version is empty, so the guard would compare against nothing. Either hardware.nvidia is not configured on this box or the option is being enabled in the wrong place.";
      }
    ];

    systemd.services = lib.genAttrs (map lib.getName cfg.units) (_: {
      serviceConfig.ExecStartPre = lib.mkBefore [script];
    });
  };
}

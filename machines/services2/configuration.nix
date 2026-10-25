# services2-specific system configuration.  The settings shared with the other
# machines (timezone, i18n, keyboard, sudo, nix-ld, ssh, bootloader,
# autoRollback, experimental-features) live in common/base.nix; keep this file
# to what is specific to this box.
#
# services2 is a clone of services1 on AMD/Radeon hardware.  Its module list in
# flake.nix is services1's (containers and crane-args included); what it *runs* is
# narrower: its own status dashboard and, since the RTX 3060 arrived, the Strata CUDA
# engine — and nothing else, because its 30 GB of RAM is strata's.  The observability
# stack that used to live here went to services1 on 2026-10-17.  See ./README.md for
# what runs, ./STRATA.md for the engine, and add a per-service module alongside the
# first real service that needs one, not before.
{
  config,
  pkgs,
  ...
}: {
  imports = [../../common/base.nix];

  # The sandbox is the default `true`, deliberately, and this comment is here to stop the
  # setting coming back by symmetry: services1 runs `relaxed` for exactly one package of its
  # own (machines/services1/configuration.nix), and this file used to copy it "so the first
  # service that needs a build-time secret does not have to remember to turn it on" — a
  # weakening of every future build on this box for a need nobody had.  sops' build-time
  # decryption does not need it at all (`extra-sandbox-paths` in common/sops.nix is what puts
  # `/var/lib/sops-nix` inside the sandbox), and the only thing `relaxed` buys anywhere is a
  # derivation that sets `__noChroot` — measured 2026-10-25 over this box's whole build
  # closure (7,840 derivations) with
  #
  #   sudo nix-store -qR $(sudo nix-store -qd /run/current-system) | grep '\.drv$' \
  #     | xargs -r grep -l __noChroot
  #
  # which printed nothing; services1's closure has exactly one hit.  If a future package here
  # does set it, the build fails loudly with `has '__noChroot' set, but that's not allowed
  # when 'sandbox' is 'true'` — put the setting back then, naming that package, rather than
  # in advance.  (Un-parking strata is the likely candidate: re-run the command above.)
  #
  # Safety margin on "nothing changes": aibox has `sandbox = true` and has been building this
  # box's entire toplevel all along, so every derivation in the closure above is already
  # proven to build strict — this only removes the *local fallback*'s exemption.

  # GPU diagnostics for this box's GPU only.  The box is AMD (Ryzen 5700G /
  # Cezanne Radeon iGPU), so it gets the AMD tool rather than services1's
  # intel-gpu-tools; aibox (ROCm/compute) installs amdgpu_top.
  environment.systemPackages = [pkgs.radeontop];

  # This is a fresh install on the current release, so stateVersion is the
  # release it was installed from (services1/aibox keep the older release they
  # were first installed with).
  system.stateVersion = "26.05"; # Did you read the comment?
}

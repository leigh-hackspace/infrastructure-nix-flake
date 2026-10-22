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

  # services1 builds packages that reach the network/NAS at build time
  # (common/sops.nix decrypts build-time secrets).  Kept identical here so the
  # first service that needs a build-time secret does not have to remember to
  # turn it on.
  nix.settings.sandbox = "relaxed";

  # GPU diagnostics for this box's GPU only.  The box is AMD (Ryzen 5700G /
  # Cezanne Radeon iGPU), so it gets the AMD tool rather than services1's
  # intel-gpu-tools; aibox (ROCm/compute) installs amdgpu_top.
  environment.systemPackages = [pkgs.radeontop];

  # This is a fresh install on the current release, so stateVersion is the
  # release it was installed from (services1/aibox keep the older release they
  # were first installed with).
  system.stateVersion = "26.05"; # Did you read the comment?
}

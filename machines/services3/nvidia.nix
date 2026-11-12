# The GTX 1080 Ti, driven — for exactly one consumer: ./llama.nix.
#
# This file replaces the "the card is not driven" decision that `services3` was
# onboarded with (see ./README.md, "What it runs"): the box now has a job, and the job
# is a model on this card.  The card is a **GP102 (Pascal, compute 6.1)**, which is the
# whole configuration problem in one fact:
#
#   * `hardware.nvidia.open = false` — the open kernel modules are Turing and newer only.
#   * `package = nvidiaPackages.legacy_580` — 580 is the last branch that supports
#     Pascal at all (`latest` dropped Maxwell/Pascal).  Verified present at this flake's
#     nixpkgs pin (`pkgs/os-specific/linux/nvidia-x11/default.nix:157: legacy_580 = generic {`),
#     which is the re-verification this file's predecessor asked for rather than assumed.
#   * **Vulkan, not CUDA**, is the llama.cpp path: CUDA 13's nvcc no longer emits Pascal,
#     so there is no CUDA llama.cpp for this card to run.  That is why the engine in
#     ./llama.nix is `packages.vulkan` and why `hardware.graphics.enable` is on: the
#     driver's own Vulkan ICD is what makes `/dev/dri` + `nvidia-smi` into a compute
#     device llama.cpp can see.  (It is also what the box's previous owner used, on the
#     same motherboard family, for the same model.)
#
# `services.xserver.videoDrivers = ["nvidia"]` is what turns all of this on even though
# there is no display and no compositor on this box: `hardware.nvidia.enabled` is derived
# from that list (the same mechanism machines/services2/nvidia.nix documents for the
# headless RTX 3060), and with it come the kernel module, `/dev/nvidia*`, `nvidia-smi`,
# `/run/opengl-driver`, and — collected into `hardware.graphics.extraPackages` by
# `nixos/modules/hardware/video/nvidia.nix` — the Vulkan ICD JSON.
#
# What is **deliberately not here**: the `overrideAttrs` re-patch the box's previous
# flake carried for its NVIDIA build (patchelf 0.19.2 mangling `nvidia-smi`'s interpreter,
# NixOS/nixpkgs#576412, at *their* pin 20261017).  This flake's pin is 20261014, a
# different build, and copying a workaround for a fault nobody has observed here is how
# the `sandbox = "relaxed"` line came back on services2 by symmetry (read
# machines/services2/configuration.nix before re-adding anything like it).  The test is
# one command on the box: `nvidia-smi`.  If it segfaults, put the override here and cite
# the measurement.
{
  config,
  pkgs,
  ...
}: {
  services.xserver.videoDrivers = ["nvidia"];

  hardware.nvidia = {
    package = config.boot.kernelPackages.nvidiaPackages.legacy_580;
    open = false;
    modesetting.enable = true;
  };

  # GL + the Vulkan ICD collection.  Without this the driver loads, `nvidia-smi` works,
  # and `llama-server --list-devices` reports no Vulkan device at all — the failure mode
  # worth naming because the first two look healthy.
  hardware.graphics.enable = true;

  # GPU diagnostics for this box's GPU only (the house rule: services1 gets
  # intel-gpu-tools, services2 radeontop, aibox amdgpu_top).  `vulkaninfo` is the tool
  # that answers the only question this box has: which Vulkan index is the NVIDIA card.
  environment.systemPackages = [pkgs.vulkan-tools];
}

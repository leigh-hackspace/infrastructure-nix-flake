# NVIDIA GTX 1060 3GB (Pascal) — DEPRECATED.
#
# The card was removed from aibox on 2026-09-02 (see gtx1060-followup.md) and
# there is no NVIDIA GPU on the box anymore; everything below is dead config.
# On the next config pass, drop `./nvidia.nix` from default.nix and delete this
# file together with ./nvidia-580-linux-7-strncpy.patch (this legacy_580
# driver build is the patch's only remaining user).
{
  config,
  ...
}:
{
  services.xserver.videoDrivers = [ "nvidia" ];

  hardware.nvidia = {
    # legacy_580 (LTSB): the last branch supporting Maxwell/Pascal/Volta.
    # The default `stable` (595) dropped the GTX 10-series.
    branch = "legacy_580";

    # Pascal requires the closed kernel module (open modules are Turing+ only).
    # Must be explicit: driver >= 560 asserts `open` is configured.
    open = false;

    # Linux 7.x removed strncpy() from the kernel headers; 580.173.02 still
    # uses it. Patch the kernel-module sources (strscpy / strnlen equivalents)
    # before the module build. Applied like nixpkgs' own legacy_470 patches:
    # -p1 with paths relative to the kernel/ dir of the extracted .run.
    package = config.boot.kernelPackages.nvidiaPackages.legacy_580.overrideAttrs (old: {
      patches = (old.patches or [ ]) ++ [ ./nvidia-580-linux-7-strncpy.patch ];
      patchFlags = [ "-p1" "--directory=kernel" ];
    });

    prime = {
      offload.enable = true;
      # lspci: 05:00.0 -> GTX 1060
      nvidiaBusId = "PCI:5@0:0:0";
      # lspci: 76:00.0 -> bus 0x76 = 118 decimal -> Radeon 680M iGPU
      amdgpuBusId = "PCI:118@0:0:0";
    };
  };
}

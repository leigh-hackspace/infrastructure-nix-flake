{config, ...}:
# NVIDIA RTX 3060 (GA106, Ampere, compute capability 8.6) on services2.
#
# The card is here for one reason: Strata's CUDA engine (machines/services2/
# strata.nix).  There is no display attached and no X/Wayland session on this
# box, so this is the headless-compute shape of the NVIDIA module: the driver
# and its device nodes, nvidia-smi for diagnostics, and nvidia_uvm for CUDA -
# no modesetting-for-a-desktop, no PRIME.
#
# `services.xserver.videoDrivers = ["nvidia"]` is what turns the module on even
# though `services.xserver.enable` is off: hardware.nvidia.enabled is derived
# from it (nvidia.nix: `lib.elem "nvidia" config.services.xserver.videoDrivers`),
# and with it come the kernel module, /run/opengl-driver/lib/libcuda.so.1, the
# udev rules for /dev/nvidia* and nvidia-smi.
{
  services.xserver.videoDrivers = ["nvidia"];

  hardware.nvidia = {
    # Ampere is supported by the open kernel modules (Turing and later), and the
    # nixpkgs module requires an explicit choice on driver >= 560.  The open
    # module also makes the module load nvidia_uvm eagerly (see nvidia.nix),
    # which is what a headless CUDA box wants.
    open = true;

    # No display; this only affects the DRM/KMS node, harmless and it is what the
    # driver defaults to on a modern branch.
    modesetting.enable = true;
  };
}

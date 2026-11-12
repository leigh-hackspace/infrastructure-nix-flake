# Hardware configuration for services3.
#
# Measured on the box on 2026-11-11 (`bootctl`, `lsblk`, `findmnt`, `lspci`,
# `nvidia-smi`, `uname -r`) and cross-checked against the `nixos-generate-config`
# output its previous flake carried (`cjdell/nixos-config`,
# `hosts/asus-i7-6700-nixos/hardware-configuration.nix`), which is still accurate
# about this disk and this board.  The per-machine bits the other boxes carry are
# added here too: the deliberate `mitigations=off` tradeoff (paired with
# `security.sudo.wheelNeedsPassword = false` in common/base.nix, as on services2).
#
# ASUS Z170 PRO GAMING: i7-6700 (Skylake 4c/8t, HD Graphics 530 iGPU), 16 GiB DDR4,
# one SATA SSD (`sda`, 111.8 GiB: `sda1` 1 GiB vfat label BOOT, `sda2` 110.8 GiB
# ext4 label ROOT), UEFI with systemd-boot, and a **GTX 1080 Ti (GP102, Pascal)** in
# the x16 slot.  Note what that last clause means for anyone arriving from the old
# flake's docs: the machine is the same box, the cards are not — its previous owner's
# write-up describes a GTX 1060 + GTX 970, and `lspci` answers exactly one discrete
# GPU now, a 1080 Ti on 01:00.0 with the 580.178.04 driver loaded by that other tree.
{
  config,
  lib,
  modulesPath,
  ...
}: {
  imports = [
    (modulesPath + "/installer/scan/not-detected.nix")
  ];

  # UEFI + systemd-boot, which is what is installed: `bootctl` reports
  # `Product: systemd-boot 262` as the current boot loader, firmware UEFI 2.40
  # (American Megatrends), Secure Boot disabled, no TPM2.  Same shape as every other
  # box here, so common/base.nix's bootloader settings apply unchanged.
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  boot.initrd.availableKernelModules = [
    "xhci_pci" # the USB 3.0 controller the Wi-Fi dongle is plugged into
    "ahci" # sda
    "nvme"
    "usb_storage"
    "sd_mod"
  ];
  boot.kernelModules = ["kvm-intel"];
  boot.extraModulePackages = [];

  # Performance tradeoff, decided deliberately, and the same decision the other three
  # deployable boxes made (see machines/services2/hardware-configuration.nix for the
  # wording and the pairing with passwordless wheel).  It is also what the box was
  # already running: `mitigations=off` is on the live command line
  # (`cat /proc/cmdline`, 2026-11-11), so nothing is being weakened by this move.
  boot.kernelParams = [
    "mitigations=off"
  ];

  # The by-label mounts the box actually boots with (`findmnt`, 2026-11-11), not
  # by-uuid: the labels survived the disk's journey from the HP Z240 this SSD came out
  # of, and a label is readable at a console when a uuid is not.  The vfat mask is the
  # box's current `0077`, not services2's `0022` — changed only by deciding to, not by
  # copying a neighbour.
  fileSystems."/" = {
    device = "/dev/disk/by-label/ROOT";
    fsType = "ext4";
  };

  fileSystems."/boot" = {
    device = "/dev/disk/by-label/BOOT";
    fsType = "vfat";
    options = [
      "fmask=0077"
      "dmask=0077"
    ];
  };

  # No swap, and that is an observation rather than a plan: 16 GiB of RAM with nothing
  # behind it.  It is fine for what this box does today (it runs the three Prometheus
  # exporters and nothing else) and it is the first thing to revisit before it is given
  # anything that holds a model or a build in RAM — services2's
  # hardware-configuration.nix is the file to copy, and its comment about a box
  # running out of room while evaluating *this* flake is not theoretical.
  swapDevices = [];

  # **The GTX 1080 Ti is driven by ./nvidia.nix**, not here, and the reason it is a
  # separate file is the same one services2 has an nvidia.nix for: the driver branch is
  # a hardware decision (Pascal ⇒ `legacy_580`, `open = false`) with its own verification
  # steps, and it is worth being able to read on its own.  What this file keeps is the
  # part that is about the board rather than the card: the CPU microcode, kvm-intel, the
  # by-label mounts and the `mitigations=off` tradeoff.
  #
  # The iGPU (HD Graphics 530) is still not configured: the box is headless, so there is
  # no console to drive.  It is not innocent either — it is a second Vulkan device, which
  # is why ./llama.nix's `vulkanDevice` says to measure the index rather than assume it.

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.intel.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}

# Hardware configuration for services2.
#
# Derived from `nixos-generate-config` output on the installed NVMe (Kingston
# SA2000M8 250G) with the per-machine bits services1 also carries added back:
# the NAS exports (common/nas.nix turns them into mounts + automounts) and the
# deliberate mitigations=off tradeoff.
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}: {
  imports = [
    (modulesPath + "/installer/scan/not-detected.nix")
  ];

  boot.initrd.availableKernelModules = [
    "xhci_pci"
    "ahci"
    "nvme"
    "usb_storage"
    "usbhid"
    "sd_mod"
  ];
  boot.initrd.kernelModules = [];
  boot.kernelModules = ["kvm-amd"];
  boot.extraModulePackages = [];

  # Performance tradeoff, decided deliberately: CPU side-channel mitigations
  # are off on all machines (see also security.sudo.wheelNeedsPassword = false
  # in common/base.nix).  These boxes are single-operator, on a trusted LAN
  # behind authentik/nginx, and the workload is I/O- and GPU-bound; the cost is
  # real (Spectre/Meltdown class attacks) and accepted, not overlooked.
  boot.kernelParams = [
    "mitigations=off"
  ];

  boot.kernel.sysctl = {
    "kernel.task_delayacct" = 1;
  };

  # AMD Cezanne Radeon iGPU (Mesa/radeonsi provides VA-API/VDPAU), unlike
  # services1 which is Intel/i915 and needs intel-media-driver.
  hardware.graphics = {
    enable = true;
  };

  fileSystems."/" = {
    device = "/dev/disk/by-uuid/b4f380f9-e3a0-4e1b-aacc-538f78f4e36a";
    fsType = "ext4";
  };

  fileSystems."/boot" = {
    device = "/dev/disk/by-uuid/FBB6-8ECF";
    fsType = "vfat";
    options = [
      "fmask=0022"
      "dmask=0022"
    ];
  };

  # NAS shares (TrueNAS, infra.nas.host).  common/nas.nix turns each of these
  # into the mount unit *and* its automount unit, and builds wait-for-nas.service
  # from the same list — see the header of that file for why the automount units
  # have to exist explicitly (the `x-systemd.automount` mount option is dead
  # config for unit-file mounts) and why NAS-dependent services must order
  # themselves against wait-for-nas.service.
  #
  # services2 has no services yet, so nothing consumes these; they are here so
  # the box is a drop-in clone of services1 once they are enabled.
  infra.nas.exports = [
    {
      where = "/mnt/cameras";
      share = "/mnt/sas-10k/cameras";
    }
    {
      where = "/mnt/filestore";
      share = "/mnt/sas-10k/filestore";
    }
    {
      where = "/mnt/backups";
      share = "/mnt/sas-10k/backups";
    }
  ];

  swapDevices = [];

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}

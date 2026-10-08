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
    "nvme"
    "xhci_pci"
    "thunderbolt"
    "usbhid"
    "usb_storage"
    "sd_mod"
  ];
  boot.initrd.kernelModules = [];
  boot.initrd.supportedFilesystems.zfs = false;

  boot.supportedFilesystems.zfs = false;

  boot.kernelModules = ["kvm-amd"];
  boot.extraModulePackages = [];

  boot.kernelPackages = pkgs.linuxPackages_latest;

  boot.kernelParams = [
    # More speed.  CPU side-channel mitigations are off on both machines as a
    # deliberate tradeoff (same decision as passwordless wheel sudo in
    # common/base.nix): single-operator boxes on a trusted LAN behind
    # authentik/nginx, I/O- and GPU-bound workloads.  See also
    # machines/services1/hardware-configuration.nix.
    "mitigations=off"
    # IOMMU off (less overhead)
    "amd_iommu=off"
    # Power management off
    "amdgpu.runpm=0"
    # 56G of VRAM
    "amdgpu.gttsize=57344"
    "ttm.pages_limit=13668850"
    "ttm.page_pool_size=13668850"
  ];

  hardware.graphics.enable = true;

  fileSystems."/" = {
    device = "/dev/disk/by-uuid/6b17b4bc-1523-481e-b6ce-87f3ea324e27";
    fsType = "ext4";
  };

  fileSystems."/boot" = {
    device = "/dev/disk/by-uuid/0030-BB62";
    fsType = "vfat";
    options = [
      "fmask=0077"
      "dmask=0077"
    ];
  };

  # NAS shares (TrueNAS, infra.nas.host).  common/nas.nix generates the mount
  # units, their automount units and wait-for-nas.service from this list.
  # Without _netdev, the automounts and the ordering against wait-for-network
  # (all added by common/nas.nix), a NAS that is still importing after a power
  # cut can hang this host's boot — see AGENTS.md (automount gotcha).
  infra.nas.exports = [
    { where = "/mnt/filestore"; share = "/mnt/sas-10k/filestore"; }
    { where = "/mnt/ds-photos"; share = "/mnt/sas-10k/ds-photos"; }
  ];

  swapDevices = [
    {device = "/dev/disk/by-uuid/b44042ef-cd03-49b9-aa18-b923c243cba8";}
  ];

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}

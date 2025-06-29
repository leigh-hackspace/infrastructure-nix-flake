{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

{
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
  boot.initrd.kernelModules = [ ];
  boot.kernelModules = [ "kvm-amd" ];
  boot.extraModulePackages = [ ];

  boot.kernelPackages = pkgs.linuxPackages_latest;

  # boot.kernelPackages = pkgs.linuxPackagesFor (
  #   pkgs.linux_6_14.override {
  #     argsOverride = rec {
  #       src = pkgs.fetchurl {
  #         url = "mirror://kernel/linux/kernel/v6.x/linux-${version}.tar.xz";
  #         sha256 = "sha256-xcaCo1TqMZATk1elfTSnnlw3IhrOgjqTjhARa1d6Lhs=";
  #       };
  #       version = "6.14.2";
  #       modDirVersion = "6.14.2";
  #     };
  #   }
  # );

  boot.kernelParams = [
    "mitigations=off"
    "amd_iommu=on"
    "amdgpu.gttsize=57344"
    "ttm.pages_limit=13668850"
    "ttm.page_pool_size=13668850"
  ];

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

  fileSystems."/mnt/filestore" = {
    device = "10.3.1.6:/mnt/sas-10k/filestore";
    fsType = "nfs";
    options = [ "nfsvers=4.2" ];
  };

  swapDevices = [
    { device = "/dev/disk/by-uuid/b44042ef-cd03-49b9-aa18-b923c243cba8"; }
  ];

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}

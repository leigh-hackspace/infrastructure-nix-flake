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

  boot.kernelParams = [
    # More speed
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

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
  boot.initrd.supportedFilesystems.zfs = false;

  boot.supportedFilesystems.zfs = false;
  boot.supportedFilesystems.nfs = true;

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

  # NAS mounts mirror machines/services1/hardware-configuration.nix.  Both
  # shares live on the slow TrueNAS (10.3.1.6); without _netdev, automount and
  # ordering against wait-for-network a delayed NAS after a power cut can hang
  # this host's boot.  See AGENTS.md (automount gotcha) for why the automount
  # units are declared explicitly below rather than via the mount option.
  systemd.mounts = [
    {
      where = "/mnt/filestore";
      what = "10.3.1.6:/mnt/sas-10k/filestore";
      type = "nfs";
      options = "nfsvers=4.2,_netdev,x-systemd.automount,retry=5,timeo=5,x-systemd.mount-timeout=30";
      after = [ "wait-for-network.service" ];
      requires = [ "wait-for-network.service" ];
    }
    {
      where = "/mnt/ds-photos";
      what = "10.3.1.6:/mnt/sas-10k/ds-photos";
      type = "nfs";
      options = "nfsvers=4.2,_netdev,x-systemd.automount,retry=5,timeo=5,x-systemd.mount-timeout=30";
      after = [ "wait-for-network.service" ];
      requires = [ "wait-for-network.service" ];
    }
  ];

  # x-systemd.automount in the mount options above is only honoured for
  # /etc/fstab entries.  For unit-file mounts (what NixOS systemd.mounts
  # generates) the automount unit must be defined explicitly, otherwise the
  # shares would mount eagerly (or not at all) instead of lazily on first
  # access.  wait-for-nas.service (machines/aibox/nfs-client.nix) triggers
  # these automounts and waits until the shares are genuinely mounted.
  systemd.automounts = [
    {
      where = "/mnt/filestore";
      wantedBy = [ "multi-user.target" ];
    }
    {
      where = "/mnt/ds-photos";
      wantedBy = [ "multi-user.target" ];
    }
  ];

  swapDevices = [
    { device = "/dev/disk/by-uuid/b44042ef-cd03-49b9-aa18-b923c243cba8"; }
  ];

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}
